use std::env;
use std::path::PathBuf;
use std::io::Write;

use actix_files::NamedFile;
use actix_web::{App, Error, error, get, HttpServer, web};
use actix_web::web::Data;
use reqwest::header::CONTENT_TYPE;
use reqwest::Method;
use sqlx::Row;
use tempfile::NamedTempFile;
use tokio::fs::File;
use tokio::fs;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;
use tokio_stream::StreamExt;
use tracing::{error, Level};
use xxhash_rust::xxh3::xxh3_64;

use sermcs::AppState;

const VF_THUMBNAIL_VIDEO: &str = "select=eq(n\\,34),scale='if(gt(iw,ih),min(498\\, iw),-1)':'if(gt(iw,ih),-1,min(422\\, ih))',format=rgba";
const VF_THUMBNAIL_ANIMATED_IMAGE: &str = "scale='if(gt(iw,ih),min(374\\, iw),-1)':'if(gt(iw,ih),-1,min(317\\, ih))',format=rgba";
const VF_THUMBNAIL_IMAGE: &str = "scale='if(gt(iw,ih),min(498\\, iw),-1)':'if(gt(iw,ih),-1,min(422\\, ih))',format=rgba";
const VF_WEBPUBLIC_IMAGE: &str = "scale='if(gt(iw,ih),min(2048\\, iw),-1)':'if(gt(iw,ih),-1,min(2048\\, ih))',format=rgba";

const KEY_QUERY: &str = r#"
    SELECT url, type, "accessKey", "thumbnailAccessKey", "webpublicAccessKey"
    FROM public.drive_file
    WHERE "accessKey" = $1 OR "thumbnailAccessKey" = $1 OR "webpublicAccessKey" = $1
    "#;

async fn get_thumbnail_image(input_file_path: &str, output: &PathBuf, content_type: &str, method: &str) -> PathBuf {
    let image_format = content_type.split('/').nth(1).unwrap_or("");

    let mut ffmpeg_args = vec![
        "-y",
        "-i", &input_file_path,
        "-f", &image_format,
    ];

    match method {
        "video" => {
            ffmpeg_args.extend(&["-vf", VF_THUMBNAIL_VIDEO]);
            ffmpeg_args.extend(&["-vframes", "1"]);
        }
        "animated" => {
            ffmpeg_args.extend(&["-vf", VF_THUMBNAIL_ANIMATED_IMAGE]);
            ffmpeg_args.extend(&["-loop", "0"]);
        }
        "image" => {
            ffmpeg_args.extend(&["-vf", VF_THUMBNAIL_IMAGE]);
        }
        _ => {}
    }

    ffmpeg_args.push(output.to_str().unwrap());

    let _ = Command::new("ffmpeg")
        .args(&ffmpeg_args)
        .output()
        .await.map_err(|e| {
        eprintln!("{}", e);
        error::ErrorInternalServerError(format!("ffmpeg error: {}", e))
    });

    PathBuf::from(output)
}

#[get("/{tail:.*}")]
async fn detail(key: web::Path<(String, )>, data: Data<AppState>) -> Result<NamedFile, Error> {
    let key = key.into_inner().0;

    let rows = match sqlx::query(KEY_QUERY)
        .bind(&*key)
        .fetch_one(&data.db_pool)
        .await {
        Ok(rows) => rows,
        Err(e) => {
            error!("Query failed: {e}");
            return Err(error::ErrorBadGateway(format!("Query failed: {}", e)));
        }
    };

    let url: String = rows.get("url");
    let file_type: String = rows.get("type");
    let access_key: String = rows.get("accessKey");
    let thumbnail_access_key: String = rows.get("thumbnailAccessKey");
    let webpublic_access_key: String = rows.get("webpublicAccessKey");

    let is_access = access_key == &*key;
    let is_thumbnail = thumbnail_access_key == key;
    let is_webpublic = webpublic_access_key == key;

    return if is_access {
        let file_hash = xxh3_64(key.as_ref());
        let base_dir = &data.temp_dir;
        let file_path = PathBuf::from(&base_dir).join(file_hash.to_string());

        let ext_hint_path = format!("ext-{}", file_hash);
        let ext_hint_file = PathBuf::from(&base_dir).join(&ext_hint_path);
        let check = ext_hint_file.exists();

        if check {
            let file = File::open(&ext_hint_file).await.unwrap();
            let mut reader = BufReader::new(file);
            let mut ext = String::new();
            reader.read_line(&mut ext).await?;
            let file_path_with_ext = if ext.trim() == "None" {
                file_path.clone()
            } else {
                file_path.with_extension(ext.trim())
            };
            NamedFile::open(file_path_with_ext).map_err(|e| {
                eprintln!("{}", e);
                error::ErrorInternalServerError(format!("Error: {}", e))
            })
        } else {
            let mut download = File::create(&file_path).await.unwrap();

            let res = data.http_client.request(Method::GET, url).send().await.unwrap();

            let headers = res.headers().clone();
            if let Some(content_type) = headers.get(CONTENT_TYPE) {
                if content_type == "image/png" && file_type == "image/apng" {
                } else if file_type != content_type.to_str().unwrap() {
                    return Err(error::ErrorBadGateway("content-type != :failed".to_string()));
                }
            }

            let mut stream = res.bytes_stream();

            while let Some(chunk_result) = stream.next().await {
                let chunk = chunk_result.unwrap();
                download.write_all(&chunk).await.unwrap();
            }

            download.flush().await.unwrap();

            let ext = mime_guess::get_mime_extensions_str(&*file_type);
            let ext_hint_file_path = PathBuf::from(&base_dir).join(&ext_hint_path);
            let ext_hint_file_path_str = ext_hint_file_path.to_string_lossy().to_string();
            let new_file_path = if let Some(ext) = ext {
                let ext = ext[0];
                let new_file_path_with_ext = format!("{}.{}", file_path.to_string_lossy(), ext);
                fs::rename(&file_path, &new_file_path_with_ext).await.unwrap();
                File::create(&ext_hint_file_path_str).await.unwrap().write_all(ext.as_bytes()).await.unwrap();
                new_file_path_with_ext
            } else {
                File::create(&ext_hint_file_path_str).await.unwrap().write_all("None".as_bytes()).await.unwrap();
                file_path.to_string_lossy().to_string()
            };

            NamedFile::open(new_file_path).map_err(|e| {
                eprintln!("{}", e);
                error::ErrorInternalServerError(format!("Error: {}", e))
            })
        }
    } else if is_thumbnail {
        let file_hash = xxh3_64(format!("{}-thumbnail", key).as_ref());
        let base_dir = &data.temp_dir;

        let file_path_avif = PathBuf::from(&base_dir).join(format!("{}-thumbnail.avif", file_hash));
        let file_path_webp = PathBuf::from(&base_dir).join(format!("{}-thumbnail.webp", file_hash));

        let check_avif = file_path_avif.exists();
        let check_webp = file_path_webp.exists();

        let file_path = if check_avif {
            file_path_avif
        } else if check_webp {
            file_path_webp
        } else {
            let mut download = NamedTempFile::new().unwrap();

            let res = data.http_client.request(Method::GET, url).send().await.unwrap();

            let headers = res.headers().clone();
            if let Some(content_type) = headers.get(CONTENT_TYPE) {
                if content_type == "image/png" && file_type == "image/apng" {
                } else if file_type != content_type.to_str().unwrap() {
                    return Err(error::ErrorBadGateway("content-type != :failed".to_string()));
                }
            }

            let mut stream = res.bytes_stream();

            while let Some(chunk_result) = stream.next().await {
                let chunk = chunk_result.unwrap();
                download.write_all(&chunk).unwrap();
            }

            download.flush().unwrap();

            let file_path = if file_type.starts_with("video/") {
                let thumbnail_path = PathBuf::from(&base_dir).join(format!("{}-thumbnail.avif", file_hash));
                File::create(&thumbnail_path).await.unwrap();
                get_thumbnail_image(download.path().to_str().unwrap(), &thumbnail_path, "image/avif", "video").await
            } else if file_type == "image/apng" || file_type == "image/gif" {
                let thumbnail_path = PathBuf::from(&base_dir).join(format!("{}-thumbnail.webp", file_hash));
                File::create(&thumbnail_path).await.unwrap();
                get_thumbnail_image(download.path().to_str().unwrap(), &thumbnail_path, "image/webp", "animated").await
            } else {
                let thumbnail_path = PathBuf::from(&base_dir).join(format!("{}-thumbnail.avif", file_hash));
                File::create(&thumbnail_path).await.unwrap();
                get_thumbnail_image(download.path().to_str().unwrap(), &thumbnail_path, "image/avif", "image").await
            };

            file_path
        };

        NamedFile::open(file_path).map_err(|e| {
            eprintln!("{}", e);
            error::ErrorInternalServerError(format!("Error: {}", e))
        })
    } else if is_webpublic {
        let file_hash = xxh3_64(key.as_ref());
        let base_dir = &data.temp_dir;
        let file_path = PathBuf::from(&base_dir).join(file_hash.to_string());

        let ext_hint_path = format!("ext-{}", file_hash);
        let ext_hint_file = PathBuf::from(&base_dir).join(&ext_hint_path);
        let check = ext_hint_file.exists();

        if check {
            let file = File::open(&ext_hint_file).await.unwrap();
            let mut reader = BufReader::new(file);
            let mut ext = String::new();
            reader.read_line(&mut ext).await?;
            let file_path_with_ext = if ext.trim() == "None" {
                file_path.clone()
            } else {
                file_path.with_extension(ext.trim())
            };
            NamedFile::open(file_path_with_ext).map_err(|e| {
                eprintln!("{}", e);
                error::ErrorInternalServerError(format!("Error: {}", e))
            })
        } else {
            let mut download = File::create(&file_path).await.unwrap();

            let res = data.http_client.request(Method::GET, url).send().await.unwrap();

            let headers = res.headers().clone();
            if let Some(content_type) = headers.get(CONTENT_TYPE) {
                if content_type == "image/png" && file_type == "image/apng" {
                } else if file_type != content_type.to_str().unwrap() {
                    return Err(error::ErrorBadGateway("content-type != :failed".to_string()));
                }
            }

            let mut stream = res.bytes_stream();

            while let Some(chunk_result) = stream.next().await {
                let chunk = chunk_result.unwrap();
                download.write_all(&chunk).await.unwrap();
            }

            download.flush().await.unwrap();

            let ext = mime_guess::get_mime_extensions_str(&*file_type);
            let ext_hint_file_path = PathBuf::from(&base_dir).join(&ext_hint_path);
            let ext_hint_file_path_str = ext_hint_file_path.to_string_lossy().to_string();
            let new_file_path = if let Some(ext) = ext {
                let ext = ext[0];
                let new_file_path_with_ext = format!("{}.{}", file_path.to_string_lossy(), ext);
                fs::rename(&file_path, &new_file_path_with_ext).await.unwrap();
                File::create(&ext_hint_file_path_str).await.unwrap().write_all(ext.as_bytes()).await.unwrap();
                new_file_path_with_ext
            } else {
                File::create(&ext_hint_file_path_str).await.unwrap().write_all("None".as_bytes()).await.unwrap();
                file_path.to_string_lossy().to_string()
            };

            NamedFile::open(new_file_path).map_err(|e| {
                eprintln!("{}", e);
                error::ErrorInternalServerError(format!("Error: {}", e))
            })
        }
    } else {
        return Err(error::ErrorBadGateway("failed".to_string()));
    };
}

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    tracing_subscriber::fmt()
        .with_max_level(Level::INFO)
        .init();

    let app_state = AppState::new().await;

    HttpServer::new(move || {
        App::new()
            .service(detail)
            .app_data(Data::new(app_state.clone()))
    })
        .bind(("0.0.0.0", 8080))?
        .run()
        .await
}
