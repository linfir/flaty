# flaty

Flat-File CMS written in Rust.

flaty serves a directory of Markdown files as a website. There is no database
and no build step: pages are rendered on request (and cached), so editing a file
is immediately reflected on the next reload.

## Quick start

Serve a site directory with Docker:

```
docker run --rm -it --name flaty -p 8080:80 -v ./your_site_root:/data:ro ghcr.io/linfir/flaty
```

Then open <http://localhost:8080/>.

## How it works

A site is just a directory. For a request to `/foo/`, flaty renders
`foo/page.md` through a Handlebars template and returns HTML. The root URL `/`
renders `page.md` at the top of the site.

```
your_site_root/
  _config.toml            # site configuration
  _style/                 # templates, stylesheets, error pages (never served directly)
    default.html          # the default page template
    default.scss          # compiled and served at /default.css
    404.html              # optional custom error pages
  page.md                 # the home page  (served at /)
  heart.svg               # a static asset (served at /heart.svg)
  about/
    page.md               # served at /about/
```

Any file or directory whose name starts with `_` or `.` is never served, so
`_style/` and `_config.toml` stay private.

## Pages

A page is a `page.md` file: an optional TOML front-matter block followed by
Markdown (CommonMark; inline HTML is passed through).

```markdown
---
title = "My title"
author = "Flaty"
lang = "en"
template = "default"
---

# Hello

This is a *very simple* page, with a [link](/about/).
```

Every front-matter field becomes a template variable. Non-string scalar values
(numbers, booleans, dates) are converted to strings; arrays and tables are
ignored. The rendered Markdown body is available as the `contents` variable.

The `template` field selects the layout (see below); it defaults to `default`.

## Templates

Templates live in `_style/<name>.html` and are rendered with
[Handlebars](https://handlebarsjs.com/). A page selects one with the `template`
front-matter field. The template receives all of the page's front-matter fields
plus `contents`. Use triple braces to emit raw HTML (double braces HTML-escape):

```html
<!doctype html>
<html lang="{{{lang}}}">
  <head>
    <link rel="stylesheet" href="/default.css" />
    <title>{{title}}</title>
  </head>
  <body>
    {{{contents}}}
  </body>
</html>
```

Template names must be bare identifiers (letters, digits, `-`, `_`).

## Styles

A stylesheet `_style/<name>.scss` is compiled from SCSS and served at
`/<name>.css`. So the `<link href="/default.css">` above is produced from
`_style/default.scss`. Each template can ship its own stylesheet.

## Static assets

Only file extensions listed in `_config.toml` are served as static files. This
is an allow-list, so nothing is exposed by accident:

```toml
extensions = ["svg", "png", "jpg", "pdf"]
```

With the above, `/heart.svg` is served from `heart.svg` on disk. A request for an
extension that is not listed returns 404.

## Custom error pages

If present, `_style/404.html` and `_style/500.html` are served for the
corresponding errors. Otherwise a minimal default response is returned.

## URLs

- `/foo/` renders `foo/page.md`; `/` renders the top-level `page.md`.
- `/foo` (no trailing slash) redirects to `/foo/` when the page exists.
- `/<name>.css` compiles `_style/<name>.scss`.
- A whitelisted static file is served at its path.
- Names starting with `_` or `.` are rejected.
- Only `GET` is handled.

## Caching

Rendered pages, templates and stylesheets are cached in memory and reloaded
automatically when the source file changes. Responses carry an `ETag`, so a
conditional request (`If-None-Match`) returns `304 Not Modified` when nothing has
changed.

## Running from source

```
cargo run -- --directory your_site_root --bind localhost --port 8080
```

Flags (all optional):

| Flag | Default | Description |
| --- | --- | --- |
| `-d`, `--directory` | `.` | Site directory |
| `-b`, `--bind` | `localhost` | Bind address |
| `-p`, `--port` | `8080` | Port |

For local development with auto-reload of the server itself, see the `justfile`
(`just dev`).

## License

[AGPL-3.0-only](LICENSE).
