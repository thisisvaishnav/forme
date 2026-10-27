use std::env;
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::thread;

fn content_type(path: &Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()).unwrap_or("") {
        "html" => "text/html; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "js" => "text/javascript; charset=utf-8",
        "wasm" => "application/wasm",
        "json" => "application/json; charset=utf-8",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "ico" => "image/x-icon",
        "woff2" => "font/woff2",
        _ => "application/octet-stream",
    }
}

fn respond(stream: &mut TcpStream, status: &str, ctype: &str, body: &[u8]) {
    let head = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nCache-Control: no-cache\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let _ = stream.write_all(head.as_bytes());
    let _ = stream.write_all(body);
    let _ = stream.flush();
}

fn safe_join(root: &Path, request_path: &str) -> Option<PathBuf> {
    let clean = request_path.split(['?', '#']).next().unwrap_or("/");
    let clean = clean.trim_start_matches('/');
    let candidate = root.join(clean);
    let joined = candidate.canonicalize().ok()?;
    let root = root.canonicalize().ok()?;
    if joined.starts_with(&root) {
        Some(joined)
    } else {
        None
    }
}

fn handle(mut stream: TcpStream, root: PathBuf) {
    let mut reader = BufReader::new(match stream.try_clone() {
        Ok(s) => s,
        Err(_) => return,
    });
    let mut first = String::new();
    if reader.read_line(&mut first).is_err() {
        return;
    }
    let mut parts = first.split_whitespace();
    let method = parts.next().unwrap_or("GET");
    let path = parts.next().unwrap_or("/");
    if method != "GET" && method != "HEAD" {
        respond(&mut stream, "405 Method Not Allowed", "text/plain", b"method not allowed");
        return;
    }

    if path == "/" {
        let index = root.join("index.html");
        if let Ok(body) = fs::read(&index) {
            respond(&mut stream, "200 OK", content_type(&index), &body);
        } else {
            respond(
                &mut stream,
                "500 Internal Server Error",
                "text/plain",
                b"dist/index.html missing. Run ./build.sh first.",
            );
        }
        return;
    }

    match safe_join(&root, path) {
        Some(file) if file.is_file() => match fs::read(&file) {
            Ok(body) => respond(&mut stream, "200 OK", content_type(&file), &body),
            Err(_) => respond(&mut stream, "500 Internal Server Error", "text/plain", b"read error"),
        },
        _ => {
            let mut sink = [0u8; 4096];
            let _ = stream.read(&mut sink);
            respond(&mut stream, "404 Not Found", "text/plain", b"not found");
        }
    }
}

fn main() {
    let port: u16 = env::var("PORT").ok().and_then(|p| p.parse().ok()).unwrap_or(8080);
    let root = env::var("FORME_DIST").unwrap_or_else(|_| "dist".to_string());
    let root = PathBuf::from(root);

    if !root.join("index.html").exists() {
        eprintln!("warning: {} has no index.html. Run ./build.sh first.", root.display());
    }

    let addr = format!("127.0.0.1:{port}");
    let listener = match TcpListener::bind(&addr) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("cannot bind {addr}: {e}");
            std::process::exit(1);
        }
    };

    println!("Forme running at http://{addr}");

    for stream in listener.incoming().flatten() {
        let root = root.clone();
        thread::spawn(move || handle(stream, root));
    }
}
