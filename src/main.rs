use std::net::SocketAddr;

use axum::{
    response::{IntoResponse, Response},
    Router,
};
use tracing::log::info;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt().init();

    let address: SocketAddr = "127.0.0.1:8080".parse().unwrap();

    let app = Router::new().fallback(handler);

    info!("Listening on http://{}", address);
    axum::Server::bind(&address)
        .serve(app.into_make_service())
        .await
        .unwrap();
}

async fn handler() -> Response {
    "Hello, world!".into_response()
}
