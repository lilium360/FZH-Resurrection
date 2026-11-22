//! The problem report: one text file the player saves and attaches to an issue on the project's
//! public tracker. It keeps the Online ID; the password, tokens and keys never go in it, and network
//! addresses (the player's, other players', a private server's) are replaced by markers.

use crate::game::CREATE_NO_WINDOW;
use crate::instance::Server;
use crate::layout::Layout;
use crate::{accounts, clock, instance, settings, store, users};
use serde::Deserialize;
use serde_json::Value;
use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::path::Path;
use std::process::{Command, Stdio};

#[cfg(windows)]
use std::os::windows::process::CommandExt;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProblemReport {
    pub description: String,
    pub attach_logs: bool,
    pub attach_system: bool,
}

const LAUNCHER_LOG_MAX: u64 = 1 << 20;
const EMULATOR_LOG_MAX: u64 = 4 << 20;
const REMOVED: &str = "[removed]";
const SECRET_WORDS: &[&str] = &["password", "passwd", "token", "secret", "apikey", "api_key"];

/// Removes every known secret, and the value of any line or JSON key that names one.
pub fn redact(text: &str, secrets: &[String]) -> String {
    let mut out = String::with_capacity(text.len());
    for line in text.split_inclusive('\n') {
        let mut line = line.to_string();
        for s in secrets.iter().filter(|s| s.len() >= 3) {
            line = line.replace(s.as_str(), REMOVED);
        }
        let lower = line.to_ascii_lowercase();
        if SECRET_WORDS.iter().any(|w| lower.contains(w)) {
            let keep = line.find(['=', ':']).map(|i| i + 1).unwrap_or(0);
            let end = if line.ends_with('\n') { "\n" } else { "" };
            line = format!("{} {REMOVED}{end}", line[..keep].trim_end());
        }
        out.push_str(&line);
    }
    out
}

/// A host the report names by what it is: the host in lower case, and its marker.
pub type Named = (String, String);

fn local_host(host: &str) -> bool {
    host == "localhost" || host.parse::<IpAddr>().is_ok_and(|a| a.is_loopback() || a.is_unspecified())
}

/// The hosts this PC knows as servers: the private server and its relays, the community server,
/// the servers an account is remembered for. Longest first, so that no host is cut at a shorter one.
fn named_hosts(layout: &Layout) -> Vec<Named> {
    let mut named: Vec<Named> = Vec::new();
    let mut add = |host: &str, marker: String| {
        let host = host.to_ascii_lowercase();
        if host.len() >= 2 && !local_host(&host) && !named.iter().any(|(h, _)| *h == host) {
            named.push((host, marker));
        }
    };
    let private = store::load_settings(&layout.settings_json()).private_server;
    if !private.is_empty() {
        let at = Server::of(&private);
        add(&at.host, "[private server]".into());
        for (n, (relay, _)) in at.relays.iter().enumerate() {
            add(relay, format!("[relay {}]", n + 1));
        }
    }
    add(&Server::of(settings::default_server()).host, "[community server]".into());
    for (n, host) in accounts::load(&layout.accounts_json()).hosts().iter().enumerate() {
        add(host, format!("[server {}]", n + 1));
    }
    named.sort_by_key(|(h, _)| std::cmp::Reverse(h.len()));
    named
}

fn word(c: u8) -> bool {
    c.is_ascii_alphanumeric()
}

/// The length of a dotted IPv4 address at `i`, and the address. Not a part of a longer dotted number.
fn ipv4_at(b: &[u8], i: usize) -> Option<(usize, Ipv4Addr)> {
    let mut octets = [0u8; 4];
    let mut j = i;
    for (n, octet) in octets.iter_mut().enumerate() {
        if n > 0 {
            if b.get(j) != Some(&b'.') {
                return None;
            }
            j += 1;
        }
        let digits = b[j..].iter().take(3).take_while(|c| c.is_ascii_digit()).count();
        *octet = std::str::from_utf8(&b[j..j + digits]).ok()?.parse().ok()?;
        j += digits;
    }
    let more = b.get(j).is_some_and(|&c| word(c)) || (b.get(j) == Some(&b'.') && b.get(j + 1).is_some_and(u8::is_ascii_digit));
    (!more).then(|| (j - i, Ipv4Addr::from(octets)))
}

/// The same for an IPv6 address: the longest run of hexadecimal digits, `:` and `.` that parses as
/// one, less the punctuation of the sentence after it. A clock time has too few groups to parse.
fn ipv6_at(b: &[u8], i: usize) -> Option<(usize, Ipv6Addr)> {
    let run = b[i..].iter().take_while(|c| c.is_ascii_hexdigit() || matches!(c, b':' | b'.')).count();
    if b.get(i + run).is_some_and(|&c| word(c)) {
        return None;
    }
    let mut text = std::str::from_utf8(&b[i..i + run]).ok()?;
    loop {
        let colons = text.bytes().filter(|&c| c == b':').count();
        if colons < 2 || (colons < 4 && !text.bytes().any(|c| c.is_ascii_digit())) {
            return None;
        }
        if let Ok(a) = text.parse::<Ipv6Addr>() {
            return Some((text.len(), a));
        }
        text = text.strip_suffix([':', '.'])?;
    }
}

/// A driver or Windows version can be four small numbers too.
fn version_before(b: &[u8], i: usize) -> bool {
    let before = String::from_utf8_lossy(&b[i.saturating_sub(12)..i]).to_ascii_lowercase();
    let before = before.trim_end_matches([' ', ':', '=']);
    before.ends_with("version") || before.ends_with("driver")
}

fn lan_v4(a: &Ipv4Addr) -> bool {
    a.is_private() || a.is_link_local() || (a.octets()[0] == 100 && a.octets()[1] & 0xC0 == 64)
}

fn lan_v6(a: &Ipv6Addr) -> bool {
    a.segments()[0] & 0xFFC0 == 0xFE80 || a.segments()[0] & 0xFE00 == 0xFC00
}

/// Replaces IPv4 and IPv6 addresses by `[IP-n]` (`[LAN-IP-n]` in a private range) and the hosts in
/// `named` by their marker. `n` counts the addresses in order of appearance, so two lines about
/// the same address show the same marker. Loopback, unspecified and multicast addresses, port
/// numbers, clock times and version numbers stay as they are.
pub fn hide_addresses(text: &str, named: &[Named]) -> String {
    let b = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut numbers: HashMap<IpAddr, usize> = HashMap::new();
    let mut marker = |a: IpAddr, lan: bool| {
        let next = numbers.len() + 1;
        format!("[{}IP-{}]", if lan { "LAN-" } else { "" }, numbers.entry(a).or_insert(next))
    };
    let (mut i, mut copied) = (0, 0);
    while i < b.len() {
        let before = if i == 0 { b' ' } else { b[i - 1] };
        let c = b[i];
        let mut found: Option<(usize, Option<String>)> = None;
        if !word(before) && !matches!(before, b'.' | b'_' | b'-') && word(c) {
            found = named.iter().find_map(|(host, name)| {
                let end = i + host.len();
                let same = b.len() >= end && b[i..end].eq_ignore_ascii_case(host.as_bytes());
                let more = b.get(end).is_some_and(|&c| word(c) || matches!(c, b'_' | b'-'))
                    || (b.get(end) == Some(&b'.') && b.get(end + 1).is_some_and(|&c| word(c)));
                (same && !more).then(|| (host.len(), Some(name.clone())))
            });
        }
        if found.is_none() && !word(before) && before != b'.' && c.is_ascii_digit() {
            if let Some((len, a)) = ipv4_at(b, i).filter(|_| !version_before(b, i)) {
                let kept = a.is_loopback() || a.is_multicast() || matches!(a.octets()[0], 0 | 255);
                found = Some((len, (!kept).then(|| marker(a.into(), lan_v4(&a)))));
            }
        }
        if found.is_none() && !word(before) && !matches!(before, b'.' | b':') && (c.is_ascii_hexdigit() || c == b':') {
            if let Some((len, a)) = ipv6_at(b, i) {
                let kept = a.is_loopback() || a.is_unspecified() || a.is_multicast();
                found = Some((len, (!kept).then(|| marker(a.into(), lan_v6(&a)))));
            }
        }
        match found {
            Some((len, Some(name))) => {
                out.push_str(&text[copied..i]);
                out.push_str(&name);
                i += len;
                copied = i;
            }
            Some((len, None)) => i += len,
            None => i += 1,
        }
    }
    out.push_str(&text[copied..]);
    out
}

const USER: &str = "[user]";

/// The names Windows knows the player by: the profile folder's and the account's.
fn user_names() -> Vec<String> {
    let folder = std::env::var("USERPROFILE").ok().and_then(|p| Path::new(&p).file_name().map(|n| n.to_string_lossy().into_owned()));
    let mut names: Vec<String> = folder.into_iter().chain(std::env::var("USERNAME").ok()).filter(|n| !n.trim().is_empty()).collect();
    names.sort_by_key(|n| std::cmp::Reverse(n.len()));
    names.dedup_by(|a, b| a.eq_ignore_ascii_case(b));
    names
}

/// Replaces the Windows user name by `[user]`: always as the folder after `Users` in a path
/// (`\`, `/` or the `\\` of JSON), and on its own when it has at least three characters, so that
/// a short name does not eat ordinary words. A longer word that contains it stays.
pub fn hide_user(text: &str, names: &[String]) -> String {
    let b = text.as_bytes();
    let part = |c: u8| word(c) || matches!(c, b'_' | b'-');
    let mut out = String::with_capacity(text.len());
    let (mut i, mut copied) = (0, 0);
    while i < b.len() {
        let found = names.iter().find(|name| {
            let end = i + name.len();
            let alone = (i == 0 || !part(b[i - 1])) && !b.get(end).is_some_and(|&c| part(c));
            let in_profile = || {
                let before = b[..i].strip_suffix(b"\\\\").or_else(|| b[..i].strip_suffix(b"\\")).or_else(|| b[..i].strip_suffix(b"/"));
                before.is_some_and(|p| p.len() >= 5 && p[p.len() - 5..].eq_ignore_ascii_case(b"users"))
            };
            b.len() >= end && b[i..end].eq_ignore_ascii_case(name.as_bytes()) && alone && (name.len() >= 3 || in_profile())
        });
        match found {
            Some(name) => {
                out.push_str(&text[copied..i]);
                out.push_str(USER);
                i += name.len();
                copied = i;
            }
            None => i += 1,
        }
    }
    out.push_str(&text[copied..]);
    out
}

fn tail(path: &Path, max: u64) -> String {
    let Ok(bytes) = std::fs::read(path) else { return "(missing)\n".into() };
    let skip = (bytes.len() as u64).saturating_sub(max) as usize;
    let mut text = String::from_utf8_lossy(&bytes[skip..]).into_owned();
    if skip > 0 {
        text = format!("(first {skip} bytes left out)\n{}", text.split_once('\n').map(|(_, r)| r).unwrap_or(&text));
    }
    if !text.ends_with('\n') {
        text.push('\n');
    }
    text
}

fn pretty(path: &Path) -> String {
    match instance::read_json(path) {
        Value::Null => "(missing)\n".into(),
        v => serde_json::to_string_pretty(&v).unwrap_or_default() + "\n",
    }
}

fn system() -> String {
    let script = "$ErrorActionPreference='SilentlyContinue'; \
        $o = Get-CimInstance Win32_OperatingSystem; 'Windows: ' + $o.Caption + ' ' + $o.Version + ' (build ' + $o.BuildNumber + ')'; \
        Get-CimInstance Win32_VideoController | ForEach-Object { 'GPU: ' + $_.Name + ', driver ' + $_.DriverVersion }; \
        $c = Get-CimInstance Win32_Processor | Select-Object -First 1; 'CPU: ' + $c.Name; \
        'Memory: ' + [math]::Round((Get-CimInstance Win32_ComputerSystem).TotalPhysicalMemory / 1GB) + ' GB'";
    let mut cmd = Command::new("powershell");
    cmd.args(["-NoProfile", "-NonInteractive", "-Command", script]).stdin(Stdio::null()).stderr(Stdio::null());
    #[cfg(windows)]
    cmd.creation_flags(CREATE_NO_WINDOW);
    match cmd.output() {
        Ok(o) if !o.stdout.is_empty() => String::from_utf8_lossy(&o.stdout).replace("\r\n", "\n"),
        _ => "(not available)\n".into(),
    }
}

pub fn build(layout: &Layout, report: &ProblemReport, version: &str) -> String {
    let account = users::account(&instance::read_json(&layout.users_json()));
    let secrets: Vec<String> = account.iter().map(|c| c.password.clone()).collect();
    let mut s = String::new();
    let mut section = |title: &str, body: &str| {
        s.push_str(&format!("===== {title} =====\n"));
        s.push_str(body);
        if !body.ends_with('\n') {
            s.push('\n');
        }
        s.push('\n');
    };
    section(
        "FZH:Resurrection problem report",
        &format!(
            "Created: {}\nLauncher: {version}\nEmulator commit: {}\nOnline ID: {}\nSign-in credentials are left out of this report.\n\
             Network addresses are replaced by markers such as [IP-1] or [private server]: the same marker is the same address. \
             The Windows user name is replaced by [user].\n",
            clock::local_date_time(),
            layout.emulator_commit(),
            account.as_ref().map(|c| c.npid.as_str()).unwrap_or("(not signed in)"),
        ),
    );
    section("What happened", report.description.trim());
    section("Settings", &pretty(&layout.settings_json()));
    section("Game files", &pretty(&layout.launcher_json()));
    if report.attach_system {
        section("System", &system());
    }
    if report.attach_logs {
        section("Last launch", &tail(&layout.logs_dir().join("last-launch.txt"), LAUNCHER_LOG_MAX));
        section("Launcher log", &tail(&layout.logs_dir().join("launcher.log"), LAUNCHER_LOG_MAX));
        section("Emulator log (shad_log.txt)", &tail(&layout.log_file(), EMULATOR_LOG_MAX));
    }
    hide_addresses(&hide_user(&redact(&s, &secrets), &user_names()), &named_hosts(layout))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secrets_never_survive() {
        let pw = "Hunter2xyz".to_string();
        let text = "Online ID: Player_1\nlogin with Hunter2xyz ok\n\"shadnet_password\": \"abc\",\nSHADPS4_VR=1\nauth token=zzz\n";
        let out = redact(text, &[pw]);
        assert!(!out.contains("Hunter2xyz") && !out.contains("abc") && !out.contains("zzz"));
        assert!(out.contains("Online ID: Player_1\n"));
        assert!(out.contains("SHADPS4_VR=1\n"));
        assert!(out.contains("\"shadnet_password\": [removed]\n"));
    }

    fn hidden(text: &str) -> String {
        hide_addresses(text, &[])
    }

    #[test]
    fn addresses_become_numbered_markers() {
        let text = "12:00:01 peer 203.0.113.7:3658 connected\n\
                    12:00:02 relay http://198.51.100.20:31316/x and 203.0.113.7 again, then 198.51.100.20.\n\
                    12:00:03 lan 192.168.1.20:3658, 10.0.0.5 and 100.64.3.1\n";
        assert_eq!(
            hidden(text),
            "12:00:01 peer [IP-1]:3658 connected\n\
             12:00:02 relay http://[IP-2]:31316/x and [IP-1] again, then [IP-2].\n\
             12:00:03 lan [LAN-IP-3]:3658, [LAN-IP-4] and [LAN-IP-5]\n"
        );
        let v6 = "a [2001:db8::17]:3658 b 2001:DB8:0:0:0:0:0:17, c fe80::1c2d%12 d ::ffff:203.0.113.7: e fd00::1.";
        assert_eq!(hidden(v6), "a [[IP-1]]:3658 b [IP-1], c [LAN-IP-2]%12 d [IP-3]: e [LAN-IP-4].");
    }

    #[test]
    fn versions_times_ports_and_loopback_stay() {
        for kept in [
            "Launcher: 0.2.1\n",
            "shadPS4 v0.16.1 WIP, FZH-Launcher v0.2.1\n",
            "Created: 2026-11-22 12:00:00\n",
            "[12:00:00.123] <Info> np.cpp:42 Core::Libraries::Np: bind 0.0.0.0:3658, backend http://127.0.0.1:1338/parse/\n",
            "listening on [::1]:31313 and [::]:31315, group 239.255.255.250, mask 255.255.255.0\n",
            "GPU: Synthetic Card, driver 32.0.15.6094\n",
            "GPU: Synthetic Card, driver 31.0.101.2\n",
            "Windows: Synthetic 10.0.26300 (build 26300), version: 10.0.1.2\n",
            "mac 00:1a:2b:3c:4d:5e, v1.2.3.4, 1.2.3.4.5, 1.2.3, 300.1.2.3, fade::in, std::dec\n",
            "\"frameRate\": \"90\", \"sharpness\": 0.5,\n",
            "caf\u{e9} \u{2014} 12:00\n",
        ] {
            assert_eq!(hidden(kept), kept);
        }
    }

    #[test]
    fn known_servers_are_named() {
        let named = vec![("relay.play.example".to_string(), "[relay 1]".to_string()), ("play.example".to_string(), "[private server]".to_string())];
        let text = "\"privateServer\": \"Play.Example:40001:40002:40003:40004:40005/relay.play.example:9000\",\n\
                    Cannot reach the private server play.example (port 40001). replay.example, play.example.org and my-play.example are others.\n";
        assert_eq!(
            hide_addresses(text, &named),
            "\"privateServer\": \"[private server]:40001:40002:40003:40004:40005/[relay 1]:9000\",\n\
             Cannot reach the private server [private server] (port 40001). replay.example, play.example.org and my-play.example are others.\n"
        );
        let named = vec![("203.0.113.60".to_string(), "[private server]".to_string())];
        assert_eq!(hide_addresses("203.0.113.60:31313 203.0.113.6 203.0.113.60", &named), "[private server]:31313 [IP-1] [private server]");
    }

    #[test]
    fn the_windows_user_name_becomes_a_marker() {
        let names = vec!["Synthetic User".to_string(), "synth".to_string()];
        let text = "cwd: C:\\Users\\Synthetic User\\AppData\\Local\\x\n\
                    \"path\": \"C:\\\\Users\\\\synthetic user\\\\Games\\\\CUSA11182\",\n\
                    file:///C:/Users/Synth/AppData and D:\\Games\\synth, owner SYNTH.\n\
                    Online ID: synth_1, Synthetic, resynth, synth-x\n";
        assert_eq!(
            hide_user(text, &names),
            "cwd: C:\\Users\\[user]\\AppData\\Local\\x\n\
             \"path\": \"C:\\\\Users\\\\[user]\\\\Games\\\\CUSA11182\",\n\
             file:///C:/Users/[user]/AppData and D:\\Games\\[user], owner [user].\n\
             Online ID: synth_1, Synthetic, resynth, synth-x\n"
        );
        let short = vec!["al".to_string()];
        let text = "C:\\Users\\al\\x, C:/Users/AL/x, \"C:\\\\Users\\\\al\\\\x\", al fresco, D:\\al\\x, C:\\Users\\alice\n";
        assert_eq!(hide_user(text, &short), "C:\\Users\\[user]\\x, C:/Users/[user]/x, \"C:\\\\Users\\\\[user]\\\\x\", al fresco, D:\\al\\x, C:\\Users\\alice\n");
        assert_eq!(hide_user("caf\u{e9} C:\\Users\\Ren\u{e9}e\\x", &["Ren\u{e9}e".to_string()]), "caf\u{e9} C:\\Users\\[user]\\x");
        assert_eq!(hide_user("nothing to hide", &[]), "nothing to hide");
    }

    fn data_folder(name: &str) -> (std::path::PathBuf, Layout) {
        let dir = std::env::temp_dir().join(format!("fzh-report-{name}-{}", std::process::id()));
        let none = std::path::PathBuf::from("missing");
        let layout = Layout { data: dir.clone(), emulator: none.clone(), patches: none.clone(), licences: none.clone(), warmup_script: none };
        std::fs::create_dir_all(layout.user_dir().join("log")).unwrap();
        std::fs::create_dir_all(layout.logs_dir()).unwrap();
        (dir, layout)
    }

    #[test]
    fn report_from_a_data_folder() {
        let (dir, layout) = data_folder("plain");
        let creds = users::Credentials { npid: "Synthetic_1".into(), password: "not-a-real-pw".into(), email: String::new() };
        instance::write_json(&layout.users_json(), &users::set_account(Value::Null, &creds)).unwrap();
        std::fs::write(layout.log_file(), "x Logged in npid='Synthetic_1' with not-a-real-pw\n").unwrap();
        std::fs::write(layout.logs_dir().join("launcher.log"), "12:00:00 Launcher: game started\n").unwrap();
        let profile = std::env::var("USERPROFILE").unwrap_or_default();
        std::fs::write(layout.logs_dir().join("last-launch.txt"), format!("cwd: {profile}\\AppData\\run\n")).unwrap();
        let r = ProblemReport { description: "It froze".into(), attach_logs: true, attach_system: false };
        let text = build(&layout, &r, "0.1.0");
        if let Some(name) = Path::new(&profile).file_name() {
            assert!(text.contains("\\[user]\\AppData\\run\n") && !text.contains(&format!("\\{}\\AppData", name.to_string_lossy())));
        }
        assert!(text.contains("Online ID: Synthetic_1"));
        assert!(text.contains("It froze"));
        assert!(text.contains("12:00:00 Launcher: game started"));
        assert!(text.contains("Launcher: 0.1.0\n"));
        assert!(!text.contains("not-a-real-pw"));
        assert!(!text.contains("===== System"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_report_names_no_server_and_no_peer() {
        let (dir, layout) = data_folder("servers");
        let settings = settings::Settings { private_server: "play.example/eu.relay.example,198.51.100.9:9000".into(), ..Default::default() };
        store::save_settings(&layout.settings_json(), &settings).unwrap();
        let mut known = accounts::Accounts::default();
        known.remember("old.example", "Synthetic_1", "");
        accounts::save(&layout.accounts_json(), &known).unwrap();
        std::fs::write(
            layout.logs_dir().join("launcher.log"),
            "12:00:00 Network: signed in as Synthetic_1 on the private server play.example\n12:00:05 Network: old.example gone\n",
        )
        .unwrap();
        std::fs::write(layout.log_file(), "relay eu.relay.example:31316 ok, 198.51.100.9:9000 ok, peer 203.0.113.7:3658, me 203.0.113.99\n").unwrap();
        let r = ProblemReport { description: "My server is play.example".into(), attach_logs: true, attach_system: false };
        let text = build(&layout, &r, "0.2.1");
        for leaked in ["play.example", "relay.example", "old.example", "198.51.100.9", "203.0.113."] {
            assert!(!text.contains(leaked), "{leaked}");
        }
        assert!(text.contains("\"privateServer\": \"[private server]/[relay 1],[relay 2]:9000\""));
        assert!(text.contains("My server is [private server]\n"));
        assert!(text.contains("12:00:05 Network: [server 1] gone\n"));
        assert!(text.contains("relay [relay 1]:31316 ok, [relay 2]:9000 ok, peer [IP-1]:3658, me [IP-2]\n"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
