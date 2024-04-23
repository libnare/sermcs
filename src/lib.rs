use std::env;
use std::process::exit;
use lazy_static::lazy_static;
use reqwest::Client as HttpClient;
use sqlx::{Pool, Postgres};
use sqlx::postgres::PgPoolOptions;
use tracing::{error, info};

lazy_static! {
    static ref HTTP_CLIENT: HttpClient = {
        HttpClient::new()
    };
}

#[derive(Debug, Clone)]
pub struct AppState {
    pub db_pool: Pool<Postgres>,
    pub http_client: HttpClient,
    pub temp_dir: String,
}

#[derive(Debug, Clone)]
pub struct Bind {
    pub host: Option<String>,
    pub port: Option<u16>,
}

#[derive(Debug, Clone)]
pub struct DbConfig {
    pub host: String,
    pub port: u16,
    pub user: String,
    pub password: String,
    pub database: String,
}

impl Default for Bind {
    fn default() -> Self {
        Self {
            host: env::var("HOST").unwrap_or_else(|_| "0.0.0.0".to_string()).into(),
            port: env::var("PORT").unwrap_or_else(|_| "8080".to_string()).parse().ok(),
        }
    }
}

impl AppState {
    pub async fn new() -> Self {
        let db_config = DbConfig {
            host: env::var("DB_HOST").unwrap(),
            port: env::var("DB_PORT").unwrap().parse().unwrap(),
            user: env::var("DB_USER").unwrap(),
            password: env::var("DB_PASSWORD").unwrap(),
            database: env::var("DB_NAME").unwrap(),
        };
        let pool = match PgPoolOptions::new()
            .max_connections(5)
            .connect(&format!(
                "postgres://{}:{}@{}:{}/{}",
                db_config.user, db_config.password, db_config.host, db_config.port, db_config.database
            )).await {
            Ok(pool) => pool,
            Err(e) => {
                error!("Error occurred while connecting to database: {}", e);
                exit(1)
            },
        };

        let row: (String, ) = sqlx::query_as("SELECT version()")
            .fetch_one(&pool).await.unwrap();

        let version = row.0;

        info!("Connected to PostgreSQL: {}", version);

        Self {
            db_pool: pool,
            http_client: HTTP_CLIENT.clone(),
            temp_dir: env::var("SERMCS_TEMP_DIR").unwrap(),
        }
    }
}