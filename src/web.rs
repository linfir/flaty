use std::{collections::HashMap, future::Future, pin::Pin, sync::Arc, time::Duration};

use base64::Engine as _;
use camino::{Utf8Path, Utf8PathBuf};
use dashmap::DashMap;
use parking_lot::Mutex;
use serde::Deserialize;
use serde_json::Value as Json;
use tokio::{sync::Mutex as AsyncMutex, time::Instant};
use tracing::{debug, error};

use crate::{
    cache::{Cache, CacheMap, Cacheable},
    markdown::{render_markdown, strip_html_comments, Block, Document, Page},
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
    rendered: RenderedPages,
    last_access: Mutex<Instant>,
}

impl App {
    pub fn new(root: Utf8PathBuf) -> Self {
        App {
            config: Cache::new(root.join("_config.toml")),
            root,
            pages: CacheMap::default(),
            templates: CacheMap::default(),
            styles: CacheMap::default(),
            rendered: RenderedPages::default(),
            last_access: Mutex::new(Instant::now()),
        }
    }

    pub fn root(&self) -> &Utf8Path {
        &self.root
    }

    // Mark this site as just accessed (multi mode uses it to drop idle sites).
    pub fn touch(&self) {
        *self.last_access.lock() = Instant::now();
    }

    pub fn idle_for(&self, now: Instant) -> Duration {
        now.saturating_duration_since(*self.last_access.lock())
    }

    // Drop cache entries idle beyond `ttl`, releasing their memory.
    pub fn sweep(&self, ttl: Duration) {
        self.pages.sweep(ttl);
        self.templates.sweep(ttl);
        self.styles.sweep(ttl);
        self.rendered.sweep(ttl);
    }

    // Load the config once at startup so problems show up in the log.
    // A missing `_config.toml` is fine (treated as empty). An invalid one is
    // non-fatal: requests get 404 until it is valid (see `web`), and the
    // server recovers once the file is fixed.
    pub async fn check_config(&self) -> anyhow::Result<()> {
        self.config.load_optional().await.map_err(|(_, err)| err)?;
        Ok(())
    }
}

// A page-layout or snippet template, cached as raw Handlebars source.
#[derive(Clone, Default)]
struct Template(String);

impl Cacheable for Template {
    fn compute(src: &str) -> anyhow::Result<Self> {
        Ok(Template(src.to_owned()))
    }
}

const MAX_RENDERED_PAGES: usize = 1024;

// Rendered Markdown keyed by the identities of its page and snippet inputs.
struct RenderedPages {
    map: DashMap<Utf8PathBuf, Arc<AsyncMutex<RenderedPage>>>,
    cap: usize,
    #[cfg(test)]
    renders: std::sync::atomic::AtomicUsize,
}

impl Default for RenderedPages {
    fn default() -> Self {
        Self {
            map: DashMap::new(),
            cap: MAX_RENDERED_PAGES,
            #[cfg(test)]
            renders: std::sync::atomic::AtomicUsize::new(0),
        }
    }
}

#[derive(Default)]
struct RenderedPage {
    page: Option<Arc<Page>>,
    snippets: Vec<Arc<Template>>,
    html: String,
    last_access: Option<Instant>,
}

impl RenderedPage {
    fn matches(&self, page: &Arc<Page>, snippets: &[Arc<Template>]) -> bool {
        self.page
            .as_ref()
            .is_some_and(|cached| Arc::ptr_eq(cached, page))
            && self.snippets.len() == snippets.len()
            && self
                .snippets
                .iter()
                .zip(snippets)
                .all(|(cached, current)| Arc::ptr_eq(cached, current))
    }
}

impl RenderedPages {
    async fn load(
        &self,
        path: &Utf8Path,
        root: &Utf8Path,
        page: Arc<Page>,
        snippets: Vec<Arc<Template>>,
    ) -> Result<String, MyError> {
        let entry = self
            .map
            .entry(path.to_owned())
            .or_insert_with(|| Arc::new(AsyncMutex::new(RenderedPage::default())))
            .clone();
        let mut cached = entry.lock().await;
        cached.last_access = Some(Instant::now());

        if cached.matches(&page, &snippets) {
            let html = cached.html.clone();
            drop(cached);
            self.enforce_cap();
            return Ok(html);
        }

        #[cfg(test)]
        self.renders
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);

        let render_root = root.to_owned();
        let render_page = page.clone();
        let render_snippets = snippets.clone();
        let result = tokio::task::spawn_blocking(move || {
            let mut snippets = render_snippets.iter();
            let html = render_document(
                &render_root,
                render_page.body(),
                render_page.fields(),
                &mut snippets,
            )?;
            if snippets.next().is_some() {
                return Err(MyError::Internal(
                    "unused snippet rendering dependency".into(),
                ));
            }
            Ok(html)
        })
        .await
        .map_err(|err| {
            error!("page rendering task failed: {err}");
            MyError::Internal("cannot render page".into())
        })?;

        if let Ok(html) = &result {
            cached.page = Some(page);
            cached.snippets = snippets;
            cached.html = html.clone();
        }
        drop(cached);
        self.enforce_cap();
        result
    }

    fn sweep(&self, ttl: Duration) {
        let now = Instant::now();
        self.map.retain(|_, entry| {
            entry.try_lock().map_or(true, |cached| {
                cached
                    .last_access
                    .is_none_or(|time| now.saturating_duration_since(time) < ttl)
            })
        });
    }

    fn enforce_cap(&self) {
        let excess = self.map.len().saturating_sub(self.cap);
        if excess == 0 {
            return;
        }
        let mut entries: Vec<_> =
            self.map
                .iter()
                .filter_map(|entry| {
                    entry.value().try_lock().ok().and_then(|cached| {
                        cached.last_access.map(|time| (entry.key().clone(), time))
                    })
                })
                .collect();
        entries.sort_by_key(|(_, time)| *time);
        for (key, _) in entries.into_iter().take(excess) {
            self.map.remove(&key);
        }
    }
}

#[derive(Debug, Default)]
struct Config {
    // Path prefix -> users allowed to access it (HTTP Basic auth).
    protected: HashMap<String, Vec<String>>,
    // Plain-text credentials (user -> password).
    users: HashMap<String, String>,
}

#[derive(Deserialize, Default)]
struct ConfigFile {
    #[serde(default)]
    protected: HashMap<String, Vec<String>>,
    #[serde(default)]
    users: HashMap<String, String>,
}

impl Cacheable for Config {
    fn compute(src: &str) -> anyhow::Result<Self> {
        let cf: ConfigFile = toml::from_str(src)?;
        Ok(Config {
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

    // A missing `_config.toml` is treated as empty (an unconfigured site).
    // An invalid one -> 404, rather than serving a misconfigured site; the
    // cache logs the underlying error.
    let config = match app.config.load_optional().await {
        Ok(cfg) => cfg,
        Err(_) => return Err(MyError::NotFound),
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
            // Serve a real `.css` file as-is (falls through to raw file below);
            // only compile `_style/{stem}.scss` when no such file exists.
            let css_path = app.root.join(name);
            let css_exists = tokio::fs::try_exists(&css_path).await.unwrap_or(false);
            if !css_exists && valid_asset_name(stem) {
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

    // Any path with an extension is served as a raw file.
    if url.extension().is_some() {
        return Ok(MyResponse::File(app.root.join(url.relative_path())));
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

    let mut snippets = Vec::new();
    load_snippet_templates(app, page.body(), &mut snippets).await?;
    let contents = app
        .rendered
        .load(&page_path, &app.root, page.clone(), snippets)
        .await?;
    let mut fields = page.fields().clone();
    fields.insert("contents".into(), Json::String(contents));

    let mut hbs = handlebars::Handlebars::new();
    hbs.register_helper("is_empty", Box::new(is_empty));
    let html = hbs
        .render_template(&tpl.0, &fields)
        .map_err(|_| MyError::Internal("invalid template".into()))?;

    Ok(strip_html_comments(&html))
}

fn load_snippet_templates<'a>(
    app: &'a App,
    document: &'a Document,
    templates: &'a mut Vec<Arc<Template>>,
) -> Pin<Box<dyn Future<Output = Result<(), MyError>> + Send + 'a>> {
    Box::pin(async move {
        for block in document.blocks() {
            let Block::Snippet {
                name, body, line, ..
            } = block
            else {
                continue;
            };
            let path = app.root.join(format!("_style/snippets/{name}.html"));
            if !tokio::fs::try_exists(&path).await.unwrap_or(false) {
                error!("missing snippet `{path}` used at line {line}");
                return Err(MyError::InvalidPage);
            }
            let template = match app.templates.load(&path).await {
                Ok(template) => template,
                Err(_) => return Err(MyError::InvalidPage),
            };
            templates.push(template);
            load_snippet_templates(app, body, templates).await?;
        }
        Ok(())
    })
}

fn render_document<'a>(
    root: &Utf8Path,
    document: &Document,
    page: &serde_json::Map<String, Json>,
    templates: &mut impl Iterator<Item = &'a Arc<Template>>,
) -> Result<String, MyError> {
    let mut expanded = String::new();
    for block in document.blocks() {
        match block {
            Block::Markdown(src) => expanded.push_str(src),
            Block::Snippet {
                name,
                params,
                body,
                line,
            } => {
                let template = templates.next().ok_or_else(|| {
                    MyError::Internal("missing snippet rendering dependency".into())
                })?;
                let body = render_document(root, body, page, templates)?;
                let mut context = params.clone();
                context.insert("contents".into(), Json::String(body));
                context.insert("page".into(), Json::Object(page.clone()));

                let path = root.join(format!("_style/snippets/{name}.html"));
                let mut hbs = handlebars::Handlebars::new();
                hbs.register_helper("is_empty", Box::new(is_empty));
                let html = hbs.render_template(&template.0, &context).map_err(|err| {
                    error!("invalid snippet `{path}` used at line {line}: {err}");
                    MyError::InvalidPage
                })?;
                expanded.push_str("\n\n");
                expanded.push_str(&html);
                expanded.push_str("\n\n");
            }
        }
    }
    Ok(render_markdown(&expanded))
}

// True for null, empty string, empty array, or empty object.
handlebars::handlebars_helper!(is_empty: |v: Json| {
    use handlebars::JsonValue;
    match v {
        JsonValue::Null => true,
        JsonValue::String(s) => s.is_empty(),
        JsonValue::Array(a) => a.is_empty(),
        JsonValue::Object(o) => o.is_empty(),
        _ => false,
    }
});

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

    #[tokio::test]
    async fn rendered_cache_tracks_page_and_snippets() {
        use std::sync::atomic::Ordering;

        let cache = RenderedPages::default();
        let root = Utf8Path::new("/tmp/rendered-cache-test");
        let path = root.join("page.md");
        let page = Arc::new(Page::compute(":::card\n\n**first**\n:::\n").unwrap());
        let template = Arc::new(Template("<aside>one {{{contents}}}</aside>".into()));

        let html = cache
            .load(&path, root, page.clone(), vec![template.clone()])
            .await
            .unwrap();
        assert!(html.contains("one <p><strong>first</strong>"));
        assert_eq!(cache.renders.load(Ordering::Relaxed), 1);

        cache
            .load(&path, root, page.clone(), vec![template])
            .await
            .unwrap();
        assert_eq!(cache.renders.load(Ordering::Relaxed), 1);

        let template = Arc::new(Template("<aside>two {{{contents}}}</aside>".into()));
        let html = cache
            .load(&path, root, page, vec![template.clone()])
            .await
            .unwrap();
        assert!(html.contains("two <p><strong>first</strong>"));
        assert_eq!(cache.renders.load(Ordering::Relaxed), 2);

        let page = Arc::new(Page::compute(":::card\n\n**second**\n:::\n").unwrap());
        let html = cache.load(&path, root, page, vec![template]).await.unwrap();
        assert!(html.contains("two <p><strong>second</strong>"));
        assert_eq!(cache.renders.load(Ordering::Relaxed), 3);

        cache.sweep(Duration::ZERO);
        assert!(cache.map.is_empty());
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
        let config = Config { protected, users };
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
            MyResponse::Html(h) => {
                assert!(h.contains("Hello"));
                assert!(h.contains("<strong>A snippet</strong>"));
                assert!(h.contains("contains <strong>Markdown</strong>"));
                assert!(h.contains("On My title by Flaty"));
            }
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
    async fn renders_snippet_context_and_body() {
        let dir = Utf8PathBuf::from_path_buf(
            std::env::temp_dir().join(format!("flaty-snippet-{}", std::process::id())),
        )
        .unwrap();
        std::fs::create_dir_all(dir.join("_style/snippets")).unwrap();
        std::fs::write(dir.join("_style/default.html"), "{{{contents}}}").unwrap();
        std::fs::write(
            dir.join("_style/snippets/card.html"),
            "<aside>{{label}} {{page.title}} {{{contents}}}</aside>",
        )
        .unwrap();
        std::fs::write(
            dir.join("page.md"),
            "---\ntitle = \"<Page>\"\n---\n[Link][target]\n\n:::card\nlabel = \"<Label>\"\n\n**Body**\n:::\n\n[target]: /ok\n",
        )
        .unwrap();

        let app = Arc::new(App::new(dir.clone()));
        let response = web(
            app,
            MyRequest::GET {
                path: "/",
                authorization: None,
            },
        )
        .await
        .unwrap();
        let MyResponse::Html(html) = response else {
            panic!("expected html");
        };
        assert!(html.contains("&lt;Label&gt; &lt;Page&gt;"));
        assert!(html.contains("<strong>Body</strong>"));
        assert!(html.contains("<a href=\"/ok\">Link</a>"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn strips_comments_from_layouts() {
        let dir = Utf8PathBuf::from_path_buf(
            std::env::temp_dir().join(format!("flaty-comments-{}", std::process::id())),
        )
        .unwrap();
        std::fs::create_dir_all(dir.join("_style")).unwrap();
        std::fs::write(
            dir.join("_style/default.html"),
            "<!-- layout note --><main>{{{contents}}}</main>",
        )
        .unwrap();
        std::fs::write(dir.join("page.md"), "Visible").unwrap();

        let app = Arc::new(App::new(dir.clone()));
        let MyResponse::Html(html) = web(
            app,
            MyRequest::GET {
                path: "/",
                authorization: None,
            },
        )
        .await
        .unwrap() else {
            panic!("expected html");
        };
        assert!(!html.contains("<!--"));
        assert!(!html.contains("layout note"));
        assert!(html.contains("<p>Visible</p>"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn missing_snippet_invalidates_page() {
        let dir = Utf8PathBuf::from_path_buf(
            std::env::temp_dir().join(format!("flaty-missing-snippet-{}", std::process::id())),
        )
        .unwrap();
        std::fs::create_dir_all(dir.join("_style")).unwrap();
        std::fs::write(dir.join("_style/default.html"), "{{{contents}}}").unwrap();
        std::fs::write(dir.join("page.md"), ":::missing\n:::\n").unwrap();

        let app = Arc::new(App::new(dir.clone()));
        let response = web(
            app,
            MyRequest::GET {
                path: "/",
                authorization: None,
            },
        )
        .await;
        assert!(matches!(response, Err(MyError::InvalidPage)));
        std::fs::remove_dir_all(&dir).ok();
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
    async fn real_css_wins_over_scss() {
        // A `.css` file on disk is served as-is, without compiling SCSS.
        let dir = Utf8PathBuf::from_path_buf(
            std::env::temp_dir().join(format!("flaty-css-{}", std::process::id())),
        )
        .unwrap();
        std::fs::create_dir_all(dir.join("_style")).unwrap();
        std::fs::write(dir.join("theme.css"), "body{color:red}").unwrap();
        std::fs::write(dir.join("_style/theme.scss"), "body{color:blue}").unwrap();
        let app = Arc::new(App::new(dir.clone()));
        let r = web(
            app,
            MyRequest::GET {
                path: "/theme.css",
                authorization: None,
            },
        )
        .await;
        assert!(matches!(r, Ok(MyResponse::File(_))));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn serves_static_file() {
        match resp("/heart.svg").await.unwrap() {
            MyResponse::File(f) => assert!(f.ends_with("heart.svg")),
            _ => panic!("expected file"),
        }
    }

    #[tokio::test]
    async fn missing_config_treated_as_empty() {
        // A directory without `_config.toml` still serves its files.
        let dir = Utf8PathBuf::from_path_buf(
            std::env::temp_dir().join(format!("flaty-noconfig-{}", std::process::id())),
        )
        .unwrap();
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("x.svg"), "<svg/>").unwrap();
        let app = Arc::new(App::new(dir.clone()));
        let r = web(
            app,
            MyRequest::GET {
                path: "/x.svg",
                authorization: None,
            },
        )
        .await;
        assert!(matches!(r, Ok(MyResponse::File(_))));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn missing_is_not_found() {
        assert!(matches!(resp("/nope/").await, Err(MyError::NotFound)));
    }
}
