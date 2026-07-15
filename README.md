# flaty

Flat-File CMS written in Rust.

flaty serves a directory of Markdown files as a website. There is no database
and no build step: pages are rendered on request (and cached), so editing a file
is immediately reflected on the next reload.

## Quick start

Serve a site directory with Docker:

```
docker run --rm -it --name flaty --read-only -p 8080:8080 -v ./your_site_root:/data:ro ghcr.io/linfir/flaty
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

`_config.toml` is optional: a missing config is treated as empty (no access
control). A present config must parse -- while it is invalid the site serves
404, so a broken edit cannot silently disable access control, and it recovers
once the file is fixed.

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

Every front-matter field becomes a template variable, keeping its TOML type:
strings, numbers, and booleans stay themselves, and arrays and tables are passed
through too (datetimes become strings, since JSON has no datetime). So a boolean
works with `{{#if draft}}` or `(eq author false)`, a number compares numerically
with `(gt n 10)`, and a list drives `{{#each tags}}`. The rendered Markdown body
is available as the `contents` variable.

The `template` field selects the layout (see below); it defaults to `default`.

## Templates

Templates live in `_style/<name>.html` and are rendered with
[handlebars-rust](https://github.com/sunng87/handlebars-rust), the Rust
implementation of [Handlebars](https://handlebarsjs.com/). A page selects one
with the `template` front-matter field. The template receives all of the page's
front-matter fields plus `contents`. Use triple braces to emit raw HTML (double
braces HTML-escape):

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

In addition to the built-in Handlebars helpers, flaty registers an `is_empty`
helper. It is true for a missing field, an empty string, an empty array, or an
empty table, and is meant for subexpressions:

```html
<title>{{#if (is_empty title)}}Untitled{{else}}{{title}}{{/if}}</title>
```

## Styles

A stylesheet `_style/<name>.scss` is compiled from SCSS and served at
`/<name>.css`. So the `<link href="/default.css">` above is produced from
`_style/default.scss`. Each template can ship its own stylesheet.

## Static assets

Any request path with a file extension is served as a raw file from disk, so
`/heart.svg` is served from `heart.svg`. Files under `_style/` and any path
component starting with `.` or `_` remain unreachable.

## Access control

Paths can be protected with HTTP Basic auth. Each protected path prefix maps to
the list of users allowed under it, and credentials live in a separate table:

```toml
# prefix -> users allowed at (or under) that prefix
[protected]
"/foo" = ["user1"]
"/bar" = ["user2"]
"/quz" = ["user1", "user2"]

# plain-text credentials
[users]
user1 = "pw1"
user2 = "pw2"
```

With the above, `/foo` is restricted to `user1`, `/bar` to `user2`, and `/quz`
to either. Protection covers everything under a prefix -- pages, stylesheets and
static files alike -- and when prefixes overlap, the most specific (longest)
match applies. Passwords are stored in clear text, so this is meant for casual
gating, not sensitive data; serve over HTTPS (for example behind a reverse
proxy) so credentials are not sent in the clear.

## Deployment

flaty is meant to run behind a reverse proxy that terminates HTTPS. With
nginx:

- Proxy with `proxy_pass http://flaty;` (no URI part), so the raw request URI
  is forwarded unchanged. flaty never percent-decodes paths and uses the same
  string for access control and file lookup, which keeps encoded-path tricks
  harmless; forwarding the raw URI preserves that property.
- Expose the container port only to the proxy, not publicly.
- flaty does not rate-limit authentication attempts; add a `limit_req` zone
  on protected prefixes to slow down password brute force.
- Add HTTPS-related headers (`Strict-Transport-Security`, a
  `Content-Security-Policy`) at the proxy. flaty itself sends
  `X-Content-Type-Options: nosniff`.

The container runs as an unprivileged user and only reads `/data`, so
`--read-only` and a `:ro` volume mount (as in the quick start) are
recommended.

## Multi-site

One flaty instance can serve several websites. With `--multi`, the data
directory contains one subdirectory per hostname, each a normal flaty site:

```
/data/
  example.com/
    _config.toml
    _style/
    page.md
  blog.org/
    _config.toml
    _style/
    page.md
```

The `Host` header of each request selects the site (lowercased, `:port` and
trailing dot stripped). A request whose host matches no directory gets a plain
404, and every site's `_config.toml` is validated at startup. Sites are
discovered at startup, so restart the server after adding one.

Run it with the image's multi-site switch:

```
docker run --rm -it --name flaty --read-only -p 8080:8080 -v ./sites:/data:ro \
  -e FLATY_MULTI=true ghcr.io/linfir/flaty
```

The image defaults to `--bind 0.0.0.0 --port 8080 --directory /data`. These can
be changed with `FLATY_BIND`, `FLATY_PORT` and `FLATY_DIRECTORY`, or by passing
explicit flaty arguments after the image name.

With nginx, every vhost proxies to the same upstream and must forward the
original host:

```nginx
proxy_pass http://flaty;        # no URI part, as above
proxy_set_header Host $host;    # required: nginx would otherwise send "flaty"
```

For host aliases (`www.example.com`), either redirect at the proxy or symlink
one site directory to another.

## Custom error pages

If present, `_style/404.html` and `_style/500.html` are served for the
corresponding errors. Otherwise a minimal default response is returned.

## URLs

- `/foo/` renders `foo/page.md`; `/` renders the top-level `page.md`.
- `/foo` (no trailing slash) redirects to `/foo/` when the page exists.
- `/<name>.css` compiles `_style/<name>.scss`.
- A whitelisted static file is served at its path.
- Names starting with `_` or `.` are rejected.
- Only `GET` and `HEAD` are handled.

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
