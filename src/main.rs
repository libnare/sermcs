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
use tracing::{error, info, Level};
use zerocopy::IntoBytes;

use sermcs::AppState;

const VF_THUMBNAIL_VIDEO: &str = "select=eq(n\\,34),scale='if(gt(iw,ih),min(498\\, iw),-1)':'if(gt(iw,ih),-1,min(422\\, ih))',format=rgba";
const VF_THUMBNAIL_ANIMATED_IMAGE: &str = "scale='if(gt(iw,ih),min(374\\, iw),-1)':'if(gt(iw,ih),-1,min(317\\, ih))',format=rgba";
const VF_THUMBNAIL_IMAGE: &str = "scale='if(gt(iw,ih),min(498\\, iw),-1)':'if(gt(iw,ih),-1,min(422\\, ih))',format=rgba";

#[get("/{tail:.*}")]
async fn detail(key: web::Path<(String, )>, data: Data<AppState>) -> HttpResponse {
    let key = key.into_inner().0;

    let query = r#"
        SELECT t.url, t.type, t."accessKey", t."thumbnailAccessKey"
        FROM public.drive_file t
        WHERE "accessKey" = $1 OR "thumbnailAccessKey" = $1
    "#;
    let rows = match sqlx::query(query)
        .bind(&*key)
        .fetch_one(&data.db_pool)
        .await {
        Ok(rows) => rows,
        Err(e) => {
            error!("Query failed: {e}");
            return HttpResponse::new(StatusCode::from_u16(502).unwrap());
        }
    };

    let url: String = rows.get("url");
    let file_type: String = rows.get("type");
    let access_key: String = rows.get("accessKey");
    let thumbnail_access_key: String = rows.get("thumbnailAccessKey");

    let is_access = access_key == &*key;
    let is_thumbnail = thumbnail_access_key == key;

    let res = data.http_client.request(Method::GET, url).send().await.unwrap();
    let headers = res.headers().clone();
    return if is_access {
        let mut http_res = HttpResponse::build(StatusCode::from_u16(res.status().as_u16()).unwrap()).body(res.bytes().await.unwrap());
        if let Some(content_type) = headers.get(CONTENT_TYPE) {
            if file_type != content_type.to_str().unwrap() {
                return HttpResponse::new(StatusCode::from_u16(502).unwrap());
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
        let status_code = res.status().as_u16();
        let res: web::Bytes = res.bytes().await.unwrap();

        let mut input_file = NamedTempFile::new().unwrap();
        input_file.write(&*res.as_bytes()).unwrap();

        if file_type.starts_with("video/") {
            let output_file = NamedTempFile::new().unwrap();

            info!("{}", output_file.path().to_str().unwrap());

            Command::new("ffmpeg")
                .args(["-y",
                    "-i", input_file.path().to_str().unwrap(),
                    "-vf", VF_THUMBNAIL_VIDEO,
                    "-vframes", "1",
                    "-f", "avif",
                    output_file.path().to_str().unwrap()])
                .output()
                .await
                .expect("failed to ffmpeg process");

            let content = web::block(move || std::fs::read(output_file.path())).await.unwrap().unwrap();

            let mut http_res = HttpResponse::build(StatusCode::from_u16(status_code).unwrap())
                .content_type("image/avif")
                .body(content);
            if let Some(content_type) = headers.get(CONTENT_TYPE) {
                if file_type != content_type.to_str().unwrap() {
                    return HttpResponse::new(StatusCode::from_u16(502).unwrap());
                }
            }
            if let Some(content_disposition) = headers.get(CONTENT_DISPOSITION) {
                http_res.headers_mut().insert(
                    HeaderName::from_str(CONTENT_DISPOSITION.as_str()).unwrap(),
                    HeaderValue::from_str(content_disposition.to_str().unwrap()).unwrap(),
                );
            }

            return http_res;
        } else if file_type == "image/apng" {
            let output_file = NamedTempFile::new().unwrap();

            info!("{}", output_file.path().to_str().unwrap());

            Command::new("ffmpeg")
                .args(["-y",
                    "-i", input_file.path().to_str().unwrap(),
                    "-vf", VF_THUMBNAIL_ANIMATED_IMAGE,
                    "-f", "webp",
                    output_file.path().to_str().unwrap()])
                .output()
                .await
                .expect("failed to ffmpeg process");

            let content = web::block(move || std::fs::read(output_file.path())).await.unwrap().unwrap();

            let mut http_res = HttpResponse::build(StatusCode::from_u16(status_code).unwrap())
                .content_type("image/webp")
                .body(content);
            if let Some(content_type) = headers.get(CONTENT_TYPE) {
                if file_type != content_type.to_str().unwrap() {
                    return HttpResponse::new(StatusCode::from_u16(502).unwrap());
                }
            }
            if let Some(content_disposition) = headers.get(CONTENT_DISPOSITION) {
                http_res.headers_mut().insert(
                    HeaderName::from_str(CONTENT_DISPOSITION.as_str()).unwrap(),
                    HeaderValue::from_str(content_disposition.to_str().unwrap()).unwrap(),
                );
            }

            return http_res;
        } else if file_type == "image/gif" {
            let output_file = NamedTempFile::new().unwrap();

            info!("{}", output_file.path().to_str().unwrap());

            Command::new("ffmpeg")
                .args(["-y",
                    "-i", input_file.path().to_str().unwrap(),
                    "-vf", VF_THUMBNAIL_ANIMATED_IMAGE,
                    "-loop", "0",
                    "-f", "webp",
                    output_file.path().to_str().unwrap()])
                .output()
                .await
                .expect("failed to ffmpeg process");

            let content = web::block(move || std::fs::read(output_file.path())).await.unwrap().unwrap();

            let mut http_res = HttpResponse::build(StatusCode::from_u16(status_code).unwrap())
                .content_type("image/webp")
                .body(content);
            if let Some(content_type) = headers.get(CONTENT_TYPE) {
                if file_type != content_type.to_str().unwrap() {
                    return HttpResponse::new(StatusCode::from_u16(502).unwrap());
                }
            }
            if let Some(content_disposition) = headers.get(CONTENT_DISPOSITION) {
                http_res.headers_mut().insert(
                    HeaderName::from_str(CONTENT_DISPOSITION.as_str()).unwrap(),
                    HeaderValue::from_str(content_disposition.to_str().unwrap()).unwrap(),
                );
            }

            return http_res;
        }

        let output_file = NamedTempFile::new().unwrap();

        info!("{}", output_file.path().to_str().unwrap());

        Command::new("ffmpeg")
            .args(["-y",
                "-i", input_file.path().to_str().unwrap(),
                "-vf", VF_THUMBNAIL_IMAGE,
                "-f", "avif",
                output_file.path().to_str().unwrap()])
            .output()
            .await
            .expect("failed to ffmpeg process");

        let content = web::block(move || std::fs::read(output_file.path())).await.unwrap().unwrap();

        let mut http_res = HttpResponse::build(StatusCode::from_u16(status_code).unwrap())
            .content_type("image/avif")
            .body(content);
        if let Some(content_type) = headers.get(CONTENT_TYPE) {
            if file_type != content_type.to_str().unwrap() {
                return HttpResponse::new(StatusCode::from_u16(502).unwrap());
            }
        }
        if let Some(content_disposition) = headers.get(CONTENT_DISPOSITION) {
            http_res.headers_mut().insert(
                HeaderName::from_str(CONTENT_DISPOSITION.as_str()).unwrap(),
                HeaderValue::from_str(content_disposition.to_str().unwrap()).unwrap(),
            );
        }

        http_res
    } else {
        HttpResponse::new(StatusCode::from_u16(502).unwrap())
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
        .bind(("127.0.0.1", 8080))?
        .run()
        .await
}