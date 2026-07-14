use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

use base64::Engine as _;
use camino::{Utf8Path, Utf8PathBuf};
use serde::Deserialize;
use tracing::debug;

use crate::{
    cache::{Cache, CacheMap, Cacheable},
    markdown::Page,
    sass::Stylesheet,
    url::UrlPath,
};

// No dependency on the webserver

pub struct App {
    root: Utf8PathBuf,
    config: Cache<Arc<Config>>,
    pages: CacheMap<Arc<Page>>,
    templates: CacheMap<Arc<Template>>,
    styles: CacheMap<Arc<Stylesheet>>,
}

impl App {
    pub fn new(root: Utf8PathBuf) -> Self {
        App {
            config: Cache::new(root.join("_config.toml")),
            root,
            pages: CacheMap::default(),
            templates: CacheMap::default(),
            styles: CacheMap::default(),
        }
    }

    pub fn root(&self) -> &Utf8Path {
        &self.root
    }

    // Load the config once at startup so problems show up in the log.
    // Missing or invalid config is non-fatal: requests get 503 until it is
    // valid (see `web`), and the server recovers once the file appears.
    pub async fn check_config(&self) -> anyhow::Result<()> {
        self.config.load().await.map_err(|(_, err)| err)?;
        Ok(())
    }
}

// A page-layout template, cached as raw Handlebars source.
#[derive(Clone, Default)]
struct Template(String);

impl Cacheable for Template {
    fn compute(src: &str) -> anyhow::Result<Self> {
        Ok(Template(src.to_owned()))
    }
}

#[derive(Debug, Default)]
struct Config {
    extensions: HashSet<String>,
    // Path prefix -> users allowed to access it (HTTP Basic auth).
    protected: HashMap<String, Vec<String>>,
    // Plain-text credentials (user -> password).
    users: HashMap<String, String>,
}

// Raw file types served directly when `_config.toml` omits `extensions`.
fn default_extensions() -> Vec<String> {
    [
        "png", "jpg", "jpeg", "gif", "svg", "webp", "avif", "ico", "pdf", "txt", "woff", "woff2",
        "ttf", "otf",
    ]
    .into_iter()
    .map(String::from)
    .collect()
}

#[derive(Deserialize, Default)]
struct ConfigFile {
    #[serde(default = "default_extensions")]
    extensions: Vec<String>,
    #[serde(default)]
    protected: HashMap<String, Vec<String>>,
    #[serde(default)]
    users: HashMap<String, String>,
}

impl Cacheable for Config {
    fn compute(src: &str) -> anyhow::Result<Self> {
        let cf: ConfigFile = toml::from_str(src)?;
        Ok(Config {
            extensions: cf.extensions.into_iter().collect(),
            protected: cf.protected,
            users: cf.users,
        })
    }
}

#[allow(clippy::upper_case_acronyms)]
pub enum MyRequest<'a> {
    GET {
        path: &'a str,
        authorization: Option<&'a str>,
    },
}

pub enum MyResponse {
    Html(String),
    Css(String),
    File(Utf8PathBuf),
    Redirect(String),
}

#[derive(Debug)]
pub enum MyError {
    NotFound,
    Unauthorized,
    Unavailable,
    InvalidPage,
    InvalidScss,
    CannotRead,
    Internal(String),
}

pub type MyResult = Result<MyResponse, MyError>;

pub async fn web(app: Arc<App>, req: MyRequest<'_>) -> MyResult {
    let MyRequest::GET {
        path,
        authorization,
    } = req;
    debug!("GET {path}");
    let url = UrlPath::new(path).ok_or(MyError::NotFound)?;

    // A missing or invalid `_config.toml` -> 503, rather than serving a
    // misconfigured site. The cache logs the underlying error.
    let config = match app.config.load().await {
        Ok(cfg) => cfg,
        Err(_) => return Err(MyError::Unavailable),
    };

    if !authorized(&config, url.path(), authorization) {
        return Err(MyError::Unauthorized);
    }

    if url.has_final_slash() {
        let html = render_page(&app, url).await?;
        return Ok(MyResponse::Html(html));
    }

    if let Some(name) = url.path().strip_prefix('/').filter(|p| !p.contains('/')) {
        if let Some(stem) = name.strip_suffix(".css") {
            if valid_asset_name(stem) {
                let scss_path = app.root.join(format!("_style/{stem}.scss"));
                // Don't create cache entries for missing stylesheets.
                if !tokio::fs::try_exists(&scss_path).await.unwrap_or(false) {
                    return Err(MyError::NotFound);
                }
                let css = match app.styles.load(&scss_path).await {
                    Ok(css) => css,
                    Err(_) => return Err(MyError::InvalidScss),
                };
                return Ok(MyResponse::Css(css.css().to_owned()));
            }
        }
    }

    match url.extension() {
        Some(ext) if config.extensions.contains(ext) => {
            return Ok(MyResponse::File(app.root.join(url.relative_path())));
        }
        _ => (),
    }

    if tokio::fs::try_exists(app.root.join(format!("{}/page.md", url.relative_path())))
        .await
        .unwrap_or(false)
    {
        return Ok(MyResponse::Redirect(format!("{}/", url.path())));
    }

    Err(MyError::NotFound)
}

async fn render_page(app: &App, url: UrlPath<'_>) -> Result<String, MyError> {
    let page_path = app.root.join(format!("{}page.md", url.relative_path()));
    // Don't create cache entries for missing pages.
    if !tokio::fs::try_exists(&page_path).await.unwrap_or(false) {
        return Err(MyError::NotFound);
    }
    let page = match app.pages.load(&page_path).await {
        Ok(page) => page,
        // The file exists (checked above), so a load failure is a bad page.
        Err(_) => return Err(MyError::InvalidPage),
    };

    let template = page.template();
    if !valid_asset_name(template) {
        return Err(MyError::NotFound);
    }
    let tpl_path = app.root.join(format!("_style/{template}.html"));
    let tpl = match app.templates.load(&tpl_path).await {
        Ok(tpl) => tpl,
        Err(_) => return Err(MyError::CannotRead),
    };

    let hbs = handlebars::Handlebars::new();
    let html = hbs
        .render_template(&tpl.0, page.fields())
        .map_err(|_| MyError::Internal("invalid template".into()))?;

    Ok(html)
}

// Frontmatter/URL supplied names must be bare identifiers, no path traversal.
fn valid_asset_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

fn prefix_matches(prefix: &str, path: &str) -> bool {
    let prefix = prefix.strip_suffix('/').unwrap_or(prefix);
    path == prefix || path.starts_with(&format!("{prefix}/"))
}

// Users allowed at `path`, or None when the path is not protected.
// The most specific (longest) matching prefix wins.
fn allowed_users<'a>(config: &'a Config, path: &str) -> Option<&'a [String]> {
    config
        .protected
        .iter()
        .filter(|(prefix, _)| prefix_matches(prefix, path))
        .max_by_key(|(prefix, _)| prefix.len())
        .map(|(_, users)| users.as_slice())
}

// Decode a `Basic <base64>` header into (user, password).
fn parse_basic(header: &str) -> Option<(String, String)> {
    let (scheme, rest) = header.split_once(' ')?;
    if !scheme.eq_ignore_ascii_case("basic") {
        return None;
    }
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(rest.trim())
        .ok()?;
    let text = String::from_utf8(decoded).ok()?;
    let (user, pass) = text.split_once(':')?;
    Some((user.to_owned(), pass.to_owned()))
}

// Access is allowed unless the path is protected and the credentials name an
// allowed user with the correct password.
fn authorized(config: &Config, path: &str, authorization: Option<&str>) -> bool {
    use subtle::ConstantTimeEq;
    let Some(allowed) = allowed_users(config, path) else {
        return true;
    };
    let Some((user, pass)) = authorization.and_then(parse_basic) else {
        return false;
    };
    allowed.iter().any(|u| u == &user)
        && config
            .users
            .get(&user)
            .is_some_and(|p| p.as_bytes().ct_eq(pass.as_bytes()).into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn asset_names() {
        assert!(valid_asset_name("default"));
        assert!(valid_asset_name("a-b_c"));
        assert!(!valid_asset_name(""));
        assert!(!valid_asset_name(".."));
        assert!(!valid_asset_name("a/b"));
        assert!(!valid_asset_name("a.b"));
    }

    // Runs against the checked-in `example_site` (cargo test CWD = crate root).
    async fn resp(path: &str) -> MyResult {
        let app = Arc::new(App::new("example_site".into()));
        web(
            app,
            MyRequest::GET {
                path,
                authorization: None,
            },
        )
        .await
    }

    #[test]
    fn basic_auth() {
        let users = HashMap::from([
            ("user1".to_string(), "pw1".to_string()),
            ("user2".to_string(), "pw2".to_string()),
        ]);
        let protected = HashMap::from([
            ("/foo".to_string(), vec!["user1".to_string()]),
            ("/bar".to_string(), vec!["user2".to_string()]),
            (
                "/quz".to_string(),
                vec!["user1".to_string(), "user2".to_string()],
            ),
        ]);
        let config = Config {
            extensions: HashSet::new(),
            protected,
            users,
        };
        // base64 of "user1:pw1" and "user2:pw2".
        let u1 = Some("Basic dXNlcjE6cHcx");
        let u2 = Some("Basic dXNlcjI6cHcy");

        // Unprotected paths are always allowed.
        assert!(authorized(&config, "/public", None));
        // "/foo" (a prefix of "/foobar") must not leak access.
        assert!(authorized(&config, "/foobar", None));

        // /foo: only user1.
        assert!(authorized(&config, "/foo", u1));
        assert!(authorized(&config, "/foo/x", u1));
        assert!(!authorized(&config, "/foo", u2));
        assert!(!authorized(&config, "/foo", None));

        // /bar: only user2.
        assert!(authorized(&config, "/bar/x", u2));
        assert!(!authorized(&config, "/bar", u1));

        // /quz: either user.
        assert!(authorized(&config, "/quz", u1));
        assert!(authorized(&config, "/quz", u2));

        // Right user, wrong password ("user1:wrong") -> denied.
        assert!(!authorized(&config, "/quz", Some("Basic dXNlcjE6d3Jvbmc=")));
    }

    #[tokio::test]
    async fn renders_home() {
        match resp("/").await.unwrap() {
            MyResponse::Html(h) => assert!(h.contains("Hello")),
            _ => panic!("expected html"),
        }
    }

    #[tokio::test]
    async fn renders_per_page_template() {
        match resp("/about/").await.unwrap() {
            MyResponse::Html(h) => assert!(h.contains("wide")),
            _ => panic!("expected html"),
        }
    }

    #[tokio::test]
    async fn redirects_without_slash() {
        match resp("/page1").await.unwrap() {
            MyResponse::Redirect(loc) => assert_eq!(loc, "/page1/"),
            _ => panic!("expected redirect"),
        }
    }

    #[tokio::test]
    async fn compiles_css() {
        match resp("/default.css").await.unwrap() {
            MyResponse::Css(c) => assert!(c.contains("color")),
            _ => panic!("expected css"),
        }
    }

    #[tokio::test]
    async fn missing_is_not_found() {
        assert!(matches!(resp("/nope/").await, Err(MyError::NotFound)));
    }
}
