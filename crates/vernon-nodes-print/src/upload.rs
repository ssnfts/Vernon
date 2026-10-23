//! Printer-host upload over plain HTTP (file manager API of a Klipper host):
//! SHA-256, a minimal HTTP/1.1 client and the multipart upload with checksum.
//! No dependencies; TLS is not supported (use a local http address or a proxy).

use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::path::Path;
use std::time::Duration;

use serde_json::Value;

// ----------------------------------------------------------------- SHA-256

const K: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
    0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
    0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
    0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
];

fn compress(h: &mut [u32; 8], block: &[u8]) {
    let mut w = [0u32; 64];
    for (i, c) in block.chunks_exact(4).enumerate() {
        w[i] = u32::from_be_bytes([c[0], c[1], c[2], c[3]]);
    }
    for i in 16..64 {
        let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
        let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
        w[i] = w[i - 16].wrapping_add(s0).wrapping_add(w[i - 7]).wrapping_add(s1);
    }
    let mut v = *h;
    for i in 0..64 {
        let s1 = v[4].rotate_right(6) ^ v[4].rotate_right(11) ^ v[4].rotate_right(25);
        let ch = (v[4] & v[5]) ^ (!v[4] & v[6]);
        let t1 = v[7].wrapping_add(s1).wrapping_add(ch).wrapping_add(K[i]).wrapping_add(w[i]);
        let s0 = v[0].rotate_right(2) ^ v[0].rotate_right(13) ^ v[0].rotate_right(22);
        let maj = (v[0] & v[1]) ^ (v[0] & v[2]) ^ (v[1] & v[2]);
        let t2 = s0.wrapping_add(maj);
        v = [t1.wrapping_add(t2), v[0], v[1], v[2], v[3].wrapping_add(t1), v[4], v[5], v[6]];
    }
    for i in 0..8 {
        h[i] = h[i].wrapping_add(v[i]);
    }
}

pub fn sha256(data: &[u8]) -> [u8; 32] {
    let mut h: [u32; 8] = [0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19];
    let mut blocks = data.chunks_exact(64);
    for b in &mut blocks {
        compress(&mut h, b);
    }
    let mut tail = blocks.remainder().to_vec();
    tail.push(0x80);
    while tail.len() % 64 != 56 {
        tail.push(0);
    }
    tail.extend_from_slice(&(data.len() as u64 * 8).to_be_bytes());
    for b in tail.chunks_exact(64) {
        compress(&mut h, b);
    }
    let mut out = [0u8; 32];
    for (i, v) in h.iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&v.to_be_bytes());
    }
    out
}

pub fn sha256_hex(data: &[u8]) -> String {
    sha256(data).iter().map(|b| format!("{b:02x}")).collect()
}

// -------------------------------------------------------------------- HTTP

#[derive(Debug, Clone, PartialEq)]
pub struct Url {
    pub host: String,
    pub port: u16,
    /// Path prefix without a trailing slash.
    pub base: String,
}

pub fn parse_url(u: &str) -> Result<Url, String> {
    let u = u.trim();
    if u.starts_with("https://") {
        return Err("https is not supported; use an http address on the local network or put a proxy in front".into());
    }
    let rest = u.strip_prefix("http://").unwrap_or(u);
    let (authority, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, ""),
    };
    if authority.is_empty() {
        return Err("the host address is empty".into());
    }
    let (host, port) = match authority.rsplit_once(':') {
        Some((h, p)) => (h, p.parse::<u16>().map_err(|_| format!("bad port '{p}'"))?),
        None => (authority, 80),
    };
    Ok(Url { host: host.to_string(), port, base: path.trim_end_matches('/').to_string() })
}

#[derive(Debug, Clone)]
pub struct Response {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Response {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }

    pub fn json(&self) -> Option<Value> {
        serde_json::from_slice(&self.body).ok()
    }
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

fn decode_chunked(mut b: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    while let Some(i) = find(b, b"\r\n") {
        let size = usize::from_str_radix(String::from_utf8_lossy(&b[..i]).split(';').next().unwrap_or("").trim(), 16).unwrap_or(0);
        b = &b[i + 2..];
        if size == 0 || b.len() < size {
            break;
        }
        out.extend_from_slice(&b[..size]);
        b = &b[size..];
        b = b.strip_prefix(b"\r\n").unwrap_or(b);
    }
    out
}

pub fn parse_response(raw: &[u8]) -> Result<Response, String> {
    let split = find(raw, b"\r\n\r\n").ok_or("the host sent no complete response")?;
    let head = String::from_utf8_lossy(&raw[..split]).to_string();
    let mut lines = head.lines();
    let status = lines
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|s| s.parse::<u16>().ok())
        .ok_or("the response has no status line")?;
    let headers: Vec<(String, String)> = lines.filter_map(|l| l.split_once(':')).map(|(k, v)| (k.trim().to_string(), v.trim().to_string())).collect();
    let mut body = raw[split + 4..].to_vec();
    let get = |n: &str| headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(n)).map(|(_, v)| v.to_ascii_lowercase());
    if get("transfer-encoding").is_some_and(|v| v.contains("chunked")) {
        body = decode_chunked(&body);
    } else if let Some(len) = get("content-length").and_then(|v| v.parse::<usize>().ok()) {
        body.truncate(len);
    }
    Ok(Response { status, headers, body })
}

fn send(url: &Url, request: &[u8], timeout: Duration) -> Result<Response, String> {
    let addr = (url.host.as_str(), url.port).to_socket_addrs().map_err(|e| format!("cannot resolve {}: {e}", url.host))?.next().ok_or("no address for the host")?;
    let mut s = TcpStream::connect_timeout(&addr, timeout).map_err(|e| format!("cannot connect to {}:{}: {e}", url.host, url.port))?;
    s.set_read_timeout(Some(timeout)).ok();
    s.set_write_timeout(Some(timeout)).ok();
    s.write_all(request).map_err(|e| format!("send failed: {e}"))?;
    let mut raw = Vec::new();
    s.read_to_end(&mut raw).map_err(|e| format!("receive failed: {e}"))?;
    parse_response(&raw)
}

fn pct_encode(s: &str) -> String {
    s.bytes()
        .map(|b| if b.is_ascii_alphanumeric() || b"-_.~/".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") })
        .collect()
}

pub fn build_multipart(boundary: &str, fields: &[(&str, &str)], filename: &str, data: &[u8]) -> Vec<u8> {
    let mut b = Vec::new();
    for (k, v) in fields {
        b.extend_from_slice(format!("--{boundary}\r\nContent-Disposition: form-data; name=\"{k}\"\r\n\r\n{v}\r\n").as_bytes());
    }
    b.extend_from_slice(format!("--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{filename}\"\r\nContent-Type: application/octet-stream\r\n\r\n").as_bytes());
    b.extend_from_slice(data);
    b.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    b
}

#[derive(Debug, Clone)]
pub struct UploadParams<'a> {
    pub api_key: &'a str,
    /// `gcodes` or `config`.
    pub root: &'a str,
    /// Sub-folder, created by the host when missing.
    pub path: &'a str,
    pub print: bool,
    /// Replace the computed checksum (used to test the host's rejection).
    pub checksum_override: Option<&'a str>,
    pub timeout: Duration,
}

#[derive(Debug, Clone)]
pub struct UploadOutcome {
    pub status: u16,
    pub item_path: Option<String>,
    pub item_size: Option<u64>,
    pub print_started: bool,
    pub body: Value,
    pub local_size: u64,
    pub checksum: String,
    /// True for HTTP 201 whose reported size equals the local size.
    pub verified: bool,
}

/// Upload `file` to the host at `base_url`. Transport failures are errors; a
/// refusal by the host (for example 422) is returned in `status`.
pub fn upload(base_url: &str, file: &Path, p: &UploadParams) -> Result<UploadOutcome, String> {
    let url = parse_url(base_url)?;
    let data = std::fs::read(file).map_err(|e| format!("cannot read {}: {e}", file.display()))?;
    let name = file.file_name().and_then(|n| n.to_str()).ok_or("the file has no name")?;
    let checksum = p.checksum_override.map(str::to_string).unwrap_or_else(|| sha256_hex(&data));
    let mut fields: Vec<(&str, &str)> = vec![("root", p.root)];
    if !p.path.is_empty() {
        fields.push(("path", p.path));
    }
    fields.push(("checksum", &checksum));
    fields.push(("print", if p.print { "true" } else { "false" }));
    let boundary = format!("----vernon{}", &sha256_hex(&data)[..16]);
    let body = build_multipart(&boundary, &fields, name, &data);
    let mut req = format!(
        "POST {}/server/files/upload HTTP/1.1\r\nHost: {}:{}\r\nContent-Type: multipart/form-data; boundary={boundary}\r\nContent-Length: {}\r\nConnection: close\r\n",
        url.base,
        url.host,
        url.port,
        body.len()
    );
    if !p.api_key.is_empty() {
        req.push_str(&format!("X-Api-Key: {}\r\n", p.api_key));
    }
    req.push_str("\r\n");
    let mut bytes = req.into_bytes();
    bytes.extend_from_slice(&body);
    let resp = send(&url, &bytes, p.timeout)?;
    let json = resp.json().unwrap_or(Value::Null);
    let item = json.get("item");
    let item_size = item.and_then(|i| i.get("size")).and_then(Value::as_u64);
    let item_path = item.and_then(|i| i.get("path")).and_then(Value::as_str).map(str::to_string);
    let print_started = json.get("print_started").and_then(Value::as_bool).unwrap_or(false);
    let local_size = data.len() as u64;
    Ok(UploadOutcome {
        status: resp.status,
        verified: resp.status == 201 && item_size == Some(local_size),
        item_path,
        item_size,
        print_started,
        body: json,
        local_size,
        checksum,
    })
}

/// `GET /server/files/metadata?filename=...`; returns the `result` object.
pub fn fetch_metadata(base_url: &str, filename: &str, api_key: &str, timeout: Duration) -> Result<Value, String> {
    let url = parse_url(base_url)?;
    let mut req = format!(
        "GET {}/server/files/metadata?filename={} HTTP/1.1\r\nHost: {}:{}\r\nConnection: close\r\n",
        url.base,
        pct_encode(filename),
        url.host,
        url.port
    );
    if !api_key.is_empty() {
        req.push_str(&format!("X-Api-Key: {api_key}\r\n"));
    }
    req.push_str("\r\n");
    let resp = send(&url, req.as_bytes(), timeout)?;
    if resp.status != 200 {
        return Err(format!("metadata request answered with status {}", resp.status));
    }
    let v = resp.json().ok_or("metadata answer is not JSON")?;
    Ok(v.get("result").cloned().unwrap_or(v))
}

#[cfg(test)]
pub mod mock {
    use super::*;
    use std::net::TcpListener;

    fn read_request(s: &mut TcpStream) -> Vec<u8> {
        let mut buf = Vec::new();
        let mut tmp = [0u8; 8192];
        loop {
            let n = s.read(&mut tmp).unwrap_or(0);
            if n == 0 {
                return buf;
            }
            buf.extend_from_slice(&tmp[..n]);
            if let Some(h) = find(&buf, b"\r\n\r\n") {
                let head = String::from_utf8_lossy(&buf[..h]).to_ascii_lowercase();
                let len = head.lines().find_map(|l| l.strip_prefix("content-length:")).and_then(|v| v.trim().parse::<usize>().ok()).unwrap_or(0);
                if buf.len() >= h + 4 + len {
                    return buf;
                }
            }
        }
    }

    fn field(body: &[u8], name: &str) -> Option<String> {
        let key = format!("name=\"{name}\"\r\n\r\n");
        let i = find(body, key.as_bytes())? + key.len();
        let j = find(&body[i..], b"\r\n")?;
        Some(String::from_utf8_lossy(&body[i..i + j]).to_string())
    }

    /// Serve `n` connections on a loopback port; returns the base url and the join handle.
    pub fn serve(n: usize, estimated_time: f64) -> (String, std::thread::JoinHandle<Vec<String>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
        let h = std::thread::spawn(move || {
            let mut log = Vec::new();
            for _ in 0..n {
                let (mut s, _) = listener.accept().unwrap();
                let req = read_request(&mut s);
                let head_end = find(&req, b"\r\n\r\n").unwrap_or(req.len());
                let line = String::from_utf8_lossy(&req[..head_end]).lines().next().unwrap_or("").to_string();
                log.push(line.clone());
                let reply = if line.starts_with("POST /server/files/upload") {
                    let body = &req[head_end + 4..];
                    let sum = field(body, "checksum").unwrap_or_default();
                    let fstart = find(body, b"filename=\"").and_then(|i| find(&body[i..], b"\r\n\r\n").map(|j| i + j + 4));
                    let data = match (fstart, find_last(body, b"\r\n------")) {
                        (Some(a), Some(b)) if b >= a => body[a..b].to_vec(),
                        _ => Vec::new(),
                    };
                    if sum != sha256_hex(&data) {
                        let b = r#"{"error":{"code":422,"message":"checksum mismatch"}}"#;
                        format!("HTTP/1.1 422 Unprocessable Entity\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{b}", b.len())
                    } else {
                        let print_started = field(body, "print").as_deref() == Some("true");
                        let b = format!(
                            r#"{{"item":{{"path":"{}","root":"gcodes","modified":0,"size":{},"permissions":"rw"}},"print_started":{print_started},"print_queued":false,"action":"create_file"}}"#,
                            "job.gcode",
                            data.len()
                        );
                        format!("HTTP/1.1 201 Created\r\nLocation: /server/files/gcodes/job.gcode\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{b}", b.len())
                    }
                } else if line.starts_with("GET /server/files/metadata") {
                    let b = format!(r#"{{"result":{{"estimated_time":{estimated_time},"filament_total":1234.5,"slicer":"mock"}}}}"#);
                    format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{b}", b.len())
                } else {
                    "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_string()
                };
                let _ = s.write_all(reply.as_bytes());
            }
            log
        });
        (url, h)
    }

    fn find_last(hay: &[u8], needle: &[u8]) -> Option<usize> {
        hay.windows(needle.len()).rposition(|w| w == needle)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_vectors() {
        assert_eq!(sha256_hex(b""), "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
        assert_eq!(sha256_hex(b"abc"), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
        let long = vec![b'a'; 1000];
        assert_eq!(sha256_hex(&long), "41edece42d63e8d9bf515a9ba6932e1c20cbc9f5a5d134645adb5db1b9737ea3");
    }

    #[test]
    fn url_parsing() {
        assert_eq!(parse_url("http://printer.local:7125").unwrap(), Url { host: "printer.local".into(), port: 7125, base: String::new() });
        assert_eq!(parse_url("192.168.1.5/api/").unwrap().port, 80);
        assert!(parse_url("https://x").is_err());
        assert!(parse_url("http://h:notaport").is_err());
    }

    #[test]
    fn response_parsing_content_length_and_chunked() {
        let r = parse_response(b"HTTP/1.1 201 Created\r\nContent-Length: 5\r\nLocation: /x\r\n\r\nhello-extra").unwrap();
        assert_eq!((r.status, r.body.as_slice(), r.header("location")), (201, b"hello".as_slice(), Some("/x")));
        let c = parse_response(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n3\r\nabc\r\n2\r\nde\r\n0\r\n\r\n").unwrap();
        assert_eq!(c.body, b"abcde");
        assert!(parse_response(b"garbage").is_err());
    }

    #[test]
    fn multipart_shape() {
        let b = build_multipart("BB", &[("root", "gcodes")], "a.gcode", b"G1 X1\n");
        let s = String::from_utf8(b).unwrap();
        assert!(s.starts_with("--BB\r\nContent-Disposition: form-data; name=\"root\"\r\n\r\ngcodes\r\n"));
        assert!(s.contains("filename=\"a.gcode\"") && s.ends_with("\r\n--BB--\r\n"));
    }

    fn temp_file(name: &str, data: &[u8]) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("vernon-print-upload-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let f = d.join("job.gcode");
        std::fs::write(&f, data).unwrap();
        f
    }

    #[test]
    fn upload_against_mock_host() {
        let file = temp_file("ok", b"G1 X1 E1\nG1 X2 E1\n");
        let (url, h) = mock::serve(2, 90.0);
        let p = UploadParams { api_key: "k", root: "gcodes", path: "vernon", print: false, checksum_override: None, timeout: Duration::from_secs(5) };
        let o = upload(&url, &file, &p).unwrap();
        assert_eq!(o.status, 201);
        assert!(o.verified);
        assert_eq!(o.item_size, Some(o.local_size));
        assert!(!o.print_started);
        let m = fetch_metadata(&url, "vernon/job.gcode", "k", Duration::from_secs(5)).unwrap();
        assert_eq!(m["estimated_time"], 90.0);
        let log = h.join().unwrap();
        assert!(log[0].starts_with("POST /server/files/upload"));
        assert!(log[1].contains("filename=vernon/job.gcode"));
    }

    #[test]
    fn wrong_checksum_is_refused() {
        let file = temp_file("bad", b"G1 X1 E1\n");
        let (url, h) = mock::serve(1, 0.0);
        let p = UploadParams { api_key: "", root: "gcodes", path: "", print: false, checksum_override: Some("deadbeef"), timeout: Duration::from_secs(5) };
        let o = upload(&url, &file, &p).unwrap();
        assert_eq!(o.status, 422);
        assert!(!o.verified && o.item_size.is_none());
        h.join().unwrap();
    }

    #[test]
    fn unreachable_host_is_an_error() {
        let file = temp_file("down", b"x");
        let p = UploadParams { api_key: "", root: "gcodes", path: "", print: false, checksum_override: None, timeout: Duration::from_millis(300) };
        assert!(upload("http://127.0.0.1:1", &file, &p).is_err());
    }
}
