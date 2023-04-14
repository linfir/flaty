use std::sync::Arc;

use camino::{Utf8Path, Utf8PathBuf};
use serde::Deserialize;
use tracing::{debug, warn};

use crate::{
    cache::{Cachable, Cache},
    markdown::markdown,
    sass::sass,
};

// No dependency on the webserver

pub struct App {
    config: Cache<Arc<Config>>,
}

impl App {
    pub fn new() -> Self {
        App {
            config: Cache::new("_config.toml".into(), Arc::new(Config::default())),
        }
    }
}

#[derive(Debug, Default, Deserialize)]
struct Config {
    allowed_extensions: Vec<String>,
}

impl Cachable for Config {
    fn recompute(src: &str) -> anyhow::Result<Self> {
        Ok(toml::from_str(src)?)
    }
}

#[derive(Debug)]
pub enum MyRequest<'a> {
    Get(&'a str),
}

pub enum MyResponse {
    Html(String),
    Css(String),
    File(Utf8PathBuf),
    Redirect(String),
}

pub enum MyError {
    NotFound,
    InvalidScss,
    CannotRead(Utf8PathBuf),
    Internal(String),
}

pub type MyResult = Result<MyResponse, MyError>;

pub async fn web(app: Arc<App>, req: MyRequest<'_>) -> MyResult {
    debug!("Request: {:?}", req);
    let MyRequest::Get(uri_path) = req;

    let _ends_with_slash = uri_path.ends_with('/');
    let components = to_components(uri_path).ok_or(MyError::NotFound)?;

    for c in &components {
        if c.is_empty() || c.starts_with('.') || c.starts_with('_') {
            return Err(MyError::NotFound);
        }
    }
    if let Some(_c) = components.last() {}

    // Reloads config
    let config = match app.config.reload().await {
        Ok(cfg) => cfg,
        Err((cfg, err)) => {
            warn!("Error reloading `{}`: {}", app.config.path(), err);
            cfg
        }
    };
    debug!("Extensions: {:?}", config.allowed_extensions);

    if !uri_path.starts_with('/') {
        Err(MyError::NotFound)
    } else if uri_path == "/default.css" {
        let doc = slurp("_style/default.scss").await?;
        let css = sass(doc).await?;
        Ok(MyResponse::Css(css))
    } else if uri_path == "/heart.svg" {
        Ok(MyResponse::File("heart.svg".into()))
    } else if uri_path == "/zero" {
        Ok(MyResponse::File("zero".into()))
    } else if uri_path.ends_with('/') {
        let doc = slurp(&format!("{}page.md", &uri_path[1..])).await?;
        let md = markdown(&doc).map_err(|_| MyError::NotFound)?;

        let tpl = slurp("_style/default.html").await?;
        let hbs = handlebars::Handlebars::new();
        let html = hbs
            .render_template(&tpl, &md)
            .map_err(|_| MyError::Internal("invalid template".into()))?;

        Ok(MyResponse::Html(html))
    } else if uri_path == "/page1" {
        Ok(MyResponse::Redirect("/page1/".into()))
    } else {
        Err(MyError::NotFound)
    }
}

fn to_components(url: &str) -> Option<Vec<&str>> {
    if url.contains("//") {
        return None;
    }
    let url = url.strip_prefix('/')?;
    if url.is_empty() {
        return Some(Vec::new());
    }
    let url = url.strip_suffix('/').unwrap_or(url);
    let v = url.split('/').collect();
    Some(v)
}

#[test]
fn test_to_components() {
    assert_eq!(to_components(""), None);
    assert_eq!(to_components("bla"), None);
    assert_eq!(to_components("/"), Some(vec![]));
    assert_eq!(to_components("/bla"), Some(vec!["bla"]));
    assert_eq!(to_components("/bla/"), Some(vec!["bla"]));
    assert_eq!(to_components("/bla/blo"), Some(vec!["bla", "blo"]));
    assert_eq!(to_components("/bla/blo/"), Some(vec!["bla", "blo"]));
}

pub async fn slurp(path: impl AsRef<Utf8Path>) -> Result<String, MyError> {
    let path = path.as_ref();
    tokio::fs::read_to_string(path)
        .await
        .map_err(|_| MyError::CannotRead(path.to_owned()))
}
