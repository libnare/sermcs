use actix_web::{App, get, HttpResponse, HttpServer, web};

#[get("/{id}")]
async fn user_detail(key: web::Path<(String,)>) -> HttpResponse {
    HttpResponse::Ok().body(format!("detect: {}", key.into_inner().0))
}

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    HttpServer::new(|| {
        App::new().service(
            web::scope("/*")
                .service(user_detail),
        )
    })
        .bind(("127.0.0.1", 8080))?
        .run()
        .await
}