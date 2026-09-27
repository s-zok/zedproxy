//! Dumb mock upstream for manual end-to-end verification (TASK.md §10).
//!
//! Responds to every request with a JSON summary of what it received:
//! method, path, Host header, framing (chunked / content-length) and body.
//!
//! Run: `cargo run --example mock_upstream -- [listen_addr]` (default 127.0.0.1:4001)

use std::io::{Read, Write};
use std::net::TcpListener;
use std::thread;

fn main() {
    let addr = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "127.0.0.1:4001".to_string());
    let listener = TcpListener::bind(&addr).unwrap();
    eprintln!("mock upstream listening on {addr}");
    for stream in listener.incoming() {
        thread::spawn(move || {
            let Ok(mut s) = stream else { return };
            let _ = handle(&mut s);
        });
    }
}

fn handle(s: &mut std::net::TcpStream) -> std::io::Result<()> {
    let mut buf: Vec<u8> = Vec::new();
    let mut tmp = [0u8; 8192];
    let head_end = loop {
        let n = s.read(&mut tmp)?;
        if n == 0 {
            return Ok(());
        }
        buf.extend_from_slice(&tmp[..n]);
        if let Some(p) = find(&buf, b"\r\n\r\n") {
            break p;
        }
        if buf.len() > 1 << 20 {
            return Ok(());
        }
    };
    let head = String::from_utf8_lossy(&buf[..head_end]).to_string();
    let mut lines = head.split("\r\n");
    let request_line = lines.next().unwrap_or("").to_string();
    let mut method = "?".to_string();
    let mut path = "?".to_string();
    if let Some(p) = request_line.split_whitespace().nth(1) {
        path = p.to_string();
    }
    if let Some(m) = request_line.split_whitespace().next() {
        method = m.to_string();
    }
    let mut content_length: usize = 0;
    let mut chunked = false;
    let mut host = String::new();
    for line in lines {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        let name = name.trim().to_ascii_lowercase();
        let value = value.trim();
        match name.as_str() {
            "content-length" => content_length = value.parse().unwrap_or(0),
            "transfer-encoding" => chunked = value.to_ascii_lowercase().contains("chunked"),
            "host" => host = value.to_string(),
            _ => {}
        }
    }

    let mut have = buf.len() - head_end - 4;
    loop {
        if chunked {
            if body_complete_chunked(&buf[head_end + 4..]) {
                break;
            }
        } else if have >= content_length {
            break;
        }
        let n = s.read(&mut tmp)?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&tmp[..n]);
        have = buf.len() - head_end - 4;
    }
    let body_bytes = &buf[head_end + 4..];
    let body = if chunked {
        dechunk(body_bytes)
    } else {
        body_bytes.to_vec()
    };
    let body_str = String::from_utf8_lossy(&body).to_string();

    let resp = serde_json::json!({
        "method": method,
        "path": path,
        "host": host,
        "chunked": chunked,
        "content_length": if chunked { serde_json::Value::Null } else { serde_json::json!(content_length) },
        "body": body_str,
    })
    .to_string();

    let out = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        resp.len(),
        resp
    );
    s.write_all(out.as_bytes())?;
    Ok(())
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

/// chunked body is complete once the terminating 0-size chunk arrived
fn body_complete_chunked(body: &[u8]) -> bool {
    body == b"0\r\n\r\n" || find(body, b"\r\n0\r\n\r\n").is_some()
}

fn dechunk(body: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut rest = body;
    while let Some(line_end) = find(rest, b"\r\n") {
        let size_str = String::from_utf8_lossy(&rest[..line_end]).to_string();
        let size = usize::from_str_radix(size_str.trim().split(';').next().unwrap_or("0"), 16)
            .unwrap_or(0);
        rest = &rest[line_end + 2..];
        if size == 0 {
            break;
        }
        if rest.len() < size {
            break;
        }
        out.extend_from_slice(&rest[..size]);
        rest = &rest[size..];
        if rest.starts_with(b"\r\n") {
            rest = &rest[2..];
        }
    }
    out
}
