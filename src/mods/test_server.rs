//! A local HTTP server for the tests that download.

use std::io::{Read, Write};
use std::net::TcpListener;

/// Serves each path's bytes over HTTP, for as long as the tests run.
pub fn serve(files: Vec<(String, Vec<u8>)>) -> String {
    serve_with(|_| files)
}

/// `serve`, with files that may name the server's address.
pub fn serve_with(files: impl FnOnce(&str) -> Vec<(String, Vec<u8>)>) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let files = files(&format!("http://{address}"));
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let mut stream = stream.unwrap();
            let mut request = [0_u8; 4096];
            let read = stream.read(&mut request).unwrap();
            let request = String::from_utf8_lossy(&request[..read]);
            let path = request.split_whitespace().nth(1).unwrap_or("");
            let body = files
                .iter()
                .find(|(served, _)| served == path)
                .map(|(_, bytes)| bytes.clone());
            let response = match &body {
                Some(body) => format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                ),
                None => "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                    .to_string(),
            };
            let _ = stream.write_all(response.as_bytes());
            let _ = stream.write_all(&body.unwrap_or_default());
        }
    });
    format!("http://{address}")
}
