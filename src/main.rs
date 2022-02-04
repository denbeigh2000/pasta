use std::env::args;
use std::net::SocketAddr;
use std::string::FromUtf8Error;

use axum::extract::{Extension, Path};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{AddExtensionLayer, Router, body};
use bb8::{Pool, RunError};
use bb8_redis::RedisConnectionManager;
use rand::distributions::Alphanumeric;
use rand::{thread_rng, Rng};
use redis::{AsyncCommands, RedisError};
use thiserror::Error;

const EXPIRY_SECS: usize = 60 * 30;

#[tokio::main]
async fn main() {
    let addr = args()
        .nth(1)
        .unwrap_or_else(|| "redis://localhost:6379".to_string());

    let manager = RedisConnectionManager::new(addr).unwrap();
    let pool = Pool::builder().build(manager).await.unwrap();

    let app: Router<_> = Router::new()
        .route("/paste/:key", get(get_paste))
        .route("/paste", post(create_paste))
        .layer(AddExtensionLayer::new(pool));

    let addr = SocketAddr::from(([127, 0, 0, 1], 3000));
    axum::Server::bind(&addr)
        .serve(app.into_make_service())
        .await
        .unwrap();
}

#[derive(Debug, Error)]
pub enum PastaError {
    #[error("paste not found: {0}")]
    NotFound(String),
    #[error("error communicating with redis")]
    RedisError(#[from] RedisError),
    #[error("connection timeout")]
    ConnectionTimeout,
    #[error("string decode error")]
    PasteDecodeError(#[from] FromUtf8Error),
}

impl From<RunError<RedisError>> for PastaError {
    fn from(e: RunError<RedisError>) -> Self {
        match e {
            RunError::User(e) => PastaError::RedisError(e),
            RunError::TimedOut => PastaError::ConnectionTimeout,
        }
    }
}

impl IntoResponse for PastaError {
    fn into_response(self) -> Response {
        let (status, body) = match self {
            PastaError::NotFound(key) => {
                let body = body::boxed(body::Full::from(format!("not found: {key}")));

                (StatusCode::NOT_FOUND, body)
            },
            PastaError::RedisError(e) => {
                eprintln!("redis error: {e}");

                (StatusCode::INTERNAL_SERVER_ERROR, body::boxed(body::Empty::new()))
            },
            PastaError::ConnectionTimeout => {
                eprintln!("redis connection timeout");

                (StatusCode::INTERNAL_SERVER_ERROR, body::boxed(body::Empty::new()))
            },
            PastaError::PasteDecodeError(e) => {
                eprintln!("decoding error: {e}");

                (StatusCode::BAD_REQUEST, body::boxed(body::Empty::new()))
            },
        };

        Response::builder()
            .status(status)
            .body(body).unwrap()
    }
}

async fn get_paste(
    Path(key): Path<String>,
    Extension(pool): Extension<Pool<RedisConnectionManager>>,
) -> Result<String, PastaError> {
    let mut conn = pool.get().await?;

    let redis_key = format!("pasta:{key}");
    let value: Option<String> = redis::cmd("GETDEL")
        .arg(redis_key)
        .query_async(&mut *conn)
        .await?;

    value.ok_or(PastaError::NotFound(key))
}

async fn create_paste(
    paste: String,
    Extension(pool): Extension<Pool<RedisConnectionManager>>,
) -> Result<String, PastaError> {
    let mut conn = pool.get().await?;

    let key = thread_rng()
        .sample_iter(&Alphanumeric)
        .take(8)
        .map(char::from)
        .collect::<String>();
    let redis_key = format!("pasta:{key}");
    let _: () = conn.set_ex(redis_key, &paste, EXPIRY_SECS).await?;

    Ok(key)
}
