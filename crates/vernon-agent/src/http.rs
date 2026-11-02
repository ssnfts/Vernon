//! A small HTTP/1.1 client for plain `http://` endpoints (local model
//! servers and gateways). No TLS: pure Rust, no C build scripts.

use std::io::{ErrorKind, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, PartialEq)]
pub struct Url {
    pub host: String,
    pub port: u16,
    pub path: String,
}

pub fn parse_url(url: &str) -> Result<Url, String> {
    let url = url.trim();
    if url.starts_with("https://") {
        return Err("https:// endpoints are not supported (Vernon links no TLS stack). Point the agent at a local http:// gateway or proxy, e.g. LiteLLM, or at an Ollama / LM Studio / llama.cpp server".into());
    }
    let rest = url.strip_prefix("http://").ok_or_else(|| format!("endpoint must start with http:// (got '{url}')"))?;
    let (authority, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, "/"),
    };
    if authority.is_empty() {
        return Err("endpoint has no host".into());
    }
    let (host, port) = match authority.rsplit_once(':') {
        Some((h, p)) if !h.ends_with(']') || h.starts_with('[') => {
            (h.trim_matches(|c| c == '[' || c == ']'), p.parse::<u16>().map_err(|_| format!("bad port '{p}'"))?)
        }
        _ => (authority, 80),
    };
    Ok(Url { host: host.to_string(), port, path: path.to_string() })
}

#[derive(Debug)]
pub struct Response {
    pub status: u16,
    pub body: Vec<u8>,
}

/// Buffered socket reader that honours the overall deadline and the cancel
/// flag (socket read timeouts are short slices).
struct Reader<'a> {
    stream: TcpStream,
    buf: Vec<u8>,
    deadline: Instant,
    cancel: Option<&'a AtomicBool>,
    eof: bool,
}

impl Reader<'_> {
    fn fill(&mut self) -> Result<(), String> {
        loop {
            if self.cancel.is_some_and(|c| c.load(Ordering::Relaxed)) {
                return Err("cancelled".into());
            }
            if Instant::now() >= self.deadline {
                return Err("request timed out".into());
            }
            let mut chunk = [0u8; 8192];
            match self.stream.read(&mut chunk) {
                Ok(0) => {
                    self.eof = true;
                    return Ok(());
                }
                Ok(n) => {
                    self.buf.extend_from_slice(&chunk[..n]);
                    return Ok(());
                }
                Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut | ErrorKind::Interrupted) => continue,
                Err(e) => return Err(format!("read failed: {e}")),
            }
        }
    }

    fn take_line(&mut self) -> Result<String, String> {
        loop {
            if let Some(i) = self.buf.windows(2).position(|w| w == b"\r\n") {
                let line = String::from_utf8_lossy(&self.buf[..i]).into_owned();
                self.buf.drain(..i + 2);
                return Ok(line);
            }
            if self.eof {
                return Err("connection closed mid-response".into());
            }
            self.fill()?;
        }
    }

    fn take_exact(&mut self, n: usize) -> Result<Vec<u8>, String> {
        while self.buf.len() < n {
            if self.eof {
                return Err("connection closed before the body was complete".into());
            }
            self.fill()?;
        }
        Ok(self.buf.drain(..n).collect())
    }
}

/// POST `body` (JSON) to `url`. `timeout` bounds the whole exchange.
pub fn post_json(url: &str, headers: &[(String, String)], body: &[u8], timeout: Duration, cancel: Option<&AtomicBool>) -> Result<Response, String> {
    let u = parse_url(url)?;
    let deadline = Instant::now() + timeout;
    let addr = (u.host.as_str(), u.port)
        .to_socket_addrs()
        .map_err(|e| format!("cannot resolve {}: {e}", u.host))?
        .next()
        .ok_or_else(|| format!("no address for {}", u.host))?;
    let stream = TcpStream::connect_timeout(&addr, timeout.min(Duration::from_secs(10)))
        .map_err(|e| format!("cannot connect to {}:{} ({e}). Is the model server running?", u.host, u.port))?;
    stream.set_read_timeout(Some(Duration::from_millis(100))).map_err(|e| e.to_string())?;
    stream.set_write_timeout(Some(timeout)).map_err(|e| e.to_string())?;
    let mut req = format!(
        "POST {} HTTP/1.1\r\nHost: {}:{}\r\nConnection: close\r\nAccept: application/json\r\nContent-Type: application/json\r\nContent-Length: {}\r\n",
        u.path,
        u.host,
        u.port,
        body.len()
    );
    for (k, v) in headers {
        if k.contains(['\r', '\n']) || v.contains(['\r', '\n']) {
            return Err("header contains a line break".into());
        }
        req.push_str(&format!("{k}: {v}\r\n"));
    }
    req.push_str("\r\n");
    let mut w = &stream;
    w.write_all(req.as_bytes()).and_then(|_| w.write_all(body)).map_err(|e| format!("send failed: {e}"))?;
    read_response(stream, deadline, cancel)
}

fn read_response(stream: TcpStream, deadline: Instant, cancel: Option<&AtomicBool>) -> Result<Response, String> {
    const MAX_BODY: usize = 64 << 20;
    let mut r = Reader { stream, buf: Vec::new(), deadline, cancel, eof: false };
    let status_line = r.take_line()?;
    let status: u16 = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| format!("bad status line '{status_line}'"))?;
    let (mut len, mut chunked) = (None, false);
    loop {
        let line = r.take_line()?;
        if line.is_empty() {
            break;
        }
        if let Some((k, v)) = line.split_once(':') {
            let (k, v) = (k.trim().to_ascii_lowercase(), v.trim());
            if k == "content-length" {
                len = v.parse::<usize>().ok();
            } else if k == "transfer-encoding" && v.to_ascii_lowercase().contains("chunked") {
                chunked = true;
            }
        }
    }
    let body = if chunked {
        let mut body = Vec::new();
        loop {
            let line = r.take_line()?;
            let size = usize::from_str_radix(line.split(';').next().unwrap_or("").trim(), 16).map_err(|_| format!("bad chunk size '{line}'"))?;
            if size == 0 {
                break; // trailers ignored
            }
            if body.len() + size > MAX_BODY {
                return Err("response too large".into());
            }
            body.extend(r.take_exact(size)?);
            r.take_exact(2)?; // CRLF after the chunk
        }
        body
    } else if let Some(n) = len {
        if n > MAX_BODY {
            return Err("response too large".into());
        }
        r.take_exact(n)?
    } else {
        while !r.eof {
            if r.buf.len() > MAX_BODY {
                return Err("response too large".into());
            }
            r.fill()?;
        }
        std::mem::take(&mut r.buf)
    };
    Ok(Response { status, body })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    /// One-shot server: reads the request, waits `delay`, writes `reply`.
    fn serve(reply: &'static [u8], delay: Duration) -> u16 {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        std::thread::spawn(move || {
            let (mut s, _) = l.accept().unwrap();
            // Read the whole request first: closing with unread data resets the connection.
            let mut req = Vec::new();
            let mut buf = [0u8; 4096];
            while !req.ends_with(b"\r\n\r\n{}") {
                match s.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => req.extend_from_slice(&buf[..n]),
                }
            }
            std::thread::sleep(delay);
            let _ = s.write_all(reply);
        });
        port
    }

    fn post(port: u16, timeout: Duration) -> Result<Response, String> {
        post_json(&format!("http://127.0.0.1:{port}/v1/chat/completions"), &[], b"{}", timeout, None)
    }

    #[test]
    fn urls() {
        assert_eq!(parse_url("http://localhost:11434/v1").unwrap(), Url { host: "localhost".into(), port: 11434, path: "/v1".into() });
        assert_eq!(parse_url("http://example").unwrap().port, 80);
        assert!(parse_url("https://api.example.com").unwrap_err().contains("gateway"));
        assert!(parse_url("ftp://x").is_err());
    }

    #[test]
    fn content_length_body() {
        let p = serve(b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nhello", Duration::ZERO);
        let r = post(p, Duration::from_secs(5)).unwrap();
        assert_eq!((r.status, r.body.as_slice()), (200, &b"hello"[..]));
    }

    #[test]
    fn chunked_body() {
        let p = serve(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhello\r\n6;ext=1\r\n world\r\n0\r\n\r\n", Duration::ZERO);
        let r = post(p, Duration::from_secs(5)).unwrap();
        assert_eq!(r.body, b"hello world");
    }

    #[test]
    fn body_until_close_and_error_status() {
        let p = serve(b"HTTP/1.1 500 Oops\r\n\r\nboom", Duration::ZERO);
        let r = post(p, Duration::from_secs(5)).unwrap();
        assert_eq!((r.status, r.body.as_slice()), (500, &b"boom"[..]));
    }

    #[test]
    fn times_out_on_a_silent_server() {
        let p = serve(b"HTTP/1.1 200 OK\r\nContent-Length: 1\r\n\r\nx", Duration::from_secs(3));
        let t = Instant::now();
        let e = post(p, Duration::from_millis(400)).unwrap_err();
        assert!(e.contains("timed out"), "{e}");
        assert!(t.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn cancel_interrupts_a_wait() {
        let p = serve(b"HTTP/1.1 200 OK\r\nContent-Length: 1\r\n\r\nx", Duration::from_secs(3));
        let flag = AtomicBool::new(true);
        let e = post_json(&format!("http://127.0.0.1:{p}/"), &[], b"{}", Duration::from_secs(5), Some(&flag)).unwrap_err();
        assert_eq!(e, "cancelled");
    }

    #[test]
    fn truncated_body_is_an_error() {
        let p = serve(b"HTTP/1.1 200 OK\r\nContent-Length: 10\r\n\r\nabc", Duration::ZERO);
        assert!(post(p, Duration::from_secs(2)).is_err());
    }
}
