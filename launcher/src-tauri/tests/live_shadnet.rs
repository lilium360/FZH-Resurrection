//! Against a real, throwaway shadNet server and the backend (`server/backend/backend.py`) pointed
//! at it, both on 127.0.0.1 (they create accounts). Ignored by default; run with
//!   FZH_TEST_DIR=<empty folder> FZH_TEST_SERVER=127.0.0.1:<game>:<matching>:<webapi>:<backend>:<relay>
//!   FZH_TEST_PREFIX=<3-6 chars> cargo test --test live_shadnet -- --ignored --test-threads=1
//! The backend needs registration on (`--shadnet-game`, `--registration-key-file`), limits that let
//! one address register a dozen accounts (`--register-per-day 50 --register-cooldown 0`) and
//! `--game-login grant`. Each launcher account service has its own data folder and plays one PC.
//! The tests write and delete Windows Credential Manager entries named "FZH Resurrection/127.0.0.1:...".

use fzh_launcher_lib::account::{Account, Env, EnvRef};
use fzh_launcher_lib::layout::Layout;
use fzh_launcher_lib::users;
use fzh_launcher_lib::vault::{self, Vault};
use serde_json::Value;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

struct TestEnv {
    layout: Layout,
    account: Account,
    events: Mutex<Vec<(String, Value)>>,
    logs: Mutex<Vec<String>>,
}

impl Env for TestEnv {
    fn layout(&self) -> &Layout {
        &self.layout
    }
    fn account(&self) -> &Account {
        &self.account
    }
    fn emit(&self, event: &str, payload: Value) {
        self.events.lock().unwrap().push((event.to_string(), payload));
    }
    fn log(&self, line: fzh_launcher_lib::error::Msg) {
        self.logs.lock().unwrap().push(line.message);
    }
    fn client_version(&self) -> String {
        "FZH-Launcher test".into()
    }
    fn vault(&self) -> &dyn Vault {
        &vault::System
    }
}

fn server() -> String {
    std::env::var("FZH_TEST_SERVER").expect("FZH_TEST_SERVER")
}

/// Port `i` of the server address: 0 game, 1 matching, 2 WebAPI, 3 backend.
fn port(i: usize) -> u16 {
    server().split(':').nth(i + 1).expect("FZH_TEST_SERVER with ports").parse().unwrap()
}

fn prefix() -> String {
    std::env::var("FZH_TEST_PREFIX").unwrap_or_else(|_| "t".into())
}

/// A launcher data folder: one PC, on the test server.
fn pc(dir: &str) -> (Arc<TestEnv>, EnvRef) {
    pc_on(dir, &server())
}

fn pc_on(dir: &str, server: &str) -> (Arc<TestEnv>, EnvRef) {
    let root = PathBuf::from(std::env::var("FZH_TEST_DIR").expect("FZH_TEST_DIR")).join(dir);
    std::fs::create_dir_all(&root).unwrap();
    let settings = serde_json::json!({ "usePrivateServer": true, "privateServer": server });
    std::fs::write(root.join("settings.json"), settings.to_string()).unwrap();
    let env = Arc::new(TestEnv {
        layout: Layout { data: root, emulator: PathBuf::new(), patches: PathBuf::new(), licences: PathBuf::new(), warmup_script: PathBuf::new() },
        account: Account::default(),
        events: Mutex::new(Vec::new()),
        logs: Mutex::new(Vec::new()),
    });
    let r: EnvRef = env.clone();
    (env, r)
}

fn stored(env: &TestEnv) -> users::Credentials {
    users::account(&serde_json::from_slice(&std::fs::read(env.layout.users_json()).unwrap()).unwrap()).unwrap()
}

/// Registers `id` and returns its recovery code.
fn register(env: &TestEnv, r: &EnvRef, id: &str) -> String {
    let reg = env.account.register(r, id).unwrap();
    assert!(reg.problem.is_none(), "{:?}", reg.problem);
    assert_eq!(reg.session.unwrap().online_id, id);
    reg.recovery_code
}

/// Waits for a `friends` event whose payload satisfies `f`.
fn wait_friends(env: &TestEnv, what: &str, f: impl Fn(&Value) -> bool) -> Value {
    let start = Instant::now();
    loop {
        if let Some((_, v)) = env.events.lock().unwrap().iter().rev().find(|(e, _)| e == "friends") {
            if f(v) {
                return v.clone();
            }
        }
        assert!(start.elapsed() < Duration::from_secs(10), "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn names(v: &Value, list: &str) -> Vec<String> {
    v[list]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x.get("onlineId").unwrap_or(x).as_str().unwrap().to_string())
        .collect()
}

fn assert_no_secret(envs: &[&Arc<TestEnv>], secrets: &[&str]) {
    for env in envs {
        let logs = env.logs.lock().unwrap().join("\n");
        let events = format!("{:?}", env.events.lock().unwrap());
        let files: String = ["account.json", "settings.json", "launcher.json"]
            .iter()
            .filter_map(|f| std::fs::read_to_string(env.layout.data.join(f)).ok())
            .collect();
        for s in secrets {
            assert!(!logs.contains(s) && !events.contains(s) && !files.contains(s), "a secret reached a log, an event or a launcher store");
        }
    }
}

#[test]
#[ignore]
fn registration_sign_in_and_friends_on_a_real_server() {
    let (a_env, a) = pc("player-a");
    let (b_env, b) = pc("player-b");
    let (c_env, c) = pc("player-c");
    let id_a = format!("{}_Alpha", prefix());
    let id_b = format!("{}_Bravo", prefix());

    let code_a = register(&a_env, &a, &id_a);
    assert!(a_env.account.is_connected(&a));
    let creds = stored(&a_env);
    assert_eq!((creds.npid.as_str(), creds.email), (id_a.as_str(), format!("{}@users.invalid", id_a.to_lowercase())));
    let target = vault::target(&server(), &id_a);
    assert_eq!(vault::System.get(&target).as_deref(), Some(creds.password.as_str()), "the Credential Manager holds the password users.json holds");
    println!("register {id_a} -> code shown once, signed in, users.json and the Credential Manager written");

    let e = b_env.account.register(&b, &id_a.to_uppercase()).err().expect("refused");
    assert_eq!((e.code, e.key), ("already_exists", "backend.register_id_taken"));
    let e = b_env.account.register(&b, &format!("Admin{}", prefix())).err().expect("refused");
    assert_eq!(e.key, "backend.register_name_reserved");
    let e = b_env.account.register(&b, "a b").err().expect("refused");
    assert_eq!(e.key, "backend.online_id_format");
    let code_b = register(&b_env, &b, &id_b);
    println!("same name in other casing -> already_exists; reserved -> {}; bad format refused locally", e.code);

    let wrong = "00000-00000-00000-00000-00000";
    let e = c_env.account.sign_in(&c, &id_a, wrong).unwrap_err();
    assert_eq!((e.code, e.key), ("wrong_password", "backend.recover_wrong"));
    let e = c_env.account.sign_in(&c, "Nobody_There", &code_a).unwrap_err();
    assert_eq!(e.key, "backend.recover_wrong");
    let e = c_env.account.sign_in(&c, &id_a, "not a code").unwrap_err();
    assert_eq!(e.key, "backend.recovery_code_invalid");
    let e = c_env.account.sign_in(&c, &id_a, &code_a).unwrap_err();
    assert_eq!(e.code, "already_signed_in");
    println!("wrong code / unknown name -> one message; malformed code refused locally; second session -> already_signed_in");

    let e = a_env.account.send_friend_request(&a, "Nobody_There").unwrap_err();
    assert_eq!(e.code, "not_found");
    let st = a_env.account.send_friend_request(&a, &id_b).unwrap();
    assert_eq!(names(&serde_json::to_value(&st).unwrap(), "outgoing"), [id_b.clone()]);
    wait_friends(&b_env, "B's incoming request", |v| names(v, "incoming") == [id_a.clone()]);
    let e = a_env.account.send_friend_request(&a, &id_b).unwrap_err();
    assert_eq!(e.code, "already_friend");
    b_env.account.accept_friend_request(&b, &id_a).unwrap();
    let v = wait_friends(&a_env, "A's new friend", |v| names(v, "friends") == [id_b.clone()] && names(v, "outgoing").is_empty());
    assert_eq!(v["friends"][0]["online"], true);
    println!("request -> FriendQuery on B; accept -> FriendNew on A, friend online");

    b_env.account.suspend(&b);
    let v = wait_friends(&a_env, "B offline", |v| v["friends"][0]["online"] == false);
    assert!(v["friends"][0]["presence"]["lastSeen"].as_str().unwrap().ends_with('Z'));
    b_env.account.resume();
    b_env.account.tick(&b);
    wait_friends(&a_env, "B online again", |v| v["friends"][0]["online"] == true);
    println!("B suspended -> offline with lastSeen; resumed -> the monitor signed in again with the stored password");

    a_env.account.sign_out(&a).unwrap();
    assert!(!a_env.account.is_signed_in(&a));
    assert_eq!(vault::System.get(&target), None, "sign-out removes the Credential Manager entry");
    let typed = code_a.to_lowercase().replace('-', " ").replace('0', "o").replace('1', "l");
    let s = a_env.account.sign_in(&a, &id_a.to_lowercase(), &typed).unwrap();
    assert_eq!(s.online_id, id_a, "the registered casing comes from the backend");
    assert_eq!(vault::System.get(&target).as_deref(), Some(stored(&a_env).password.as_str()));
    println!("sign out -> entry removed; sign in with the name in lower case and the code typed loosely -> {}", s.online_id);

    assert_no_secret(&[&a_env, &b_env, &c_env], &[&code_a, &code_b, &stored(&a_env).password, &stored(&b_env).password]);
    a_env.account.sign_out(&a).unwrap();
    b_env.account.sign_out(&b).unwrap();
}

/// POST /parse/functions/login as the game sends it, from 127.0.0.1.
fn game_login(unique_net_id: u64) -> String {
    let body = format!(r#"{{"uniqueNetId":"{unique_net_id}","nickname":"x","accessToken":""}}"#);
    http(
        port(3),
        &format!("POST /parse/functions/login HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()),
    )
    .0
}

#[test]
#[ignore]
fn recovery_on_a_second_pc_and_the_play_grant() {
    use fzh_launcher_lib::shadnet::{self, connection::Connection};
    let (one_env, one) = pc("pc-1");
    let (two_env, two) = pc("pc-2");
    let id = format!("{}_Recov", prefix());
    let code = register(&one_env, &one, &id);
    let password = stored(&one_env).password;
    one_env.account.suspend(&one);

    let s = two_env.account.sign_in(&two, &id.to_uppercase(), &code).unwrap();
    assert_eq!(s.online_id, id);
    assert_eq!(stored(&two_env).password, password, "both PCs derive the same password");
    println!("PC 2 signs in with {} and the code -> {}, same password as PC 1", id.to_uppercase(), s.online_id);

    assert_eq!(two_env.account.play_grant(&two), None);
    let granted = two_env.logs.lock().unwrap().iter().find(|l| l.contains("play from")).cloned().expect("a play grant log line");
    assert!(granted.contains(&format!("{id} play from 127.0.0.1 for ")), "{granted}");
    println!("play grant -> \"{granted}\"");

    two_env.account.suspend(&two);
    let conn = Connection::open("127.0.0.1", port(0), shadnet::CONNECT_TIMEOUT, |_| {}).unwrap();
    let login = shadnet::login(&conn, "127.0.0.1", &id, &password).unwrap();
    conn.close();
    let status = game_login(login.user_id);
    assert!(status.contains(" 200 "), "{status}");
    let other = game_login(login.user_id + 1000);
    assert!(other.contains(" 403 "), "{other}");
    println!("the game's login for account {} -> {status}; for an account without a grant -> {other}", login.user_id);

    two_env.account.resume();
    two_env.account.tick(&two);
    two_env.account.sign_out(&two).unwrap();
    assert_eq!(vault::System.get(&vault::target(&server(), &id)), None);
    assert_no_secret(&[&one_env, &two_env], &[&code, &password]);
}

/// A 256x256 opaque PNG of one colour, as the backend's `plain_png`.
fn plain_png(rgb: [u8; 3]) -> Vec<u8> {
    use std::io::Write;
    let mut raw = Vec::new();
    for _ in 0..256 {
        raw.push(0);
        (0..256).for_each(|_| raw.extend_from_slice(&rgb));
    }
    let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::best());
    z.write_all(&raw).unwrap();
    let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut ihdr = [0u8; 13];
    ihdr[..8].copy_from_slice(&[0, 0, 1, 0, 0, 0, 1, 0]);
    ihdr[8..10].copy_from_slice(&[8, 2]);
    for (kind, data) in [(&b"IHDR"[..], &ihdr[..]), (b"IDAT", &z.finish().unwrap()), (b"IEND", &[])] {
        png.extend_from_slice(&(data.len() as u32).to_be_bytes());
        let mut crc = flate2::Crc::new();
        crc.update(kind);
        crc.update(data);
        png.extend_from_slice(kind);
        png.extend_from_slice(data);
        png.extend_from_slice(&crc.sum().to_be_bytes());
    }
    png
}

/// Status line and body of a plain HTTP/1.1 request to 127.0.0.1.
fn http(port: u16, request: &str) -> (String, Vec<u8>) {
    use std::io::{Read, Write};
    let mut s = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    s.write_all(request.as_bytes()).unwrap();
    let (mut r, mut buf) = (Vec::new(), [0u8; 8192]);
    let head_end = |r: &[u8]| r.windows(4).position(|w| w == b"\r\n\r\n");
    let length = |head: &str| head.lines().find_map(|l| l.to_ascii_lowercase().strip_prefix("content-length:").map(|v| v.trim().parse::<usize>().unwrap()));
    loop {
        if let Some(split) = head_end(&r) {
            let head = String::from_utf8_lossy(&r[..split]).to_string();
            if length(&head).is_some_and(|n| r.len() >= split + 4 + n) {
                break;
            }
        }
        let n = s.read(&mut buf).unwrap();
        if n == 0 {
            break;
        }
        r.extend_from_slice(&buf[..n]);
    }
    let split = head_end(&r).unwrap();
    let status = String::from_utf8_lossy(&r[..split]).lines().next().unwrap().to_string();
    (status, r[split + 4..].to_vec())
}

#[test]
#[ignore]
fn avatar_upload_on_a_real_server() {
    use fzh_launcher_lib::account::ProfileUpdate;
    use fzh_launcher_lib::avatar;
    use fzh_launcher_lib::shadnet::{self, connection::Connection};
    let (a_env, a) = pc("avatar-a");
    let (b_env, b) = pc("avatar-b");
    let id_a = format!("{}_AvA", prefix());
    let id_b = format!("{}_AvB", prefix());
    register(&a_env, &a, &id_a);
    register(&b_env, &b, &id_b);
    b_env.account.sign_out(&b).unwrap();

    let first = plain_png([200, 40, 40]);
    a_env.account.upload_avatar(&a, &avatar::data_url(&first)).unwrap();
    let (status, body) = http(port(3), &format!("GET /a/{id_a}.png HTTP/1.1\r\nHost: fzh.avatar\r\nConnection: close\r\n\r\n"));
    println!("upload_avatar as {id_a} -> Ok; GET /a/{id_a}.png -> {status}, {} bytes, same bytes: {}", body.len(), body == first);
    assert_eq!(body, first);
    assert!(a_env.logs.lock().unwrap().iter().any(|l| l == "Network: avatar sent to the server"));

    let profile = ProfileUpdate { online_id: id_a.clone(), avatar: "image".into(), image: Some(avatar::data_url(&first)) };
    let s = a_env.account.update_profile(&a, &profile).unwrap();
    assert_eq!((s.avatar.as_str(), s.image.as_deref()), (Some("image"), Some(avatar::data_url(&first).as_str())));
    assert_eq!(std::fs::read(a_env.layout.avatar_png()).unwrap(), first);
    assert_eq!(a_env.account.get_account(&a).session.unwrap().avatar, "image");
    println!("update_profile with the picture -> avatar.png written, session avatar \"image\" after a reload");

    let password = stored(&a_env).password;
    a_env.account.suspend(&a);
    let conn = Connection::open("127.0.0.1", port(0), shadnet::CONNECT_TIMEOUT, |_| {}).unwrap();
    shadnet::login(&conn, "127.0.0.1", &id_a, &password).unwrap();
    shadnet::report_version(&conn, "127.0.0.1", "FZH-Launcher test").unwrap();
    let token = shadnet::get_token(&conn, "127.0.0.1").unwrap();
    assert_eq!(token.npid, id_a);

    let second = plain_png([40, 200, 40]);
    avatar::upload(&server(), &id_a, &token.token, &second).unwrap();
    let (_, body) = http(port(3), &format!("GET /a/{}.png HTTP/1.1\r\nHost: fzh.avatar\r\nConnection: close\r\n\r\n", id_a.to_lowercase()));
    assert_eq!(body, second);
    let e = avatar::upload(&server(), &id_b, &token.token, &second).unwrap_err();
    assert_eq!(e.key, "backend.avatar_not_owner");
    let e = avatar::upload(&server(), &id_a, "not-a-token", &second).unwrap_err();
    assert_eq!(e.key, "backend.avatar_not_owner");
    println!("PUT with the token of the session -> stored; for another account or with a made-up token -> 403");
    conn.close();

    a_env.account.resume();
    a_env.account.tick(&a);
    a_env.account.sign_out(&a).unwrap();
    assert_no_secret(&[&a_env, &b_env], &[&password, &token.token]);
}

/// A `multipart/mixed` WebAPI POST with one JSON part, as the game sends it.
fn webapi_post(path: &str, token: &str, description: &str, json: &str) -> (String, Vec<u8>) {
    let body = format!("--fzh\r\nContent-Type: application/json; charset=utf-8\r\nContent-Description: {description}\r\n\r\n{json}\r\n--fzh--\r\n");
    http(
        port(2),
        &format!(
            "POST {path} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nAuthorization: Bearer {token}\r\nContent-Type: multipart/mixed; boundary=fzh\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            port(2),
            body.len()
        ),
    )
}

fn invites(env: &TestEnv) -> Vec<Value> {
    env.events.lock().unwrap().iter().filter(|(e, _)| e == "invite").map(|(_, v)| v.clone()).collect()
}

#[test]
#[ignore]
fn invitations_reach_the_signed_in_launcher() {
    use fzh_launcher_lib::shadnet::{self, connection::Connection};
    let (a_env, a) = pc("inv-a");
    let (b_env, b) = pc("inv-b");
    let id_a = format!("{}_InvA", prefix());
    let id_b = format!("{}_InvB", prefix());
    register(&a_env, &a, &id_a);
    register(&b_env, &b, &id_b);
    a_env.account.send_friend_request(&a, &id_b).unwrap();
    wait_friends(&b_env, "B's incoming request", |v| names(v, "incoming") == [id_a.clone()]);
    b_env.account.accept_friend_request(&b, &id_a).unwrap();
    wait_friends(&a_env, "A's new friend", |v| names(v, "friends") == [id_b.clone()]);

    let password = stored(&a_env).password;
    a_env.account.suspend(&a);
    let conn = Connection::open("127.0.0.1", port(0), shadnet::CONNECT_TIMEOUT, |_| {}).unwrap();
    shadnet::login(&conn, "127.0.0.1", &id_a, &password).unwrap();
    shadnet::report_version(&conn, "127.0.0.1", "FZH-Launcher test").unwrap();
    let token = shadnet::get_token(&conn, "127.0.0.1").unwrap().token;
    let session = r#"{"sessionType":"owner-bind","sessionPrivacy":"private","sessionMaxUser":4,"availablePlatforms":["PS4"]}"#;
    let (status, body) = webapi_post("/v1/sessions", &token, "session-request", session);
    let session_id = serde_json::from_slice::<Value>(&body).unwrap()["sessionId"].as_str().unwrap().to_string();
    println!("A, signed in as the game would be, created session {session_id}: {status}");
    let send = || webapi_post(&format!("/v1/sessions/{session_id}/invitations"), &token, "invitation-request", &format!(r#"{{"to":["{id_b}"],"message":"x"}}"#)).0;

    println!("A invites B while B's launcher is signed in -> {}", send());
    let start = Instant::now();
    while invites(&b_env).is_empty() {
        assert!(start.elapsed() < Duration::from_secs(10), "no invite event on B");
        std::thread::sleep(Duration::from_millis(100));
    }
    assert_eq!(invites(&b_env), [serde_json::json!({ "from": id_a })]);

    b_env.account.suspend(&b);
    println!("B's launcher suspended (the game starts); A invites again -> {}", send());
    std::thread::sleep(Duration::from_secs(2));
    assert_eq!(invites(&b_env).len(), 1, "a suspended launcher has no connection to receive it");

    b_env.account.resume();
    b_env.account.tick(&b);
    assert!(b_env.account.is_connected(&b));
    send();
    let start = Instant::now();
    while invites(&b_env).len() < 2 {
        assert!(start.elapsed() < Duration::from_secs(10), "no invite event after resume");
        std::thread::sleep(Duration::from_millis(100));
    }
    println!("resumed -> the next invitation arrives again");
    conn.close();
    a_env.account.resume();
    a_env.account.tick(&a);
    a_env.account.sign_out(&a).unwrap();
    b_env.account.sign_out(&b).unwrap();
}

#[test]
#[ignore]
fn status_when_the_server_is_down() {
    let free = || std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let (env, r) = pc_on("player-down", &format!("127.0.0.1:{}:31314:31315:{}:31316", free(), free()));
    let s = env.account.server_status(&r);
    assert!(!s.network && !s.online, "{s:?}");
    assert_eq!(env.events.lock().unwrap().last().unwrap().0, "server-status");
    let e = env.account.sign_in(&r, "Someone_1", "00000-00000-00000-00000-00000").unwrap_err();
    assert_eq!(e.code, "server_unreachable");
    let e = env.account.register(&r, "Someone_1").err().expect("refused");
    assert_eq!((e.code, e.key), ("server_unreachable", "backend.account_unreachable"));
    println!("server down: {s:?}; sign-in and registration -> {}", e.message);
}
