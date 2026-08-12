use std::{fmt, ops::Range};

use anyhow::anyhow;
use pulldown_cmark::{html, Event, Parser, Tag, TagEnd};
use serde_json::{Map, Value as Json};
use toml::{Table, Value};

use crate::cache::Cacheable;

const MAX_SNIPPET_DEPTH: usize = 16;

#[derive(Debug)]
pub enum MarkdownError {
    InvalidHeader,
    InvalidDirective { line: usize, message: String },
}

impl fmt::Display for MarkdownError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidHeader => write!(f, "invalid page header"),
            Self::InvalidDirective { line, message } => {
                write!(f, "invalid snippet at line {line}: {message}")
            }
        }
    }
}

#[derive(Clone, Default)]
pub struct Document {
    blocks: Vec<Block>,
}

impl Document {
    pub fn blocks(&self) -> &[Block] {
        &self.blocks
    }
}

#[derive(Clone)]
pub enum Block {
    Markdown(String),
    Snippet {
        name: String,
        params: Map<String, Json>,
        body: Document,
        line: usize,
    },
}

// A parsed page: frontmatter fields plus its Markdown/snippet document.
#[derive(Clone, Default)]
pub struct Page {
    fields: Map<String, Json>,
    body: Document,
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

    pub fn body(&self) -> &Document {
        &self.body
    }
}

impl Cacheable for Page {
    fn compute(src: &str) -> anyhow::Result<Self> {
        markdown(src).map_err(|err| anyhow!(err.to_string()))
    }
}

fn markdown(doc: &str) -> Result<Page, MarkdownError> {
    let (fields, body) = parse_header(doc)?;
    let body = DirectiveParser::new(body).parse()?;
    Ok(Page { fields, body })
}

pub fn render_markdown(src: &str) -> String {
    let mut buf = String::new();
    html::push_html(&mut buf, strip_comments(Parser::new(src)).into_iter());
    buf
}

pub fn strip_html_comments(src: &str) -> String {
    let mut output = String::with_capacity(src.len());
    let mut remaining = src;
    while let Some(start) = remaining.find("<!--") {
        output.push_str(&remaining[..start]);
        let comment = &remaining[start + "<!--".len()..];
        let Some(end) = comment.find("-->") else {
            return output;
        };
        remaining = &comment[end + "-->".len()..];
    }
    output.push_str(remaining);
    output
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

struct DirectiveParser<'a> {
    src: &'a str,
    pos: usize,
    line: usize,
    html_ranges: Vec<Range<usize>>,
    html_range: usize,
}

enum ColonLine<'a> {
    Open { fence: usize, name: &'a str },
    Close { fence: usize },
    Malformed,
}

impl<'a> DirectiveParser<'a> {
    fn new(src: &'a str) -> Self {
        Self {
            src,
            pos: 0,
            line: 1,
            html_ranges: Parser::new(src)
                .into_offset_iter()
                .filter_map(|(event, range)| {
                    matches!(event, Event::Html(_) | Event::InlineHtml(_)).then_some(range)
                })
                .collect(),
            html_range: 0,
        }
    }

    fn parse(mut self) -> Result<Document, MarkdownError> {
        self.parse_blocks(None, 0)
    }

    fn parse_blocks(
        &mut self,
        closing: Option<usize>,
        depth: usize,
    ) -> Result<Document, MarkdownError> {
        let mut blocks = Vec::new();
        let mut markdown_start = self.pos;
        let mut code_fence = None;

        while self.pos < self.src.len() {
            let in_html = self.in_html(self.pos);
            let (line_start, line_end, line) = self.current_line();

            if in_html {
                self.consume_line(line_end);
                continue;
            }
            if let Some((marker, size)) = code_fence {
                if is_code_fence_close(line, marker, size) {
                    code_fence = None;
                }
                self.consume_line(line_end);
                continue;
            }
            if let Some(fence) = code_fence_open(line) {
                code_fence = Some(fence);
                self.consume_line(line_end);
                continue;
            }

            let Some(colon) = colon_line(line) else {
                self.consume_line(line_end);
                continue;
            };

            match colon {
                ColonLine::Close { fence } => {
                    if closing != Some(fence) {
                        return self.error("unexpected or mismatched closing fence");
                    }
                    push_markdown(&mut blocks, &self.src[markdown_start..line_start]);
                    self.consume_line(line_end);
                    return Ok(Document { blocks });
                }
                ColonLine::Malformed => return self.error("malformed opening fence"),
                ColonLine::Open { fence, name } => {
                    if depth >= MAX_SNIPPET_DEPTH {
                        return self.error("snippet nesting exceeds 16 levels");
                    }
                    if !valid_name(name) {
                        return self.error("invalid snippet name");
                    }

                    push_markdown(&mut blocks, &self.src[markdown_start..line_start]);
                    let directive_line = self.line;
                    self.consume_line(line_end);
                    let params_start = self.pos;

                    let (params, body) = loop {
                        if self.pos >= self.src.len() {
                            return self.error("unclosed snippet");
                        }
                        let (param_line_start, param_line_end, param_line) = self.current_line();
                        if param_line.trim().is_empty() {
                            let params =
                                self.parse_params(params_start, param_line_start, directive_line)?;
                            self.consume_line(param_line_end);
                            let body = self.parse_blocks(Some(fence), depth + 1)?;
                            break (params, body);
                        }
                        if matches!(colon_line(param_line), Some(ColonLine::Close { fence: n }) if n == fence)
                        {
                            let params =
                                self.parse_params(params_start, param_line_start, directive_line)?;
                            self.consume_line(param_line_end);
                            break (params, Document::default());
                        }
                        self.consume_line(param_line_end);
                    };
                    blocks.push(Block::Snippet {
                        name: name.to_owned(),
                        params,
                        body,
                        line: directive_line,
                    });
                    markdown_start = self.pos;
                }
            }
        }

        if closing.is_some() {
            return self.error("unclosed snippet");
        }
        push_markdown(&mut blocks, &self.src[markdown_start..]);
        Ok(Document { blocks })
    }

    fn parse_params(
        &self,
        start: usize,
        end: usize,
        line: usize,
    ) -> Result<Map<String, Json>, MarkdownError> {
        let table: Table =
            self.src[start..end]
                .parse()
                .map_err(|_| MarkdownError::InvalidDirective {
                    line,
                    message: "invalid TOML parameter block".into(),
                })?;
        let params: Map<String, Json> = table
            .into_iter()
            .map(|(key, value)| (key, toml_to_json(value)))
            .collect();
        if params.contains_key("contents") || params.contains_key("page") {
            return self.error("`contents` and `page` are reserved parameters");
        }
        Ok(params)
    }

    fn current_line(&self) -> (usize, usize, &'a str) {
        let start = self.pos;
        let end = self.src[start..]
            .find('\n')
            .map_or(self.src.len(), |i| start + i + 1);
        let raw = &self.src[start..end];
        let line = raw.strip_suffix('\n').unwrap_or(raw);
        let line = line.strip_suffix('\r').unwrap_or(line);
        (start, end, line)
    }

    fn consume_line(&mut self, end: usize) {
        self.pos = end;
        self.line += 1;
    }

    fn in_html(&mut self, pos: usize) -> bool {
        while self
            .html_ranges
            .get(self.html_range)
            .is_some_and(|range| range.end <= pos)
        {
            self.html_range += 1;
        }
        self.html_ranges
            .get(self.html_range)
            .is_some_and(|range| range.contains(&pos))
    }

    fn error<T>(&self, message: &str) -> Result<T, MarkdownError> {
        Err(MarkdownError::InvalidDirective {
            line: self.line,
            message: message.into(),
        })
    }
}

fn push_markdown(blocks: &mut Vec<Block>, src: &str) {
    if !src.is_empty() {
        blocks.push(Block::Markdown(src.to_owned()));
    }
}

fn colon_line(line: &str) -> Option<ColonLine<'_>> {
    if !line.starts_with(":::") {
        return None;
    }
    let fence = line.bytes().take_while(|b| *b == b':').count();
    let rest = &line[fence..];
    if rest.is_empty() {
        Some(ColonLine::Close { fence })
    } else if valid_name(rest) {
        Some(ColonLine::Open { fence, name: rest })
    } else {
        Some(ColonLine::Malformed)
    }
}

fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
}

fn code_fence_open(line: &str) -> Option<(u8, usize)> {
    let line = line
        .strip_prefix("   ")
        .or_else(|| line.strip_prefix("  "))
        .or_else(|| line.strip_prefix(' '))
        .unwrap_or(line);
    let marker = *line.as_bytes().first()?;
    if marker != b'`' && marker != b'~' {
        return None;
    }
    let size = line.bytes().take_while(|b| *b == marker).count();
    (size >= 3).then_some((marker, size))
}

fn is_code_fence_close(line: &str, marker: u8, opening_size: usize) -> bool {
    let line = line
        .strip_prefix("   ")
        .or_else(|| line.strip_prefix("  "))
        .or_else(|| line.strip_prefix(' '))
        .unwrap_or(line);
    let size = line.bytes().take_while(|b| *b == marker).count();
    size >= opening_size && line[size..].trim().is_empty()
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
    }

    #[test]
    fn body_only_has_no_header_fields() {
        let page = markdown("just text").unwrap();
        assert!(page.fields().is_empty());
        assert!(matches!(&page.body().blocks()[0], Block::Markdown(s) if s == "just text"));
    }

    #[test]
    fn strips_html_comments() {
        let doc = "text <!-- hideinline --> more\n\n<!-- a\nhideblock -->\n\n`<!-- keepspan -->`\n\n```\n<!-- keepfence -->\n```\n";
        let html = render_markdown(doc);
        assert!(!html.contains("<!--"), "comment markers leaked: {html}");
        assert!(!html.contains("hideinline"));
        assert!(!html.contains("hideblock"));
        assert!(html.contains("keepspan"));
        assert!(html.contains("keepfence"));
    }

    #[test]
    fn strips_comments_from_html() {
        assert_eq!(
            strip_html_comments("before<!-- hidden -->after<!-- also hidden -->"),
            "beforeafter"
        );
        assert_eq!(strip_html_comments("before<!-- hidden"), "before");
    }

    #[test]
    fn parses_typed_snippet() {
        let page = markdown(
            "before\n\n:::card\ntitle = \"Hi\"\ncount = 2\nactive = true\n\nBody **text**.\n:::\n\nafter",
        )
        .unwrap();
        let Block::Snippet {
            name, params, body, ..
        } = &page.body().blocks()[1]
        else {
            panic!("expected snippet");
        };
        assert_eq!(name, "card");
        assert_eq!(params["title"], "Hi");
        assert_eq!(params["count"], 2);
        assert_eq!(params["active"], true);
        assert!(matches!(&body.blocks()[0], Block::Markdown(s) if s.contains("Body")));
    }

    #[test]
    fn parses_nested_snippets() {
        let page = markdown("::::outer\n\n:::inner\n:::\n::::\n").unwrap();
        let Block::Snippet { body, .. } = &page.body().blocks()[0] else {
            panic!("expected outer snippet");
        };
        assert!(matches!(&body.blocks()[0], Block::Snippet { name, .. } if name == "inner"));
    }

    #[test]
    fn ignores_directives_in_code_fences() {
        let page = markdown("```markdown\n:::card\n:::\n```\n").unwrap();
        assert_eq!(page.body().blocks().len(), 1);
        assert!(matches!(&page.body().blocks()[0], Block::Markdown(_)));
    }

    #[test]
    fn ignores_directives_in_html() {
        for doc in [
            "<!--\n:::missing\n:::\n-->\n",
            "text <!--\n:::missing\n:::\n-->\n",
            "<script>\n:::missing\n:::\n</script>\n",
            "<div>\n:::missing\n:::\n</div>\n",
        ] {
            let page = markdown(doc).unwrap();
            assert_eq!(page.body().blocks().len(), 1);
            assert!(matches!(&page.body().blocks()[0], Block::Markdown(_)));
        }
    }

    #[test]
    fn rejects_bad_directives() {
        assert!(markdown(":::card\npage = true\n:::\n").is_err());
        assert!(markdown(":::card\n\nbody\n").is_err());
        assert!(markdown(":::bad/name\n:::\n").is_err());
        assert!(markdown(":::\n").is_err());
    }

    #[test]
    fn limits_snippet_nesting() {
        let mut doc = String::new();
        for size in (3..20).rev() {
            doc.push_str(&format!("{}card\n\n", ":".repeat(size)));
        }
        for size in 3..20 {
            doc.push_str(&format!("{}\n", ":".repeat(size)));
        }
        assert!(markdown(&doc).is_err());
    }

    #[test]
    fn broken_header_errors() {
        assert!(markdown("---\ntitle = \"x\n---\nbody").is_err());
    }
}
