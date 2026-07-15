use anyhow::anyhow;
use pulldown_cmark::{html, Event, Parser, Tag, TagEnd};
use serde_json::{Map, Value as Json};
use toml::{Table, Value};

use crate::cache::Cacheable;

#[derive(Debug)]
pub enum MarkdownError {
    InvalidHeader,
}

// A rendered page: frontmatter fields plus the "contents" HTML.
#[derive(Clone, Default)]
pub struct Page {
    fields: Map<String, Json>,
}

impl Page {
    pub fn template(&self) -> &str {
        self.fields
            .get("template")
            .and_then(Json::as_str)
            .unwrap_or("default")
    }

    pub fn fields(&self) -> &Map<String, Json> {
        &self.fields
    }
}

impl Cacheable for Page {
    fn compute(src: &str) -> anyhow::Result<Self> {
        markdown(src).map_err(|_| anyhow!("invalid page header"))
    }
}

fn markdown(doc: &str) -> Result<Page, MarkdownError> {
    let (mut fields, doc) = parse_header(doc)?;
    let mut buf = String::new();
    html::push_html(&mut buf, strip_comments(Parser::new(doc)).into_iter());
    fields.insert("contents".into(), Json::String(buf));
    Ok(Page { fields })
}

// Drop `<!-- ... -->` comments so they never reach the output. A comment inside
// a code span or block is plain text (not an Html event) and is kept.
fn strip_comments<'a>(events: impl Iterator<Item = Event<'a>>) -> Vec<Event<'a>> {
    let is_comment = |s: &str| s.trim_start().starts_with("<!--");
    let mut out = Vec::new();
    let mut iter = events.peekable();
    while let Some(event) = iter.next() {
        match &event {
            Event::InlineHtml(s) if is_comment(s) => {}
            // A block comment spans Start(HtmlBlock) .. End(HtmlBlock); skip it
            // whole when its first line opens a comment.
            Event::Start(Tag::HtmlBlock) if matches!(iter.peek(), Some(Event::Html(s)) if is_comment(s)) => {
                for inner in iter.by_ref() {
                    if matches!(inner, Event::End(TagEnd::HtmlBlock)) {
                        break;
                    }
                }
            }
            _ => out.push(event),
        }
    }
    out
}

fn parse_header(src: &str) -> Result<(Map<String, Json>, &str), MarkdownError> {
    match split(src) {
        Some((header, body)) => {
            let table: Table = header.parse().map_err(|_| MarkdownError::InvalidHeader)?;
            let fields = table
                .into_iter()
                .map(|(k, v)| (k, toml_to_json(v)))
                .collect();
            Ok((fields, body))
        }
        None => Ok((Map::new(), src)),
    }
}

// TOML maps onto JSON one-to-one, except datetimes, which JSON lacks and we
// render as strings.
fn toml_to_json(value: Value) -> Json {
    match value {
        Value::String(s) => Json::String(s),
        Value::Integer(n) => Json::Number(n.into()),
        Value::Float(f) => serde_json::Number::from_f64(f).map_or(Json::Null, Json::Number),
        Value::Boolean(b) => Json::Bool(b),
        Value::Datetime(d) => Json::String(d.to_string()),
        Value::Array(a) => Json::Array(a.into_iter().map(toml_to_json).collect()),
        Value::Table(t) => Json::Object(t.into_iter().map(|(k, v)| (k, toml_to_json(v))).collect()),
    }
}

fn split(data: &str) -> Option<(&str, &str)> {
    let data = data.trim_start().strip_prefix("---\n")?;
    let i = data.find("\n---\n")?;
    Some((&data[..i], &data[i + 5..]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_frontmatter_types() {
        let doc = "---\ntitle = \"T\"\ndraft = true\nn = 42\ntags = [\"a\", \"b\"]\n---\nbody text";
        let page = markdown(doc).unwrap();
        let f = page.fields();
        assert_eq!(f.get("title").and_then(Json::as_str), Some("T"));
        assert_eq!(f.get("draft").and_then(Json::as_bool), Some(true));
        assert_eq!(f.get("n").and_then(Json::as_i64), Some(42));
        assert_eq!(
            f.get("tags").and_then(Json::as_array).map(Vec::len),
            Some(2)
        );
        assert!(f["contents"].as_str().unwrap().contains("body text"));
    }

    #[test]
    fn body_only_has_no_header_fields() {
        let page = markdown("just text").unwrap();
        assert!(page.fields()["contents"]
            .as_str()
            .unwrap()
            .contains("just text"));
        assert!(page.fields().get("title").is_none());
    }

    #[test]
    fn strips_html_comments() {
        let doc = "text <!-- hideinline --> more\n\n<!-- a\nhideblock -->\n\n`<!-- keepspan -->`\n\n```\n<!-- keepfence -->\n```\n";
        let html = markdown(doc).unwrap().fields()["contents"]
            .as_str()
            .unwrap()
            .to_string();
        assert!(!html.contains("<!--"), "comment markers leaked: {html}");
        assert!(!html.contains("hideinline"));
        assert!(!html.contains("hideblock"));
        assert!(html.contains("keepspan"));
        assert!(html.contains("keepfence"));
    }

    #[test]
    fn broken_header_errors() {
        assert!(markdown("---\ntitle = \"x\n---\nbody").is_err());
    }
}
