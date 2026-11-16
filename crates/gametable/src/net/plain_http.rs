//! A minimal blocking HTTP/1.1 client for plain `http://` URLs, as luasocket's `socket.http` was
//! used: the public-IP lookup and the UPnP gateway's description and SOAP calls. Run it on a
//! worker thread.

use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;

/// A response's status code and body.
#[derive(Debug, Clone, PartialEq)]
pub struct HttpResponse {
    pub status: u16,
    pub body: Vec<u8>,
}

/// `http://host[:port]/path` -> (host, port, path).
pub fn split_http_url(url: &str) -> Option<(String, u16, String)> {
    let rest = url.strip_prefix("http://")?;
    let (authority, path) = match rest.find('/') {
        Some(slash) => (&rest[..slash], &rest[slash..]),
        None => (rest, "/"),
    };
    let (host, port) = match authority.rsplit_once(':') {
        Some((host, port)) if !host.ends_with(']') || authority.starts_with('[') => (host, port.parse().ok()?),
        _ => (authority, 80),
    };
    let host = host.trim_start_matches('[').trim_end_matches(']');
    Some((host.to_string(), port, path.to_string()))
}

/// Decodes a `Transfer-Encoding: chunked` body.
fn decode_chunked(mut data: &[u8]) -> Vec<u8> {
    let mut body = Vec::new();
    loop {
        let Some(line_end) = data.windows(2).position(|window| window == b"\r\n") else { break };
        let size_text = String::from_utf8_lossy(&data[..line_end]);
        let size = usize::from_str_radix(size_text.split(';').next().unwrap_or("").trim(), 16).unwrap_or(0);
        data = &data[line_end + 2..];
        if size == 0 || data.len() < size {
            body.extend_from_slice(&data[..size.min(data.len())]);
            break;
        }
        body.extend_from_slice(&data[..size]);
        data = data.get(size + 2..).unwrap_or(&[]);
    }
    body
}

/// Sends one request and reads the whole response. `timeout` covers connect, write and read.
pub fn request(
    method: &str,
    url: &str,
    headers: &[(&str, String)],
    body: &[u8],
    timeout: Duration,
) -> Result<HttpResponse, String> {
    let (host, port, path) = split_http_url(url).ok_or("not an http:// URL")?;
    let address =
        (host.as_str(), port).to_socket_addrs().map_err(|error| error.to_string())?.next().ok_or("host not found")?;
    let mut stream = TcpStream::connect_timeout(&address, timeout).map_err(|error| error.to_string())?;
    stream.set_read_timeout(Some(timeout)).map_err(|error| error.to_string())?;
    stream.set_write_timeout(Some(timeout)).map_err(|error| error.to_string())?;
    let host_header = if port == 80 { host.clone() } else { format!("{host}:{port}") };
    let mut request = format!("{method} {path} HTTP/1.1\r\nhost: {host_header}\r\nconnection: close\r\n");
    for (name, value) in headers {
        request.push_str(&format!("{name}: {value}\r\n"));
    }
    if !body.is_empty() || method == "POST" {
        request.push_str(&format!("content-length: {}\r\n", body.len()));
    }
    request.push_str("\r\n");
    stream.write_all(request.as_bytes()).map_err(|error| error.to_string())?;
    stream.write_all(body).map_err(|error| error.to_string())?;
    let mut response = Vec::new();
    stream
        .read_to_end(&mut response)
        .or_else(|error| if response.is_empty() { Err(error.to_string()) } else { Ok(0) })?;
    let header_end = response.windows(4).position(|window| window == b"\r\n\r\n").ok_or("malformed response")?;
    let head = String::from_utf8_lossy(&response[..header_end]).to_string();
    let status = head
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|code| code.parse().ok())
        .ok_or("malformed status line")?;
    let raw_body = &response[header_end + 4..];
    let chunked = head
        .lines()
        .any(|line| line.to_lowercase().starts_with("transfer-encoding:") && line.to_lowercase().contains("chunked"));
    let body = if chunked { decode_chunked(raw_body) } else { raw_body.to_vec() };
    Ok(HttpResponse { status, body })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls_split() {
        assert_eq!(split_http_url("http://api.ipify.org"), Some(("api.ipify.org".into(), 80, "/".into())));
        assert_eq!(
            split_http_url("http://192.168.1.1:5000/rootDesc.xml"),
            Some(("192.168.1.1".into(), 5000, "/rootDesc.xml".into()))
        );
        assert_eq!(split_http_url("https://x"), None);
    }

    #[test]
    fn chunked_bodies_decode() {
        assert_eq!(decode_chunked(b"5\r\nhello\r\n6;x=1\r\n world\r\n0\r\n\r\n"), b"hello world");
    }

    #[test]
    fn requests_a_local_server() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0u8; 1024];
            let read = stream.read(&mut request).unwrap();
            let text = String::from_utf8_lossy(&request[..read]).to_string();
            stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 7\r\n\r\n1.2.3.4").unwrap();
            text
        });
        let response =
            request("GET", &format!("http://127.0.0.1:{port}/ip"), &[], &[], Duration::from_secs(2)).unwrap();
        assert_eq!((response.status, response.body.as_slice()), (200, b"1.2.3.4".as_slice()));
        assert!(server.join().unwrap().starts_with("GET /ip HTTP/1.1\r\n"));
    }
}
