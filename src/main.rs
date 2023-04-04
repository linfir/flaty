use std::{
    net::SocketAddr,
    path::PathBuf,
    sync::{Arc, Mutex},
};

use anyhow::Context;
use axum::{
    extract::State,
    response::{IntoResponse, Response},
    Router,
};
use clap::Parser;
use tracing::log::info;

struct App {
    root: PathBuf,
    counter: Mutex<u64>,
}

#[derive(Parser)]
struct Args {
    #[arg(default_value_t = {"127.0.0.1:8080".into()})]
    listen: String,
    #[arg(default_value_t = {".".into()})]
    root: String,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt().init();

    let args = Args::parse();
    let address: SocketAddr = args.listen.parse().context("invalid listen address")?;
    let root = PathBuf::from(args.root);

    let app = Arc::new(App {
        root,
        counter: Mutex::new(0),
    });

    let router = Router::new().fallback(handler).with_state(app);

    info!("Listening on http://{}/", &address);
    axum::Server::bind(&address)
        .serve(router.into_make_service())
        .await?;

    Ok(())
}

async fn handler(State(app): State<Arc<App>>) -> Response {
    let n = {
        let mut lock = app.counter.lock().unwrap();
        *lock += 1;
        *lock
    };
    format!(
        "Hello, world from `{}`! This is try #{}",
        app.root.display(),
        n
    )
    .into_response()
}
