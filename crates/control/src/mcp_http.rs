//! Optional MCP-over-HTTP transport (feature `mcp-http`, off by default).
//!
//! Binds to loopback only and requires `Authorization: Bearer <token>` on every request. It is
//! meant to sit behind a tunnel the user manages; the same tiers apply as for `tag mcp` over stdio.
//! Only `POST /mcp` with a single JSON-RPC message is supported (no SSE streaming).

use crate::mcp::{McpOptions, McpSession};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::{Arc, Mutex};

const MAX_BODY: usize = 1024 * 1024;

/// Constant-time byte comparison.
fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// Bind `addr` (must be a loopback address).
pub fn bind(addr: SocketAddr) -> io::Result<TcpListener> {
    if !addr.ip().is_loopback() {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "the HTTP MCP transport only binds to loopback (127.0.0.1 / ::1); use a tunnel for remote access",
        ));
    }
    TcpListener::bind(addr)
}

/// Serve MCP over HTTP on `listener` until it fails. `bearer` must be non-empty.
pub fn serve(listener: TcpListener, bearer: String, opts: McpOptions) -> io::Result<()> {
    if bearer.len() < 16 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "the bearer token must be at least 16 characters",
        ));
    }
    let session = Arc::new(Mutex::new(McpSession::new(opts)));
    let bearer = Arc::new(bearer);
    for stream in listener.incoming() {
        let stream = stream?;
        let session = session.clone();
        let bearer = bearer.clone();
        std::thread::spawn(move || {
            let _ = handle_conn(stream, &bearer, &session);
        });
    }
    Ok(())
}

fn respond(stream: &mut TcpStream, status: &str, body: &str) -> io::Result<()> {
    write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )?;
    stream.flush()
}

fn handle_conn(stream: TcpStream, bearer: &str, session: &Mutex<McpSession>) -> io::Result<()> {
    stream.set_read_timeout(Some(std::time::Duration::from_secs(30)))?;
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut stream = stream;
    let mut request_line = String::new();
    reader.read_line(&mut request_line)?;
    let mut parts = request_line.split_whitespace();
    let (method, path) = (parts.next().unwrap_or(""), parts.next().unwrap_or(""));
    let mut content_length = 0usize;
    let mut auth = String::new();
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line)? == 0 {
            break;
        }
        let line = line.trim_end();
        if line.is_empty() {
            break;
        }
        if let Some((k, v)) = line.split_once(':') {
            match k.trim().to_ascii_lowercase().as_str() {
                "content-length" => content_length = v.trim().parse().unwrap_or(usize::MAX),
                "authorization" => auth = v.trim().to_string(),
                _ => {}
            }
        }
    }
    let expected = format!("Bearer {bearer}");
    if !ct_eq(auth.as_bytes(), expected.as_bytes()) {
        return respond(
            &mut stream,
            "401 Unauthorized",
            r#"{"error":"unauthorized"}"#,
        );
    }
    if path != "/mcp" {
        return respond(&mut stream, "404 Not Found", r#"{"error":"not found"}"#);
    }
    if method != "POST" {
        return respond(
            &mut stream,
            "405 Method Not Allowed",
            r#"{"error":"use POST"}"#,
        );
    }
    if content_length > MAX_BODY {
        return respond(
            &mut stream,
            "413 Payload Too Large",
            r#"{"error":"too large"}"#,
        );
    }
    let mut body = vec![0u8; content_length];
    reader.read_exact(&mut body)?;
    let text = String::from_utf8_lossy(&body);
    let reply = session
        .lock()
        .map_err(|_| io::Error::other("session poisoned"))?
        .handle_text(&text);
    match reply {
        Some(v) => respond(&mut stream, "200 OK", &v.to_string()),
        None => respond(&mut stream, "202 Accepted", ""),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn post(addr: SocketAddr, auth: Option<&str>, body: &str) -> (String, String) {
        let mut s = TcpStream::connect(addr).unwrap();
        let auth = auth
            .map(|a| format!("Authorization: Bearer {a}\r\n"))
            .unwrap_or_default();
        write!(
            s,
            "POST /mcp HTTP/1.1\r\nHost: x\r\n{auth}Content-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        )
        .unwrap();
        let mut out = String::new();
        s.read_to_string(&mut out).unwrap();
        let (head, body) = out.split_once("\r\n\r\n").unwrap();
        (head.lines().next().unwrap().to_string(), body.to_string())
    }

    #[test]
    fn requires_bearer_and_answers_initialize() {
        assert!(bind("0.0.0.0:0".parse().unwrap()).is_err());
        let listener = bind("127.0.0.1:0".parse().unwrap()).unwrap();
        let addr = listener.local_addr().unwrap();
        let token = "0123456789abcdef0123".to_string();
        let opts = McpOptions {
            client_name: "mcp".into(),
            socket: Some("/nonexistent/tag.sock".into()),
            token: None,
        };
        std::thread::spawn(move || serve(listener, token, opts));
        let init = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18"}}"#;
        let (status, _) = post(addr, None, init);
        assert!(status.contains("401"), "{status}");
        let (status, _) = post(addr, Some("wrong-token-wrong-token"), init);
        assert!(status.contains("401"));
        let (status, body) = post(addr, Some("0123456789abcdef0123"), init);
        assert!(status.contains("200"), "{status}");
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["result"]["protocolVersion"], "2025-06-18");
        let (status, _) = post(
            addr,
            Some("0123456789abcdef0123"),
            r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
        );
        assert!(status.contains("202"));
    }
}
