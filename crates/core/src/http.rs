//! A std-only mini HTTP server: thread-per-connection, HTTP/1.1 request
//! parsing (request line, headers, `Content-Length` body), urlencoded
//! query strings and form bodies, and a tiny [`Response`] builder.
//!
//! Deliberately minimal — this is demo infrastructure, not a web
//! framework. Connections are closed after each response.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::sync::Arc;

/// A parsed HTTP request.
///
/// Header names are lowercased. `path` excludes the query string.
/// `cookies` is parsed from the `Cookie` header.
#[derive(Debug, Clone, Default)]
pub struct Request {
    pub method: String,
    pub path: String,
    pub query: HashMap<String, String>,
    pub headers: HashMap<String, String>,
    pub body: String,
    pub cookies: HashMap<String, String>,
}

/// An HTTP response. Extra headers (including `Set-Cookie`) ride in
/// `headers` as `(name, value)` pairs and are emitted in order, so
/// multiple cookies can be set by pushing multiple pairs.
#[derive(Debug, Clone)]
pub struct Response {
    pub status: u16,
    pub content_type: String,
    pub body: String,
    pub headers: Vec<(String, String)>,
}

impl Response {
    pub fn html(body: impl Into<String>) -> Response {
        Response {
            status: 200,
            content_type: "text/html; charset=utf-8".to_string(),
            body: body.into(),
            headers: Vec::new(),
        }
    }

    pub fn text(body: impl Into<String>) -> Response {
        Response {
            status: 200,
            content_type: "text/plain; charset=utf-8".to_string(),
            body: body.into(),
            headers: Vec::new(),
        }
    }

    pub fn json(body: impl Into<String>) -> Response {
        Response {
            status: 200,
            content_type: "application/json".to_string(),
            body: body.into(),
            headers: Vec::new(),
        }
    }

    /// 302 Found with a `Location` header and an empty body.
    pub fn redirect(location: impl Into<String>) -> Response {
        Response {
            status: 302,
            content_type: "text/plain; charset=utf-8".to_string(),
            body: String::new(),
            headers: vec![("Location".to_string(), location.into())],
        }
    }

    pub fn with_header(mut self, key: impl Into<String>, value: impl Into<String>) -> Response {
        self.headers.push((key.into(), value.into()));
        self
    }

    /// Append a `Set-Cookie` header whose value is the full cookie
    /// string, e.g. `name=value; HttpOnly; Path=/`.
    pub fn set_cookie(self, value_string: impl Into<String>) -> Response {
        self.with_header("Set-Cookie", value_string)
    }
}

fn reason_phrase(status: u16) -> &'static str {
    match status {
        200 => "OK",
        201 => "Created",
        204 => "No Content",
        301 => "Moved Permanently",
        302 => "Found",
        303 => "See Other",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        500 => "Internal Server Error",
        _ => "Status",
    }
}

/// Parse an `application/x-www-form-urlencoded` string into a map.
/// Later duplicate keys overwrite earlier ones.
pub fn parse_urlencoded(s: &str) -> HashMap<String, String> {
    let mut out = HashMap::new();
    for pair in s.split('&') {
        if pair.is_empty() {
            continue;
        }
        let (key, value) = match pair.split_once('=') {
            Some((k, v)) => (k, v),
            None => (pair, ""),
        };
        out.insert(url_decode(key), url_decode(value));
    }
    out
}

/// Parse a request's urlencoded body (a form submission).
pub fn form(req: &Request) -> HashMap<String, String> {
    parse_urlencoded(&req.body)
}

/// HTML-escape a string for safe interpolation into markup.
pub fn esc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(ch),
        }
    }
    out
}

fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// Percent-decode a string; `+` decodes to a space (form/query rules).
/// Invalid `%` sequences are left literal. The decoded bytes are
/// interpreted as UTF-8 with lossy replacement.
pub fn url_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => {
                match (hex_val(bytes[i + 1]), hex_val(bytes[i + 2])) {
                    (Some(hi), Some(lo)) => {
                        out.push(hi * 16 + lo);
                        i += 3;
                    }
                    _ => {
                        out.push(bytes[i]);
                        i += 1;
                    }
                }
            }
            b'%' => {
                out.push(bytes[i]);
                i += 1;
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Percent-encode a string for use as a URL query value. Unreserved
/// characters (`A-Z a-z 0-9 - _ . ~`) pass through; everything else,
/// including space (`%20`), is `%XX`-encoded.
pub fn url_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*b as char)
            }
            _ => out.push_str(&format!("%{:02X}", b)),
        }
    }
    out
}

fn parse_cookies(header: &str) -> HashMap<String, String> {
    let mut out = HashMap::new();
    for part in header.split(';') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        if let Some((k, v)) = part.split_once('=') {
            out.insert(k.trim().to_string(), v.trim().to_string());
        }
    }
    out
}

/// Read and parse one request from `reader`. Returns `None` on EOF or a
/// request so malformed that no response routing is possible.
fn read_request(reader: &mut BufReader<std::net::TcpStream>) -> Option<Request> {
    let mut request_line = String::new();
    match reader.read_line(&mut request_line) {
        Ok(0) => return None,
        Ok(_) => {}
        Err(_) => return None,
    }
    let request_line = request_line.trim_end().to_string();
    if request_line.is_empty() {
        return None;
    }
    let mut parts = request_line.split_whitespace();
    let method = parts.next()?.to_string();
    let raw_target = parts.next()?.to_string();

    let mut headers: HashMap<String, String> = HashMap::new();
    loop {
        let mut line = String::new();
        match reader.read_line(&mut line) {
            Ok(0) => break,
            Ok(_) => {}
            Err(_) => break,
        }
        let line = line.trim_end();
        if line.is_empty() {
            break;
        }
        if let Some((k, v)) = line.split_once(':') {
            headers.insert(k.trim().to_lowercase(), v.trim().to_string());
        }
    }

    let content_length: usize = headers
        .get("content-length")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let mut body_bytes = vec![0u8; content_length];
    if content_length > 0 && reader.read_exact(&mut body_bytes).is_err() {
        return None;
    }
    let body = String::from_utf8_lossy(&body_bytes).into_owned();

    let (raw_path, raw_query) = match raw_target.split_once('?') {
        Some((p, q)) => (p.to_string(), q.to_string()),
        None => (raw_target, String::new()),
    };
    let cookies = headers
        .get("cookie")
        .map(|h| parse_cookies(h))
        .unwrap_or_default();

    Some(Request {
        method,
        path: url_decode(&raw_path),
        query: parse_urlencoded(&raw_query),
        headers,
        body,
        cookies,
    })
}

fn write_response(stream: &mut std::net::TcpStream, resp: &Response) {
    let mut head = format!(
        "HTTP/1.1 {} {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n",
        resp.status,
        reason_phrase(resp.status),
        resp.content_type,
        resp.body.len()
    );
    for (k, v) in &resp.headers {
        head.push_str(&format!("{}: {}\r\n", k, v));
    }
    head.push_str("\r\n");
    let _ = stream.write_all(head.as_bytes());
    let _ = stream.write_all(resp.body.as_bytes());
    let _ = stream.flush();
}

/// Serve HTTP on `port` forever, one thread per connection.
///
/// The handler is shared across threads (`Send + Sync`) and called once
/// per request. Parse failures get a bare `400 Bad Request`.
pub fn serve(
    port: u16,
    handler: impl Fn(&Request) -> Response + Send + Sync + 'static,
) -> std::io::Result<()> {
    // Backlog matters: std's TcpListener::bind uses a 128-deep listen
    // backlog, and a burst of simultaneous connects overflows it — the
    // kernel drops SYNs and clients retransmit after ~1 s, which showed
    // up in load testing as a ~1,050 ms p95 tail at 500 concurrent
    // clients (0 errors, but a blown latency budget). Bind via socket2
    // with a 1024-deep backlog instead.
    let socket = socket2::Socket::new(
        socket2::Domain::IPV4,
        socket2::Type::STREAM,
        Some(socket2::Protocol::TCP),
    )?;
    socket.set_reuse_address(true)?;
    socket.bind(&std::net::SocketAddr::from(([0, 0, 0, 0], port)).into())?;
    socket.listen(1024)?;
    let listener = TcpListener::from(socket);
    let handler = Arc::new(handler);
    for stream in listener.incoming() {
        let stream = match stream {
            Ok(s) => s,
            Err(_) => continue,
        };
        let handler = Arc::clone(&handler);
        std::thread::spawn(move || {
            let _ = stream.set_read_timeout(Some(std::time::Duration::from_secs(15)));
            let mut writer = match stream.try_clone() {
                Ok(w) => w,
                Err(_) => return,
            };
            let mut reader = BufReader::new(stream);
            match read_request(&mut reader) {
                Some(req) => {
                    let resp = handler(&req);
                    write_response(&mut writer, &resp);
                }
                None => {
                    let resp = Response {
                        status: 400,
                        content_type: "text/plain; charset=utf-8".to_string(),
                        body: "bad request".to_string(),
                        headers: Vec::new(),
                    };
                    write_response(&mut writer, &resp);
                }
            }
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn esc_escapes_html_metacharacters() {
        assert_eq!(
            esc("<a href=\"x\">&'</a>"),
            "&lt;a href=&quot;x&quot;&gt;&amp;&#39;&lt;/a&gt;"
        );
        assert_eq!(esc("plain"), "plain");
    }

    #[test]
    fn url_codec_round_trip() {
        for s in ["hello world", "a&b=c", "/auth/login?return=/", "100% sure", "ünïcode"] {
            assert_eq!(url_decode(&url_encode(s)), s, "round trip of {:?}", s);
        }
        assert_eq!(url_decode("a+b"), "a b");
        assert_eq!(url_decode("%2Fpath"), "/path");
        assert_eq!(url_decode("%zz"), "%zz");
        assert_eq!(url_encode("a b"), "a%20b");
    }

    #[test]
    fn form_parses_urlencoded_body() {
        let mut req = Request::default();
        req.body = "name=Jane+Doe&note=a%26b&empty=".to_string();
        let f = form(&req);
        assert_eq!(f.get("name").map(|s| s.as_str()), Some("Jane Doe"));
        assert_eq!(f.get("note").map(|s| s.as_str()), Some("a&b"));
        assert_eq!(f.get("empty").map(|s| s.as_str()), Some(""));
    }

    #[test]
    fn response_builders() {
        let r = Response::redirect("/auth/login");
        assert_eq!(r.status, 302);
        assert_eq!(
            r.headers,
            vec![("Location".to_string(), "/auth/login".to_string())]
        );
        let r = Response::html("<p>hi</p>").set_cookie("exo_session=abc; HttpOnly; Path=/");
        assert_eq!(r.status, 200);
        assert!(r.content_type.starts_with("text/html"));
        assert_eq!(r.headers[0].0, "Set-Cookie");
    }
}
