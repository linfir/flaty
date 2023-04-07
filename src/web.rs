use std::{
    borrow::Cow,
    path::{Path, PathBuf},
    sync::Arc,
};

use tracing::debug;

use crate::{markdown::markdown, sass::sass};

// No dependency on Hyper or Axum

pub struct App {
    root: PathBuf,
}

impl App {
    pub fn new(root: PathBuf) -> Self {
        App { root }
    }
}

#[derive(Debug)]
pub enum MyRequest<'a> {
    Get(&'a str),
}

pub enum MyResponse {
    Html(String),
    Css(String),
    File(PathBuf),
}

pub enum MyError {
    NotFound,
    InvalidScss,
    CannotRead(PathBuf),
    Internal(Cow<'static, str>),
}

pub type MyResult = Result<MyResponse, MyError>;

pub async fn web(app: Arc<App>, req: MyRequest<'_>) -> MyResult {
    debug!("Request: {req:?}");
    let MyRequest::Get(uri_path) = req;

    let ends_with_slash = uri_path.ends_with('/');
    let p = to_components(uri_path).ok_or(MyError::NotFound)?;

    if !uri_path.starts_with('/') {
        Err(MyError::NotFound)
    } else if uri_path == "/default.css" {
        let doc = slurp(app.root.join("_style/default.scss")).await?;
        let css = sass(doc).await?;
        Ok(MyResponse::Css(css))
    } else if uri_path == "/heart.svg" {
        Ok(MyResponse::File(app.root.join("heart.svg")))
    } else if uri_path.ends_with('/') {
        let doc = slurp(app.root.join(&format!("{}page.md", &uri_path[1..]))).await?;
        let md = markdown(&doc).map_err(|_| MyError::NotFound)?;

        let tpl = slurp(app.root.join("_style/default.html")).await?;
        let hbs = handlebars::Handlebars::new();
        let html = hbs
            .render_template(&tpl, &md)
            .map_err(|_| MyError::Internal("invalid template".into()))?;

        Ok(MyResponse::Html(html))
    } else {
        Err(MyError::NotFound)
    }
}

fn to_components(url: &str) -> Option<Vec<&str>> {
    if url.find("//").is_some() {
        return None;
    }
    Some(
        url.strip_prefix('/')?
            .strip_suffix('/')
            .unwrap_or(url)
            .split('/')
            .collect(),
    )
}

pub async fn slurp(path: impl AsRef<Path>) -> Result<String, MyError> {
    let path = path.as_ref();
    tokio::fs::read_to_string(path)
        .await
        .map_err(|_| MyError::CannotRead(path.to_owned()))
}
