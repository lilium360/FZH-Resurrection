//! The backend's account API on the backend port of the server address (docs/launcher-contract.md,
//! "Backend account API"): registration, the registered casing of a name, the play grant; and the
//! relay list with its status ("Relay status"). Plain HTTP/1.1, one request per connection.

use crate::error::{self, BackendError, Res};
use crate::instance::Server;
use crate::settings::server_name;
use crate::shadnet::connection;
use serde::Serialize;
use serde_json::{json, Value};
use std::io::{Read, Write};
use std::time::Duration;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
const IO_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_RESPONSE: u64 = 64 * 1024;

pub fn host_header(host: &str, port: u16) -> String {
    if host.contains(':') && !host.starts_with('[') {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    }
}

/// Sends one request and reads the whole response; `None` when the connection fails.
pub fn exchange(host: &str, port: u16, request: &[u8]) -> Option<Vec<u8>> {
    let mut stream = connection::connect(host, port, CONNECT_TIMEOUT).ok()?;
    stream.set_read_timeout(Some(IO_TIMEOUT)).ok()?;
    stream.set_write_timeout(Some(IO_TIMEOUT)).ok()?;
    stream.write_all(request).ok()?;
    let mut response = Vec::new();
    stream.take(MAX_RESPONSE).read_to_end(&mut response).ok()?;
    Some(response)
}

pub fn status_code(response: &[u8]) -> Option<u16> {
    let line = response.split(|&b| b == b'\n').next()?;
    let mut parts = std::str::from_utf8(line).ok()?.split_whitespace();
    parts.next().filter(|v| v.starts_with("HTTP/1."))?;
    parts.next()?.parse().ok()
}

#[derive(Debug, Clone, PartialEq)]
struct Reply {
    status: u16,
    body: Value,
}

fn parse(response: &[u8]) -> Option<Reply> {
    let status = status_code(response)?;
    let split = response.windows(4).position(|w| w == b"\r\n\r\n")?;
    let body = serde_json::from_slice(&response[split + 4..]).unwrap_or(Value::Null);
    Some(Reply { status, body })
}

fn unreachable(server: &str) -> BackendError {
    let name = server_name(server);
    BackendError::new("server_unreachable", "backend.account_unreachable", format!("Cannot reach the account service of the {}.", name.message))
        .with("server", name)
}

fn request(method: &str, path: &str, host: &str, port: u16, body: Option<&Value>) -> Vec<u8> {
    let body = body.map(Value::to_string).unwrap_or_default();
    let mut head = format!("{method} {path} HTTP/1.1\r\nHost: {}\r\nAccept: application/json\r\nConnection: close\r\n", host_header(host, port));
    if !body.is_empty() || method == "POST" {
        head.push_str(&format!("Content-Type: application/json\r\nContent-Length: {}\r\n", body.len()));
    }
    [head.as_bytes(), b"\r\n", body.as_bytes()].concat()
}

fn call(server: &str, method: &str, path: &str, body: Option<&Value>) -> Res<Reply> {
    let at = Server::of(server);
    let response = exchange(&at.host, at.backend, &request(method, path, &at.host, at.backend, body)).ok_or_else(|| unreachable(server))?;
    parse(&response).ok_or_else(|| BackendError::new("internal", "backend.account_bad_reply", "The account service gave an unexpected answer."))
}

fn api_error(r: &Reply) -> BackendError {
    match r.body["error"].as_str().unwrap_or("") {
        "invalid_name" => BackendError::invalid("backend.online_id_format", "Online ID: 3 to 16 letters, digits, - and _."),
        "reserved_name" => BackendError::invalid("backend.register_name_reserved", "That Online ID is reserved. Choose another one."),
        "name_taken" => BackendError::new("already_exists", "backend.register_id_taken", "That Online ID is taken."),
        "rate_limited" => {
            let wait = r.body["retryAfter"].as_u64().unwrap_or(60);
            BackendError::new("busy", "backend.account_rate_limited", format!("Too many attempts from this network. Try again in {wait} s."))
                .with("seconds", error::seconds(wait))
        }
        "shadnet_unavailable" => {
            BackendError::new("server_unreachable", "backend.account_shadnet_down", "The server cannot handle accounts right now. Try again later.")
        }
        "registration_closed" => BackendError::unsupported("backend.register_closed", "This server does not create accounts."),
        "invalid_code" => BackendError::new("unauthorized", "backend.play_code_refused", "The server did not accept the launcher's confirmation."),
        _ => BackendError::new("internal", "backend.account_server_error", format!("The account service failed (HTTP {}).", r.status))
            .with("code", r.status),
    }
}

/// The Online ID as registered.
pub fn register(server: &str, online_id: &str, password: &str) -> Res<String> {
    let r = call(server, "POST", "/account/v1/register", Some(&json!({ "onlineId": online_id, "password": password })))?;
    if r.status != 201 {
        return Err(api_error(&r));
    }
    Ok(r.body["onlineId"].as_str().unwrap_or(online_id).to_string())
}

/// The registered casing of a name; `None` when this backend did not register it.
pub fn registered_name(server: &str, online_id: &str) -> Res<Option<String>> {
    let r = call(server, "GET", &format!("/account/v1/names/{online_id}"), None)?;
    match r.status {
        200 => Ok(r.body["onlineId"].as_str().map(str::to_string)),
        404 => Ok(None),
        _ => Err(api_error(&r)),
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct PlayGrant {
    pub online_id: String,
    /// The client address the backend saw: the game's requests must come from it.
    pub address: String,
    pub expires_in: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Play {
    Granted(PlayGrant),
    /// An older backend, which has no account API.
    NotOffered,
}

pub fn play(server: &str, code: &str) -> Res<Play> {
    let r = call(server, "POST", "/account/v1/play", Some(&json!({ "code": code })))?;
    match r.status {
        200 => Ok(Play::Granted(PlayGrant {
            online_id: r.body["onlineId"].as_str().unwrap_or("").to_string(),
            address: r.body["address"].as_str().unwrap_or("").to_string(),
            expires_in: r.body["expiresIn"].as_u64().unwrap_or(0),
        })),
        404 => Ok(Play::NotOffered),
        _ => Err(api_error(&r)),
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Relay {
    pub region: String,
    pub location: String,
    pub operator: String,
    /// `None`: the server has not checked it yet.
    pub online: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RelayList {
    pub checked_at: Option<String>,
    pub relays: Vec<Relay>,
}

/// What the screen shows of a list, whatever a server sends.
const MAX_RELAYS: usize = 64;
const MAX_RELAY_TEXT: usize = 64;

/// The relays the server lists and what its own last check found: the launcher never asks a
/// relay. `None` when the server publishes no list (an older backend, or one without a list).
pub fn relays(server: &str) -> Res<Option<RelayList>> {
    let unavailable = || {
        let name = server_name(server);
        BackendError::new("server_unreachable", "backend.relays_unavailable", format!("Cannot get the relay status from the {}.", name.message))
            .with("server", name)
    };
    let r = call(server, "GET", "/status/v1/relays", None).map_err(|_| unavailable())?;
    let text = |v: &Value| v.as_str().unwrap_or("").chars().take(MAX_RELAY_TEXT).collect::<String>();
    match (r.status, r.body["relays"].as_array()) {
        (404, _) => Ok(None),
        (200, Some(list)) => Ok(Some(RelayList {
            checked_at: r.body["checkedAt"].as_str().map(|_| text(&r.body["checkedAt"])),
            relays: list
                .iter()
                .take(MAX_RELAYS)
                .map(|e| Relay { region: text(&e["region"]), location: text(&e["location"]), operator: text(&e["operator"]), online: e["online"].as_bool() })
                .collect(),
        })),
        _ => Err(unavailable()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    fn reply(status: u16, body: &str) -> Reply {
        Reply { status, body: serde_json::from_str(body).unwrap() }
    }

    #[test]
    fn status_lines_and_bodies() {
        assert_eq!(status_code(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n"), Some(200));
        assert_eq!(status_code(b"HTTP/1.0 403 Forbidden\r\n"), Some(403));
        assert_eq!(status_code(b"SSH-2.0-OpenSSH\r\n"), None);
        assert_eq!(status_code(b""), None);
        assert_eq!(parse(b"HTTP/1.1 201 Created\r\nContent-Length: 17\r\n\r\n{\"onlineId\":\"Ab\"}"), Some(reply(201, r#"{"onlineId":"Ab"}"#)));
        assert_eq!(parse(b"HTTP/1.1 404 Not Found\r\n\r\nnot json"), Some(Reply { status: 404, body: Value::Null }));
        assert_eq!(parse(b"HTTP/1.1 200 OK\r\n"), None);
        assert_eq!(host_header("fd00::1", 1338), "[fd00::1]:1338");
        assert_eq!(host_header("10.0.0.1", 1338), "10.0.0.1:1338");
    }

    #[test]
    fn requests() {
        let r = String::from_utf8(request("POST", "/account/v1/play", "10.0.0.1", 1338, Some(&json!({"code": "abc"})))).unwrap();
        assert_eq!(
            r,
            "POST /account/v1/play HTTP/1.1\r\nHost: 10.0.0.1:1338\r\nAccept: application/json\r\nConnection: close\r\nContent-Type: application/json\r\nContent-Length: 14\r\n\r\n{\"code\":\"abc\"}"
        );
        let r = String::from_utf8(request("GET", "/account/v1/names/Ab_c", "h", 1, None)).unwrap();
        assert_eq!(r, "GET /account/v1/names/Ab_c HTTP/1.1\r\nHost: h:1\r\nAccept: application/json\r\nConnection: close\r\n\r\n");
    }

    #[test]
    fn every_error_code_has_a_message() {
        let e = |status, body| api_error(&reply(status, body));
        assert_eq!((e(400, r#"{"error":"invalid_name"}"#).code, e(400, r#"{"error":"invalid_name"}"#).key), ("invalid_input", "backend.online_id_format"));
        assert_eq!(e(403, r#"{"error":"reserved_name"}"#).key, "backend.register_name_reserved");
        assert_eq!(e(409, r#"{"error":"name_taken"}"#).code, "already_exists");
        let limited = e(429, r#"{"error":"rate_limited","retryAfter":42}"#);
        assert_eq!((limited.code, limited.key, limited.params["seconds"]["seconds"].as_u64()), ("busy", "backend.account_rate_limited", Some(42)));
        assert_eq!(e(502, r#"{"error":"shadnet_unavailable"}"#).code, "server_unreachable");
        assert_eq!(e(503, r#"{"error":"registration_closed"}"#).code, "unsupported");
        assert_eq!(e(401, r#"{"error":"invalid_code"}"#).code, "unauthorized");
        let other = e(500, r#"{"code":1,"error":"internal error"}"#);
        assert_eq!((other.key, other.params["code"].as_u64()), ("backend.account_server_error", Some(500)));
        let down = unreachable("10.0.0.1");
        assert_eq!((down.code, down.params["server"]["key"].as_str()), ("server_unreachable", Some("server.private")));
        assert!(!unreachable(crate::settings::default_server()).message.contains(crate::settings::default_server()));
    }

    /// A backend on a free local port that answers each accepted connection with the next reply.
    fn backend(replies: &'static [&'static str]) -> (String, std::thread::JoinHandle<Vec<String>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            let mut seen = Vec::new();
            for reply in replies {
                let (mut s, _) = listener.accept().unwrap();
                s.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
                let mut buf = vec![0u8; 4096];
                let n = s.read(&mut buf).unwrap();
                seen.push(String::from_utf8_lossy(&buf[..n]).to_string());
                s.write_all(reply.as_bytes()).unwrap();
            }
            seen
        });
        (format!("127.0.0.1:31313:31314:31315:{port}:31316"), server)
    }

    #[test]
    fn calls_over_a_fake_backend() {
        let (server, handle) = backend(&[
            "HTTP/1.1 201 Created\r\nContent-Length: 24\r\n\r\n{\"onlineId\": \"Player_1\"}",
            "HTTP/1.1 200 OK\r\n\r\n{\"onlineId\": \"Player_1\"}",
            "HTTP/1.1 404 Not Found\r\n\r\n{\"error\": \"not_found\"}",
            "HTTP/1.1 200 OK\r\n\r\n{\"accountId\": \"7\", \"onlineId\": \"Player_1\", \"address\": \"203.0.113.9\", \"expiresIn\": 43200}",
            "HTTP/1.1 404 Not Found\r\n\r\n{\"code\": 141, \"error\": \"not implemented\"}",
            "HTTP/1.1 401 Unauthorized\r\n\r\n{\"error\": \"invalid_code\"}",
        ]);
        assert_eq!(register(&server, "player_1", "p").unwrap(), "Player_1");
        assert_eq!(registered_name(&server, "PLAYER_1").unwrap().as_deref(), Some("Player_1"));
        assert_eq!(registered_name(&server, "Nobody").unwrap(), None);
        let granted = PlayGrant { online_id: "Player_1".into(), address: "203.0.113.9".into(), expires_in: 43200 };
        assert_eq!(play(&server, "c0de").unwrap(), Play::Granted(granted));
        assert_eq!(play(&server, "c0de").unwrap(), Play::NotOffered);
        assert_eq!(play(&server, "c0de").unwrap_err().code, "unauthorized");
        let seen = handle.join().unwrap();
        assert!(seen[0].starts_with("POST /account/v1/register HTTP/1.1\r\n"));
        assert!(seen[0].ends_with(r#"{"onlineId":"player_1","password":"p"}"#));
        assert!(seen[1].starts_with("GET /account/v1/names/PLAYER_1 HTTP/1.1\r\n"));
        assert!(seen[3].ends_with(r#"{"code":"c0de"}"#));
        assert_eq!(play(&server, "c0de").unwrap_err().key, "backend.account_unreachable");
    }

    #[test]
    fn relay_lists() {
        let (server, handle) = backend(&[
            "HTTP/1.1 200 OK\r\n\r\n{\"checkedAt\": \"2026-11-24T08:00:00.000Z\", \"relays\": [{\"region\": \"Europe\", \"location\": \"Stockholm, Sweden\", \"operator\": \"someone\", \"online\": true}, {\"region\": \"Asia\", \"online\": false, \"address\": \"relay.example.org\"}, {\"region\": 5, \"online\": null}, 7]}",
            "HTTP/1.1 200 OK\r\n\r\n{\"checkedAt\": null, \"relays\": []}",
            "HTTP/1.1 404 Not Found\r\n\r\n{\"error\": \"not_found\"}",
            "HTTP/1.1 404 Not Found\r\n\r\n{\"code\": 141, \"error\": \"not implemented\"}",
            "HTTP/1.1 429 Too Many Requests\r\n\r\n{\"error\": \"rate_limited\", \"retryAfter\": 9}",
            "HTTP/1.1 200 OK\r\n\r\n{\"relays\": \"none\"}",
            "HTTP/1.1 200 OK\r\n\r\nnot json",
        ]);
        let relay = |region: &str, location: &str, operator: &str, online| Relay { region: region.into(), location: location.into(), operator: operator.into(), online };
        let list = relays(&server).unwrap().unwrap();
        assert_eq!(list.checked_at.as_deref(), Some("2026-11-24T08:00:00.000Z"));
        assert_eq!(list.relays, [relay("Europe", "Stockholm, Sweden", "someone", Some(true)), relay("Asia", "", "", Some(false)), relay("", "", "", None), relay("", "", "", None)]);
        assert_eq!(serde_json::to_value(&list).unwrap()["relays"][1], json!({"region": "Asia", "location": "", "operator": "", "online": false}));
        assert_eq!(serde_json::to_value(&list).unwrap()["checkedAt"], "2026-11-24T08:00:00.000Z");
        assert_eq!(relays(&server).unwrap(), Some(RelayList { checked_at: None, relays: vec![] }));
        assert_eq!(relays(&server).unwrap(), None);
        assert_eq!(relays(&server).unwrap(), None);
        for _ in 0..3 {
            let e = relays(&server).unwrap_err();
            assert_eq!((e.code, e.key), ("server_unreachable", "backend.relays_unavailable"));
        }
        assert!(handle.join().unwrap()[0].starts_with("GET /status/v1/relays HTTP/1.1\r\n"));
        let down = relays(&server).unwrap_err();
        assert_eq!((down.key, down.params["server"]["key"].as_str()), ("backend.relays_unavailable", Some("server.private")));
    }

    #[test]
    fn a_long_relay_list_is_cut() {
        let entry = format!("{{\"region\": \"{}\", \"online\": true}}", "x".repeat(200));
        let body = format!("HTTP/1.1 200 OK\r\n\r\n{{\"checkedAt\": null, \"relays\": [{}]}}", vec![entry; 100].join(","));
        let (server, _handle) = backend(Box::leak(Box::new([&*Box::leak(body.into_boxed_str())])));
        let list = relays(&server).unwrap().unwrap();
        assert_eq!((list.relays.len(), list.relays[0].region.len()), (MAX_RELAYS, MAX_RELAY_TEXT));
    }
}
