use std::collections::HashMap;
use std::env;
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use serde_json::{json, Value};
use sha1::{Digest, Sha1};

const WS_GUID: &str = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";
const MAX_FRAME: usize = 8 * 1024 * 1024;

type Rooms = Arc<Mutex<HashMap<String, Room>>>;

#[derive(Default)]
struct Room {
    clients: HashMap<u64, Client>,
    elements: HashMap<String, Value>,
    next_id: u64,
}

struct Client {
    name: String,
    color: String,
    cursor: Option<(f64, f64)>,
    tx: Sender<Out>,
}

enum Out {
    Text(String),
    Pong(Vec<u8>),
    Close,
}

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

fn read_headers(reader: &mut BufReader<TcpStream>) -> HashMap<String, String> {
    let mut headers = HashMap::new();
    for _ in 0..64 {
        let mut line = String::new();
        if reader.read_line(&mut line).is_err() {
            break;
        }
        let line = line.trim_end_matches(['\r', '\n']);
        if line.is_empty() {
            break;
        }
        if let Some((key, value)) = line.split_once(':') {
            headers.insert(key.trim().to_ascii_lowercase(), value.trim().to_string());
        }
    }
    headers
}

fn room_from_path(path: &str) -> String {
    let room = path
        .split_once('?')
        .map(|(_, query)| query)
        .unwrap_or("")
        .split('&')
        .find_map(|pair| pair.strip_prefix("room="))
        .map(|value| {
            value
                .chars()
                .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
                .take(64)
                .collect::<String>()
        })
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "main".to_string());
    room
}

fn handle(mut stream: TcpStream, root: PathBuf, rooms: Rooms) {
    let mut reader = BufReader::new(match stream.try_clone() {
        Ok(s) => s,
        Err(_) => return,
    });
    let mut first = String::new();
    if reader.read_line(&mut first).is_err() {
        return;
    }
    let mut parts = first.split_whitespace();
    let method = parts.next().unwrap_or("GET").to_string();
    let path = parts.next().unwrap_or("/").to_string();
    let headers = read_headers(&mut reader);

    if method == "GET" && path.starts_with("/ws") {
        ws_session(reader, stream, &headers, room_from_path(&path), rooms);
        return;
    }

    if method != "GET" && method != "HEAD" {
        respond(
            &mut stream,
            "405 Method Not Allowed",
            "text/plain",
            b"method not allowed",
        );
        return;
    }

    if path.split('?').next().unwrap_or("/") == "/" {
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

    match safe_join(&root, &path) {
        Some(file) if file.is_file() => match fs::read(&file) {
            Ok(body) => respond(&mut stream, "200 OK", content_type(&file), &body),
            Err(_) => respond(
                &mut stream,
                "500 Internal Server Error",
                "text/plain",
                b"read error",
            ),
        },
        _ => respond(&mut stream, "404 Not Found", "text/plain", b"not found"),
    }
}

fn ws_session(
    mut reader: BufReader<TcpStream>,
    mut stream: TcpStream,
    headers: &HashMap<String, String>,
    room: String,
    rooms: Rooms,
) {
    let Some(key) = headers.get("sec-websocket-key") else {
        respond(
            &mut stream,
            "400 Bad Request",
            "text/plain",
            b"missing websocket key",
        );
        return;
    };
    if headers
        .get("upgrade")
        .map(|v| !v.eq_ignore_ascii_case("websocket"))
        .unwrap_or(true)
    {
        respond(
            &mut stream,
            "400 Bad Request",
            "text/plain",
            b"expected websocket upgrade",
        );
        return;
    }

    let accept = STANDARD.encode(Sha1::digest(format!("{key}{WS_GUID}").as_bytes()));
    let reply = format!(
        "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {accept}\r\n\r\n"
    );
    if stream.write_all(reply.as_bytes()).is_err() || stream.flush().is_err() {
        return;
    }
    let _ = stream.set_nodelay(true);

    let (tx, rx) = mpsc::channel::<Out>();
    if let Ok(mut writer) = stream.try_clone() {
        thread::spawn(move || writer_loop(&mut writer, rx));
    } else {
        return;
    }

    let (id, init) = {
        let mut guard = match rooms.lock() {
            Ok(g) => g,
            Err(_) => return,
        };
        let room_state = guard.entry(room.clone()).or_default();
        room_state.next_id += 1;
        let id = room_state.next_id;
        let peers = peer_list(room_state, Some(id));
        let mut elements: Vec<Value> = room_state.elements.values().cloned().collect();
        elements.sort_by_key(|element| element.get("id").and_then(Value::as_u64).unwrap_or(0));
        room_state.clients.insert(
            id,
            Client {
                name: String::new(),
                color: String::new(),
                cursor: None,
                tx: tx.clone(),
            },
        );
        (
            id,
            json!({ "t": "init", "clientId": id, "elements": elements, "peers": peers })
                .to_string(),
        )
    };
    let _ = tx.send(Out::Text(init));

    loop {
        match read_frame(&mut reader) {
            Ok(Some((0x1, payload))) => {
                let Ok(text) = String::from_utf8(payload) else {
                    break;
                };
                handle_message(&rooms, &room, id, &text);
            }
            Ok(Some((0x8, _))) | Ok(None) => break,
            Ok(Some((0x9, payload))) => {
                let _ = tx.send(Out::Pong(payload));
            }
            Ok(Some((0xA, _))) => {}
            Ok(Some(_)) => {}
            Err(_) => break,
        }
    }

    let (peers_msg, bye) = {
        let mut guard = match rooms.lock() {
            Ok(g) => g,
            Err(_) => return,
        };
        let Some(room_state) = guard.get_mut(&room) else {
            return;
        };
        room_state.clients.remove(&id);
        (
            json!({ "t": "peers", "peers": peer_list(room_state, None) }).to_string(),
            json!({ "t": "bye", "id": id }).to_string(),
        )
    };
    let _ = tx.send(Out::Close);
    broadcast(&rooms, &room, None, &peers_msg);
    broadcast(&rooms, &room, None, &bye);
}

fn writer_loop(stream: &mut TcpStream, rx: mpsc::Receiver<Out>) {
    let _ = stream.set_write_timeout(Some(Duration::from_secs(10)));
    while let Ok(message) = rx.recv() {
        let sent = match message {
            Out::Text(text) => write_frame(stream, 0x1, text.as_bytes()),
            Out::Pong(payload) => write_frame(stream, 0xA, &payload),
            Out::Close => {
                let _ = write_frame(stream, 0x8, &[]);
                break;
            }
        };
        if sent.is_err() {
            break;
        }
    }
    let _ = stream.shutdown(std::net::Shutdown::Both);
}

fn handle_message(rooms: &Rooms, room: &str, id: u64, text: &str) {
    let Ok(message) = serde_json::from_str::<Value>(text) else {
        return;
    };
    let kind = message.get("t").and_then(Value::as_str).unwrap_or("");
    match kind {
        "hello" => {
            let name = message
                .get("name")
                .and_then(Value::as_str)
                .map(|name| {
                    name.trim()
                        .chars()
                        .filter(|c| !c.is_control())
                        .take(32)
                        .collect::<String>()
                })
                .filter(|name| !name.is_empty())
                .unwrap_or_else(|| format!("Guest {id}"));
            let color = message
                .get("color")
                .and_then(Value::as_str)
                .filter(|color| color.len() == 7 && color.starts_with('#'))
                .unwrap_or("#e03131")
                .to_string();
            let peers_msg = {
                let mut guard = match rooms.lock() {
                    Ok(g) => g,
                    Err(_) => return,
                };
                let Some(room_state) = guard.get_mut(room) else {
                    return;
                };
                let Some(client) = room_state.clients.get_mut(&id) else {
                    return;
                };
                client.name = name;
                client.color = color;
                json!({ "t": "peers", "peers": peer_list(room_state, None) }).to_string()
            };
            broadcast(rooms, room, None, &peers_msg);
        }
        "cursor" => {
            let x = message.get("x").and_then(Value::as_f64);
            let y = message.get("y").and_then(Value::as_f64);
            let cursor = match (x, y) {
                (Some(x), Some(y)) if x.is_finite() && y.is_finite() => Some((x, y)),
                _ => None,
            };
            {
                let mut guard = match rooms.lock() {
                    Ok(g) => g,
                    Err(_) => return,
                };
                let Some(room_state) = guard.get_mut(room) else {
                    return;
                };
                let Some(client) = room_state.clients.get_mut(&id) else {
                    return;
                };
                client.cursor = cursor;
            }
            let msg = match cursor {
                Some((x, y)) => json!({ "t": "cursor", "id": id, "x": x, "y": y }).to_string(),
                None => json!({ "t": "cursor", "id": id, "x": null, "y": null }).to_string(),
            };
            broadcast(rooms, room, Some(id), &msg);
        }
        "patch" => {
            {
                let mut guard = match rooms.lock() {
                    Ok(g) => g,
                    Err(_) => return,
                };
                let Some(room_state) = guard.get_mut(room) else {
                    return;
                };
                if let Some(upserts) = message.get("upsert").and_then(Value::as_array) {
                    if upserts.len() > 10_000 {
                        return;
                    }
                    for element in upserts {
                        let Some(key) = element.get("id").and_then(id_key) else {
                            continue;
                        };
                        room_state.elements.insert(key, element.clone());
                    }
                }
                if let Some(deletes) = message.get("delete").and_then(Value::as_array) {
                    for key in deletes.iter().filter_map(id_key) {
                        room_state.elements.remove(&key);
                    }
                }
            }
            broadcast(rooms, room, Some(id), text);
        }
        _ => {}
    }
}

fn id_key(value: &Value) -> Option<String> {
    if let Some(id) = value.as_u64() {
        Some(id.to_string())
    } else {
        value
            .as_str()
            .filter(|id| !id.is_empty() && id.len() < 32)
            .map(str::to_string)
    }
}

fn peer_list(room: &Room, except: Option<u64>) -> Vec<Value> {
    let mut peers: Vec<Value> = room
        .clients
        .iter()
        .filter(|(id, client)| Some(**id) != except && !client.name.is_empty())
        .map(|(id, client)| {
            json!({
                "id": id,
                "name": client.name,
                "color": client.color,
                "cursor": client.cursor.map(|(x, y)| json!({ "x": x, "y": y }))
            })
        })
        .collect();
    peers.sort_by_key(|peer| peer.get("id").and_then(Value::as_u64).unwrap_or(0));
    peers
}

fn broadcast(rooms: &Rooms, room: &str, except: Option<u64>, text: &str) {
    let senders: Vec<Sender<Out>> = {
        let guard = match rooms.lock() {
            Ok(g) => g,
            Err(_) => return,
        };
        let Some(room_state) = guard.get(room) else {
            return;
        };
        room_state
            .clients
            .iter()
            .filter(|(id, _)| Some(**id) != except)
            .map(|(_, client)| client.tx.clone())
            .collect()
    };
    for sender in senders {
        let _ = sender.send(Out::Text(text.to_string()));
    }
}

fn read_frame(reader: &mut BufReader<TcpStream>) -> std::io::Result<Option<(u8, Vec<u8>)>> {
    let mut header = [0u8; 2];
    if let Err(e) = reader.read_exact(&mut header) {
        if e.kind() == std::io::ErrorKind::UnexpectedEof {
            return Ok(None);
        }
        return Err(e);
    }
    let opcode = header[0] & 0x0f;
    let masked = header[1] & 0x80 != 0;
    let mut len = (header[1] & 0x7f) as usize;
    if len == 126 {
        let mut buf = [0u8; 2];
        reader.read_exact(&mut buf)?;
        len = u16::from_be_bytes(buf) as usize;
    } else if len == 127 {
        let mut buf = [0u8; 8];
        reader.read_exact(&mut buf)?;
        let wide = u64::from_be_bytes(buf);
        len = usize::try_from(wide).unwrap_or(MAX_FRAME + 1);
    }
    if len > MAX_FRAME {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "frame too large",
        ));
    }
    let mask = if masked {
        let mut buf = [0u8; 4];
        reader.read_exact(&mut buf)?;
        Some(buf)
    } else {
        None
    };
    let mut payload = vec![0u8; len];
    reader.read_exact(&mut payload)?;
    if let Some(mask) = mask {
        for (i, byte) in payload.iter_mut().enumerate() {
            *byte ^= mask[i % 4];
        }
    }
    Ok(Some((opcode, payload)))
}

fn write_frame(stream: &mut TcpStream, opcode: u8, payload: &[u8]) -> std::io::Result<()> {
    let mut header = Vec::with_capacity(10);
    header.push(0x80 | opcode);
    let len = payload.len();
    if len < 126 {
        header.push(len as u8);
    } else if len <= u16::MAX as usize {
        header.push(126);
        header.extend_from_slice(&(len as u16).to_be_bytes());
    } else {
        header.push(127);
        header.extend_from_slice(&(len as u64).to_be_bytes());
    }
    stream.write_all(&header)?;
    stream.write_all(payload)?;
    stream.flush()
}

fn main() {
    let port: u16 = env::var("PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(8080);
    let host = env::var("HOST").unwrap_or_else(|_| "127.0.0.1".to_string());
    let root = env::var("FORME_DIST").unwrap_or_else(|_| "dist".to_string());
    let root = PathBuf::from(root);

    if !root.join("index.html").exists() {
        eprintln!(
            "warning: {} has no index.html. Run ./build.sh first.",
            root.display()
        );
    }

    let addr = format!("{host}:{port}");
    let listener = match TcpListener::bind(&addr) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("cannot bind {addr}: {e}");
            std::process::exit(1);
        }
    };

    println!("Forme running at http://{addr}");
    if host == "0.0.0.0" {
        if let Ok(socket) = std::net::UdpSocket::bind("0.0.0.0:0") {
            if socket.connect("8.8.8.8:80").is_ok() {
                if let Ok(local) = socket.local_addr() {
                    println!("On your network: http://{}:{port}", local.ip());
                }
            }
        }
    }

    let rooms: Rooms = Arc::new(Mutex::new(HashMap::new()));
    for stream in listener.incoming().flatten() {
        let root = root.clone();
        let rooms = rooms.clone();
        thread::spawn(move || handle(stream, root, rooms));
    }
}
