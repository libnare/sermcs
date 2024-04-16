use std::io::Write;
use std::str::FromStr;

use actix_web::{App, get, HttpResponse, HttpServer, web};
use actix_web::http::header::{HeaderName, HeaderValue};
use actix_web::http::StatusCode;
use actix_web::web::Data;
use reqwest::header::{CONTENT_DISPOSITION, CONTENT_TYPE};
use reqwest::Method;
use sqlx::Row;
use tempfile::NamedTempFile;
use tokio::process::Command;
use tracing::{error, Level};
use zerocopy::IntoBytes;

use sermcs::AppState;

const VF_THUMBNAIL_VIDEO: &str = "select=eq(n\\,34),scale='if(gt(iw,ih),min(498\\, iw),-1)':'if(gt(iw,ih),-1,min(422\\, ih))',format=rgba";
const VF_THUMBNAIL_ANIMATED_IMAGE: &str = "scale='if(gt(iw,ih),min(374\\, iw),-1)':'if(gt(iw,ih),-1,min(317\\, ih))',format=rgba";
const VF_THUMBNAIL_IMAGE: &str = "scale='if(gt(iw,ih),min(498\\, iw),-1)':'if(gt(iw,ih),-1,min(422\\, ih))',format=rgba";

const KEY_QUERY: &str = r#"
    SELECT url, type, "accessKey", "thumbnailAccessKey"
    FROM public.drive_file
    WHERE "accessKey" = $1 OR "thumbnailAccessKey" = $1
    "#;

async fn get_thumbnail_image(input_file_path: &str, content_type: &str, method: &str) -> HttpResponse {
    let output_file = NamedTempFile::new().unwrap();

    let ct;
    let status_code;

    let image_format = content_type.split('/').nth(1).unwrap_or("");

    let output_file_str = output_file.path().to_string_lossy();

    let mut ffmpeg_args = vec![
        "-y",
        "-i", &input_file_path,
        "-f", &image_format,
    ];

    match method {
        "video" => {
            ffmpeg_args.extend(&["-vf", VF_THUMBNAIL_VIDEO]);
            ffmpeg_args.extend(&["-vframes", "1"]);
        },
        "animated" => {
            ffmpeg_args.extend(&["-vf", VF_THUMBNAIL_ANIMATED_IMAGE]);
            ffmpeg_args.extend(&["-loop", "0"]);
        },
        "image" => {
            ffmpeg_args.extend(&["-vf", VF_THUMBNAIL_IMAGE]);
        },
        _ => {}
    }

    ffmpeg_args.push(&output_file_str);

    let output = Command::new("ffmpeg")
        .args(&ffmpeg_args)
        .output()
        .await;

    match output {
        Ok(_) => {
            status_code = StatusCode::OK;
            ct = content_type
        }
        Err(_) => {
            status_code = StatusCode::INTERNAL_SERVER_ERROR;
            ct = "text/plain";
        }
    }

    let content = web::block(move || std::fs::read(output_file.path())).await.unwrap().unwrap();

    HttpResponse::build(status_code)
        .content_type(ct)
        .body(content)
}

#[get("/{tail:.*}")]
async fn detail(key: web::Path<(String, )>, data: Data<AppState>) -> HttpResponse {
    let key = key.into_inner().0;

    let rows = match sqlx::query(KEY_QUERY)
        .bind(&*key)
        .fetch_one(&data.db_pool)
        .await {
        Ok(rows) => rows,
        Err(e) => {
            error!("Query failed: {e}");
            return HttpResponse::new(StatusCode::BAD_GATEWAY);
        }
    };

    let url: String = rows.get("url");
    let file_type: String = rows.get("type");
    let access_key: String = rows.get("accessKey");
    let thumbnail_access_key: String = rows.get("thumbnailAccessKey");

    let is_access = access_key == &*key;
    let is_thumbnail = thumbnail_access_key == key;

    let res = match data.http_client.request(Method::GET, url).send().await {
        Ok(res) => res,
        Err(_) => return HttpResponse::new(StatusCode::BAD_GATEWAY),
    };

    let headers = res.headers().clone();

    if let Some(content_type) = headers.get(CONTENT_TYPE) {
        if file_type != content_type.to_str().unwrap() {
            return HttpResponse::new(StatusCode::INTERNAL_SERVER_ERROR);
        }
    }

    return if is_access {
        let mut http_res = HttpResponse::build(StatusCode::from_u16(res.status().as_u16()).unwrap()).body(res.bytes().await.unwrap());

        if let Some(content_type) = headers.get(CONTENT_TYPE) {
            if file_type != content_type.to_str().unwrap() {
                return HttpResponse::new(StatusCode::BAD_GATEWAY);
            }
            http_res.headers_mut().insert(
                HeaderName::from_str(CONTENT_TYPE.as_str()).unwrap(),
                HeaderValue::from_str(content_type.to_str().unwrap()).unwrap(),
            );
        }
        if let Some(content_disposition) = headers.get(CONTENT_DISPOSITION) {
            http_res.headers_mut().insert(
                HeaderName::from_str(CONTENT_DISPOSITION.as_str()).unwrap(),
                HeaderValue::from_str(content_disposition.to_str().unwrap()).unwrap(),
            );
        }

        http_res
    } else if is_thumbnail {
        let res: web::Bytes = res.bytes().await.unwrap();

        let mut input_file = NamedTempFile::new().unwrap();
        input_file.write(&*res.as_bytes()).unwrap();

        return if file_type.starts_with("video/") {
            get_thumbnail_image(input_file.path().to_str().unwrap(), "image/avif", "video").await
        } else if file_type == "image/apng" {
            get_thumbnail_image(input_file.path().to_str().unwrap(), "image/webp", "animated").await
        } else if file_type == "image/gif" {
            get_thumbnail_image(input_file.path().to_str().unwrap(), "image/webp", "animated").await
        } else {
            get_thumbnail_image(input_file.path().to_str().unwrap(), "image/avif", "image").await
        }
    } else {
        return HttpResponse::new(StatusCode::BAD_GATEWAY);
    }
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
