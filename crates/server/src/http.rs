//! Just enough HTTP/1.1 for one proxy (`tailscale serve`) and a few people: keep-alive,
//! `Content-Length` bodies (no chunked requests), JSON, files and event streams out.
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::time::Duration;

pub const MAX_HEAD: usize = 16 << 10;
pub const MAX_BODY: usize = 1 << 20;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    pub method: String,
    /// Without the query.
    pub path: String,
    pub query: String,
    /// Lowercased names.
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
    pub peer: SocketAddr,
}

impl Request {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
    }

    pub fn all(&self, name: &str) -> impl Iterator<Item = &str> {
        self.headers.iter().filter(move |(k, _)| k == name).map(|(_, v)| v.as_str())
    }

    pub fn keep_alive(&self) -> bool {
        !self.header("connection").is_some_and(|c| c.eq_ignore_ascii_case("close"))
    }
}

#[derive(Debug)]
pub enum ReadError {
    /// The client closed (or idled out) between requests.
    Closed,
    TooLarge,
    Bad,
    Io(std::io::Error),
}

/// Reads one request from a kept-alive connection.
pub fn read_request(r: &mut BufReader<TcpStream>, peer: SocketAddr) -> Result<Request, ReadError> {
    let mut head = 0usize;
    let mut line = String::new();
    let n = r.read_line(&mut line).map_err(|e| if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) { ReadError::Closed } else { ReadError::Io(e) })?;
    if n == 0 {
        return Err(ReadError::Closed);
    }
    head += n;
    let mut parts = line.split_whitespace();
    let (method, target, version) = (parts.next().unwrap_or(""), parts.next().unwrap_or(""), parts.next().unwrap_or(""));
    if method.is_empty() || !target.starts_with('/') || !version.starts_with("HTTP/1.") {
        return Err(ReadError::Bad);
    }
    let (path, query) = target.split_once('?').map(|(p, q)| (p.to_string(), q.to_string())).unwrap_or((target.to_string(), String::new()));
    let mut headers = Vec::new();
    loop {
        let mut h = String::new();
        let n = r.read_line(&mut h).map_err(ReadError::Io)?;
        if n == 0 {
            return Err(ReadError::Bad);
        }
        head += n;
        if head > MAX_HEAD {
            return Err(ReadError::TooLarge);
        }
        let h = h.trim_end_matches(['\r', '\n']);
        if h.is_empty() {
            break;
        }
        let Some((k, v)) = h.split_once(':') else { return Err(ReadError::Bad) };
        headers.push((k.trim().to_ascii_lowercase(), v.trim().to_string()));
    }
    if headers.iter().any(|(k, v)| k == "transfer-encoding" && !v.eq_ignore_ascii_case("identity")) {
        return Err(ReadError::Bad);
    }
    let len: usize = match headers.iter().find(|(k, _)| k == "content-length") {
        Some((_, v)) => v.parse().map_err(|_| ReadError::Bad)?,
        None => 0,
    };
    if len > MAX_BODY {
        return Err(ReadError::TooLarge);
    }
    let mut body = vec![0; len];
    r.read_exact(&mut body).map_err(ReadError::Io)?;
    Ok(Request { method: method.to_string(), path, query, headers, body, peer })
}

pub enum Body {
    Bytes(Vec<u8>),
    /// A file streamed with its length.
    File(std::fs::File, u64),
    /// An event stream: the handler writes to the socket itself and the connection closes.
    Stream(Box<dyn FnOnce(&mut TcpStream) + Send>),
}

pub struct Response {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Body,
}

impl Response {
    pub fn json(status: u16, v: &serde_json::Value) -> Response {
        Response { status, headers: vec![("Content-Type".into(), "application/json".into())], body: Body::Bytes(serde_json::to_vec(v).unwrap_or_default()) }
    }

    pub fn error(e: &otherwise_remote::ApiError) -> Response {
        let mut r = Response::json(e.status(), &e.envelope());
        if e.status() == 503 {
            r.headers.push(("Retry-After".into(), "10".into()));
        }
        r
    }

    pub fn empty(status: u16) -> Response {
        Response { status, headers: Vec::new(), body: Body::Bytes(Vec::new()) }
    }

    pub fn header(mut self, k: &str, v: &str) -> Response {
        self.headers.push((k.into(), v.into()));
        self
    }
}

fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        304 => "Not Modified",
        400 => "Bad Request",
        401 => "Unauthorized",
        402 => "Payment Required",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        409 => "Conflict",
        413 => "Payload Too Large",
        503 => "Service Unavailable",
        _ => "Error",
    }
}

/// Writes `resp`; returns whether the connection may carry another request.
pub fn write_response(s: &mut TcpStream, resp: Response, keep_alive: bool) -> std::io::Result<bool> {
    let mut head = format!("HTTP/1.1 {} {}\r\n", resp.status, reason(resp.status));
    for (k, v) in &resp.headers {
        head.push_str(&format!("{k}: {v}\r\n"));
    }
    head.push_str("Cache-Control: no-store\r\nX-Content-Type-Options: nosniff\r\n");
    match resp.body {
        Body::Bytes(b) => {
            head.push_str(&format!("Content-Length: {}\r\n", b.len()));
            head.push_str(if keep_alive { "Connection: keep-alive\r\n\r\n" } else { "Connection: close\r\n\r\n" });
            s.write_all(head.as_bytes())?;
            s.write_all(&b)?;
            s.flush()?;
            Ok(keep_alive)
        }
        Body::File(mut f, len) => {
            head.push_str(&format!("Content-Length: {len}\r\n"));
            head.push_str(if keep_alive { "Connection: keep-alive\r\n\r\n" } else { "Connection: close\r\n\r\n" });
            s.write_all(head.as_bytes())?;
            let n = std::io::copy(&mut (&mut f).take(len), s)?;
            s.flush()?;
            Ok(keep_alive && n == len)
        }
        Body::Stream(f) => {
            head.push_str("Content-Type: text/event-stream\r\nConnection: close\r\n\r\n");
            s.write_all(head.as_bytes())?;
            s.flush()?;
            let _ = s.set_read_timeout(None);
            let _ = s.set_write_timeout(Some(Duration::from_secs(30)));
            f(s);
            Ok(false)
        }
    }
}

/// `a=1&b=x` → value of `k` (no percent-decoding needed for our ids and dates).
pub fn query_param<'a>(q: &'a str, k: &str) -> Option<&'a str> {
    q.split('&').filter_map(|kv| kv.split_once('=')).find(|(a, _)| *a == k).map(|(_, v)| v)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    fn roundtrip(raw: &'static [u8]) -> Result<Request, ReadError> {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap();
        std::thread::spawn(move || {
            let mut c = TcpStream::connect(addr).unwrap();
            c.write_all(raw).unwrap();
            std::thread::sleep(Duration::from_millis(200));
        });
        let (s, peer) = l.accept().unwrap();
        read_request(&mut BufReader::new(s), peer)
    }

    #[test]
    fn parses_requests_and_refuses_what_it_doesnt_take() {
        let r = roundtrip(b"POST /v1/apps/expense/docs/acme/ops?x=1 HTTP/1.1\r\nHost: s\r\nX-Forwarded-For: 1.2.3.4\r\nX-Forwarded-For: 100.64.0.9\r\nContent-Length: 2\r\n\r\n{}").unwrap();
        assert_eq!((r.method.as_str(), r.path.as_str(), r.query.as_str(), r.body.as_slice()), ("POST", "/v1/apps/expense/docs/acme/ops", "x=1", b"{}".as_slice()));
        assert_eq!(r.all("x-forwarded-for").collect::<Vec<_>>(), ["1.2.3.4", "100.64.0.9"]);
        assert!(matches!(roundtrip(b"POST / HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n"), Err(ReadError::Bad)));
        assert!(matches!(roundtrip(b"POST / HTTP/1.1\r\nContent-Length: 99999999\r\n\r\n"), Err(ReadError::TooLarge)));
        assert!(matches!(roundtrip(b"garbage\r\n\r\n"), Err(ReadError::Bad)));
        assert_eq!(query_param("app=expense&since=2026-11", "since"), Some("2026-11"));
    }
}
