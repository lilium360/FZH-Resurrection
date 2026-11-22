//! The emulator instance under `<data>\run\user`: config.json, host_overrides.json and the patches.

use crate::error::{BackendError, Res};
use crate::layout::TITLE_ID;
use crate::settings::Settings;
use crate::store::write_atomic;
use serde_json::{json, Map, Value};
use std::path::Path;

pub const SHADNET_PORT: u16 = 31313;
pub const WEBAPI_PORT: u16 = 31315;
pub const BACKEND_PORT: u16 = 1338;
pub const RELAY_PORT: u16 = 31316;

/// Where the online services of one server listen. An address is `host`, with the standard ports,
/// or `host:game:matching:webapi:backend:relay` for a host that hands out its own port numbers
/// (TCP game, UDP matching, TCP WebAPI, TCP backend, UDP relay). Either form may end in
/// `/relayhost[:port]` for a relay on another machine (port 31316 unless given), or a list of
/// them separated by `,` (at most `MAX_RELAYS`, no duplicates): the emulator registers with all
/// and agrees with each peer on one. The relay port of the first part is then unused. `/` and
/// `,` cannot appear in a host name.
#[derive(Clone, Debug, PartialEq)]
pub struct Server {
    pub host: String,
    pub game: u16,
    pub matching: u16,
    pub webapi: u16,
    pub backend: u16,
    pub relays: Vec<(String, u16)>,
}

/// The emulator uses no more than this many relays.
pub const MAX_RELAYS: usize = 8;

fn port(p: &str) -> Option<u16> {
    p.parse::<u16>().ok().filter(|&n| n != 0)
}

impl Server {
    fn standard_at(host: &str) -> Server {
        Server {
            host: host.to_string(),
            game: SHADNET_PORT,
            matching: SHADNET_PORT + 1,
            webapi: WEBAPI_PORT,
            backend: BACKEND_PORT,
            relays: vec![(host.to_string(), RELAY_PORT)],
        }
    }

    pub fn parse(address: &str) -> Option<Server> {
        let (services, relay) = match address.split_once('/') {
            Some((s, r)) => (s, Some(r)),
            None => (address, None),
        };
        let mut parts = services.split(':');
        let host = parts.next().filter(|h| crate::settings::valid_host(h))?;
        let ports = parts.map(port).collect::<Option<Vec<_>>>()?;
        let mut server = match ports[..] {
            [] => Server::standard_at(host),
            [game, matching, webapi, backend, relay] => Server {
                game,
                matching,
                webapi,
                backend,
                relays: vec![(host.to_string(), relay)],
                ..Server::standard_at(host)
            },
            _ => return None,
        };
        if let Some(list) = relay {
            let mut relays: Vec<(String, u16)> = Vec::new();
            for entry in list.split(',') {
                let (relay_host, relay_port) = match entry.split_once(':') {
                    Some((h, p)) => (h, port(p)?),
                    None => (entry, RELAY_PORT),
                };
                if !crate::settings::valid_host(relay_host)
                    || relays.iter().any(|(h, p)| h.eq_ignore_ascii_case(relay_host) && *p == relay_port)
                {
                    return None;
                }
                relays.push((relay_host.to_string(), relay_port));
            }
            if relays.len() > MAX_RELAYS {
                return None;
            }
            server.relays = relays;
        }
        Some(server)
    }

    /// An address that does not parse is taken as a bare host (an IPv6 literal, for one).
    pub fn of(address: &str) -> Server {
        Server::parse(address).unwrap_or_else(|| Server::standard_at(address))
    }

    /// The address without its relay part: what the mod pak needs.
    pub fn services(address: &str) -> &str {
        address.split_once('/').map_or(address, |(s, _)| s)
    }

    /// `relayhost:port[,relayhost:port...]` when the emulator cannot find the relays by itself:
    /// without it the emulator uses one relay at the shadNet host, port 31316.
    pub fn relay_override(&self) -> Option<String> {
        (self.relays != [(self.host.clone(), RELAY_PORT)])
            .then(|| self.relays.iter().map(|(h, p)| format!("{h}:{p}")).collect::<Vec<_>>().join(","))
    }
}

fn section<'a>(root: &'a mut Map<String, Value>, name: &str) -> &'a mut Map<String, Value> {
    let entry = root.entry(name.to_string()).or_insert_with(|| json!({}));
    if !entry.is_object() {
        *entry = json!({});
    }
    entry.as_object_mut().unwrap()
}

/// Sets the keys the launcher owns in config.json and keeps every other key. The emulator merges a
/// partial file with its defaults, so a new instance starts from an empty object.
pub fn edit_config(config: Value, settings: &Settings, server: &str) -> Value {
    let mut root = match config {
        Value::Object(m) => m,
        _ => Map::new(),
    };
    let general = section(&mut root, "General");
    general.insert("shad_net_enabled".into(), json!(true));
    general.insert("connected_to_network".into(), json!(true));
    let at = Server::of(server);
    general.insert("shadnet_server".into(), json!(format!("{}:{}", at.host, at.game)));
    general.insert("shadnet_webapi_server".into(), json!(format!("http://{}:{}", at.host, at.webapi)));
    let input = section(&mut root, "Input");
    input.insert("use_special_pad".into(), json!(settings.controller == "aim"));
    input.insert("special_pad_class".into(), json!(9));
    let audio = section(&mut root, "Audio");
    audio.insert("sdl_mic_device".into(), json!(settings.mic_device()));
    audio.insert("sdl_main_output_device".into(), json!(settings.output_device()));
    // Off by default in the emulator: without it no compiled pipeline reaches user\cache\<title>.
    section(&mut root, "Vulkan").insert("pipeline_cache_enabled".into(), json!(true));
    Value::Object(root)
}

/// The shadNet WebAPI stays on the shadNet server; every other HTTP request of the game goes to the backend.
pub fn edit_host_overrides(existing: Value, server: &str) -> Value {
    let mut map = match existing {
        Value::Object(m) => m,
        _ => Map::new(),
    };
    let at = Server::of(server);
    let webapi = format!("{}:{}", at.host, at.webapi);
    // An entry that maps a host to itself is the WebAPI exemption of an earlier server.
    map.retain(|k, v| k == "*" || v.as_str() != Some(format!("http://{k}").as_str()));
    map.insert(webapi.clone(), json!(format!("http://{webapi}")));
    map.insert("*".into(), json!(format!("http://{}:{}", at.host, at.backend)));
    Value::Object(map)
}

pub fn read_json(path: &Path) -> Value {
    std::fs::read(path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or(Value::Null)
}

pub fn write_json(path: &Path, v: &Value) -> Res<()> {
    let bytes = serde_json::to_vec_pretty(v).map_err(|e| BackendError::internal(e.to_string()))?;
    write_atomic(path, &bytes)
}

pub fn apply(user_dir: &Path, settings: &Settings) -> Res<()> {
    std::fs::create_dir_all(user_dir).map_err(|e| BackendError::io_path(user_dir.display(), e))?;
    let server = settings.server();
    let config_path = user_dir.join("config.json");
    write_json(&config_path, &edit_config(read_json(&config_path), settings, &server))?;
    let overrides_path = user_dir.join("host_overrides.json");
    write_json(&overrides_path, &edit_host_overrides(read_json(&overrides_path), &server))
}

/// The `Name` of the optional patch that the "free turn" mod switches on.
pub const FREE_TURN_PATCH: &str = "No turn-around warning";

/// A patch file with the optional patch named `name` switched on or off (`isEnabled`).
pub fn with_patch_enabled(xml: &[u8], name: &str, enabled: bool) -> Vec<u8> {
    let Ok(text) = std::str::from_utf8(xml) else { return xml.to_vec() };
    let Some(at) = text.find(&format!("Name=\"{name}\"")) else { return xml.to_vec() };
    let (Some(start), Some(len)) = (text[..at].rfind("<Metadata"), text[at..].find('>')) else { return xml.to_vec() };
    let tag = &text[start..at + len];
    let (from, to) = if enabled { ("isEnabled=\"false\"", "isEnabled=\"true\"") } else { ("isEnabled=\"true\"", "isEnabled=\"false\"") };
    format!("{}{}{}", &text[..start], tag.replace(from, to), &text[at + len..]).into_bytes()
}

fn lists_title(titles: &Value) -> bool {
    titles.as_array().is_some_and(|l| l.iter().any(|t| t == TITLE_ID))
}

/// Copies the bundled patch files into `user\patches\fzh` and registers each for the game in
/// files.json. A file registered for the game that is no longer bundled is unregistered, and
/// deleted when no other title uses it: the emulator applies one patch file per game.
pub fn install_patches(source: &Path, user_dir: &Path, settings: &Settings) -> Res<Vec<String>> {
    let target = user_dir.join("patches").join("fzh");
    std::fs::create_dir_all(&target).map_err(|e| BackendError::io_path(target.display(), e))?;
    let entries = std::fs::read_dir(source)
        .map_err(|e| BackendError::io_path(source.display(), e))?;
    let mut names = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()).is_some_and(|e| e.eq_ignore_ascii_case("xml")) && path.is_file() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let bytes = std::fs::read(&path).map_err(|e| BackendError::io_path(path.display(), e))?;
            write_atomic(&target.join(&name), &with_patch_enabled(&bytes, FREE_TURN_PATCH, settings.mod_free_turn))?;
            names.push(name);
        }
    }
    if names.is_empty() {
        return Err(BackendError::new("io", "backend.files_missing", format!("No emulator patches in {}", source.display())));
    }
    names.sort();
    let index_path = target.join("files.json");
    let mut index = match read_json(&index_path) {
        Value::Object(m) => m,
        _ => Map::new(),
    };
    let stale: Vec<String> = index.iter().filter(|(n, t)| !names.contains(n) && lists_title(t)).map(|(n, _)| n.clone()).collect();
    for n in &stale {
        let Some(Value::Array(titles)) = index.get_mut(n) else { continue };
        titles.retain(|t| t != TITLE_ID);
        if titles.is_empty() {
            index.remove(n);
            let file = target.join(n);
            if file.is_file() {
                std::fs::remove_file(&file).map_err(|e| BackendError::io_path(file.display(), e))?;
            }
        }
    }
    for n in &names {
        index.insert(n.clone(), json!([TITLE_ID]));
    }
    write_json(&index_path, &Value::Object(index))?;
    Ok(names)
}

pub fn patches_installed(source: &Path, user_dir: &Path) -> bool {
    let target = user_dir.join("patches").join("fzh");
    let Ok(entries) = std::fs::read_dir(source) else { return false };
    let index = read_json(&target.join("files.json"));
    let mut bundled = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if !name.to_ascii_lowercase().ends_with(".xml") {
            continue;
        }
        bundled.push(name.clone());
        let installed = std::fs::read(target.join(&name)).ok();
        let same = std::fs::read(entry.path()).ok().is_some_and(|a| {
            [true, false].iter().any(|on| installed.as_deref() == Some(&with_patch_enabled(&a, FREE_TURN_PATCH, *on)[..]))
        });
        if !same || index.get(&name).is_none() {
            return false;
        }
    }
    let stale = index.as_object().is_some_and(|m| m.iter().any(|(n, t)| !bundled.contains(n) && lists_title(t)));
    !bundled.is_empty() && !stale
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_keeps_unknown_keys_and_sets_ours() {
        let before = json!({
            "General": {"shadnet_server": "1.2.3.4:31313", "volume_slider": 80},
            "Input": {"use_special_pad": false, "special_pad_class": 1},
            "GPU": {"present_mode": "Mailbox"}
        });
        let s = Settings { microphone: "off".into(), controller: "aim".into(), ..Settings::default() };
        let after = edit_config(before, &s, "10.0.0.7");
        assert_eq!(after["General"]["shadnet_server"], "10.0.0.7:31313");
        assert_eq!(after["General"]["shadnet_webapi_server"], "http://10.0.0.7:31315");
        assert_eq!(after["General"]["shad_net_enabled"], true);
        assert_eq!(after["General"]["connected_to_network"], true);
        assert_eq!(after["General"]["volume_slider"], 80);
        assert_eq!(after["Input"]["use_special_pad"], true);
        assert_eq!(after["Input"]["special_pad_class"], 9);
        assert_eq!(after["Audio"]["sdl_mic_device"], "None");
        assert_eq!(after["Audio"]["sdl_main_output_device"], "Default Device");
        assert_eq!(after["GPU"]["present_mode"], "Mailbox");
        let keys: Vec<&String> = after.as_object().unwrap().keys().collect();
        assert_eq!(after["Vulkan"]["pipeline_cache_enabled"], true);
        assert_eq!(keys, ["General", "Input", "GPU", "Audio", "Vulkan"]);
    }

    #[test]
    fn config_from_nothing() {
        let after = edit_config(Value::Null, &Settings { controller: "pad".into(), ..Settings::default() }, "127.0.0.1");
        assert_eq!(after["Input"]["use_special_pad"], false);
        assert_eq!(after["General"]["shadnet_server"], "127.0.0.1:31313");
    }

    #[test]
    fn host_overrides_replace_server() {
        let before = json!({"_note": "x", "127.0.0.1:31315": "http://127.0.0.1:31315", "*": "http://127.0.0.1:1338"});
        let after = edit_host_overrides(before, "host.example");
        assert_eq!(
            after,
            json!({"_note": "x", "host.example:31315": "http://host.example:31315", "*": "http://host.example:1338"})
        );
        assert_eq!(edit_host_overrides(Value::Null, "127.0.0.1")["*"], "http://127.0.0.1:1338");
        let earlier = json!({"127.0.0.1:31315": "http://127.0.0.1:31315", "*": "http://127.0.0.1:1338"});
        let custom = edit_host_overrides(earlier, "box.example:40001:40002:40003:40004:40001");
        assert_eq!(custom, json!({"box.example:40003": "http://box.example:40003", "*": "http://box.example:40004"}));
    }

    #[test]
    fn server_addresses() {
        let plain = Server::parse("10.0.0.7").unwrap();
        assert_eq!((plain.game, plain.matching, plain.webapi, plain.backend), (31313, 31314, 31315, 1338));
        assert_eq!((plain.relays.clone(), plain.relay_override()), (vec![("10.0.0.7".to_string(), 31316)], None));
        let custom = Server::parse("box.example:40001:40002:40003:40004:40001").unwrap();
        assert_eq!((custom.host.as_str(), custom.game, custom.matching, custom.webapi, custom.backend), ("box.example", 40001, 40002, 40003, 40004));
        assert_eq!(custom.relay_override().as_deref(), Some("box.example:40001"));
        let apart = Server::parse("play.example/relay.example").unwrap();
        assert_eq!((apart.host.as_str(), apart.game, apart.backend), ("play.example", 31313, 1338));
        assert_eq!(apart.relay_override().as_deref(), Some("relay.example:31316"));
        let both = Server::parse("box.example:40001:40002:40003:40004:40005/relay.example:9000").unwrap();
        assert_eq!((both.matching, both.relays), (40002, vec![("relay.example".to_string(), 9000)]));
        assert_eq!(Server::parse("10.0.0.7/10.0.0.7").unwrap().relay_override(), None);
        assert_eq!(Server::services("box.example:1:2:3:4:5/relay.example:9"), "box.example:1:2:3:4:5");
        assert_eq!(Server::services("box.example"), "box.example");
        for bad in [
            "", "a b", "host:1", "host:1:2:3:4", "host:1:2:3:4:0", "host:1:2:3:4:70000", "host:1:2:3:4:5:6",
            "host/", "/relay", "host/relay:", "host/relay:0", "host/relay:1:2", "host/a/b", "host/a b",
            "host:1:2:3:4/relay",
            "host/r1,", "host/,r1", "host/r1,,r2", "host/r1, r2", "host/r1 ,r2", "host/r1,r2:0", "host/r1,r2:x",
            "host/r1,R1", "host/r1,r1:31316", "host/r1:9,r1:9", "host/1,2,3,4,5,6,7,8,9", "host,r1", "host/r1;r2",
        ] {
            assert_eq!(Server::parse(bad), None, "{bad}");
        }
        assert_eq!(Server::of("::1").host, "::1");
        let list = Server::parse("play.example/eu.example,us.example:9000,10.0.0.9").unwrap();
        assert_eq!(list.relay_override().as_deref(), Some("eu.example:31316,us.example:9000,10.0.0.9:31316"));
        assert_eq!(Server::services("play.example/eu.example,us.example:9000"), "play.example");
        let same_host = Server::parse("play.example/play.example,us.example").unwrap();
        assert_eq!(same_host.relay_override().as_deref(), Some("play.example:31316,us.example:31316"));
        assert_eq!(Server::parse("play.example/r1:9,r1:10").unwrap().relays.len(), 2);
        assert!(Server::parse("h/1,2,3,4,5,6,7,8").is_some());
        let config = edit_config(Value::Null, &Settings::default(), "box.example:40001:40002:40003:40004:40001");
        assert_eq!(config["General"]["shadnet_server"], "box.example:40001");
        assert_eq!(config["General"]["shadnet_webapi_server"], "http://box.example:40003");
    }

    #[test]
    fn optional_patch_follows_the_setting() {
        let xml = br#"<Patch>
  <Metadata Name="Always" isEnabled="true"><PatchList/></Metadata>
  <Metadata Title="T" Name="No turn-around warning" Note="n" isEnabled="false">
    <PatchList><Line Type="bytes" Address="0x1" Value="90"/></PatchList>
  </Metadata>
</Patch>"#;
        let on = String::from_utf8(with_patch_enabled(xml, FREE_TURN_PATCH, true)).unwrap();
        assert!(on.contains(r#"Name="No turn-around warning" Note="n" isEnabled="true""#));
        assert!(on.contains(r#"Name="Always" isEnabled="true""#));
        assert_eq!(with_patch_enabled(on.as_bytes(), FREE_TURN_PATCH, false), xml.to_vec());
        assert_eq!(with_patch_enabled(b"<a/>", FREE_TURN_PATCH, true), b"<a/>".to_vec());

        let root = std::env::temp_dir().join(format!("fzh-mod-test-{}", std::process::id()));
        let (src, user) = (root.join("src"), root.join("user"));
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("P.xml"), xml).unwrap();
        let settings = Settings { mod_free_turn: true, ..Settings::default() };
        install_patches(&src, &user, &settings).unwrap();
        let written = std::fs::read_to_string(user.join("patches").join("fzh").join("P.xml")).unwrap();
        assert_eq!(written, on);
        assert!(patches_installed(&src, &user));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn patches_are_copied_and_registered() {
        let root = std::env::temp_dir().join(format!("fzh-patch-test-{}", std::process::id()));
        let src = root.join("src");
        let user = root.join("user");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("A.xml"), b"<a/>").unwrap();
        std::fs::write(src.join("readme.txt"), b"no").unwrap();
        std::fs::create_dir_all(user.join("patches").join("fzh")).unwrap();
        std::fs::write(user.join("patches").join("fzh").join("files.json"), br#"{"Other.xml": ["CUSA00001"]}"#).unwrap();
        assert!(!patches_installed(&src, &user));
        assert_eq!(install_patches(&src, &user, &Settings::default()).unwrap(), vec!["A.xml"]);
        assert!(patches_installed(&src, &user));
        let index = read_json(&user.join("patches").join("fzh").join("files.json"));
        assert_eq!(index, json!({"Other.xml": ["CUSA00001"], "A.xml": ["CUSA11182"]}));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_renamed_patch_replaces_the_old_file() {
        let root = std::env::temp_dir().join(format!("fzh-stale-test-{}", std::process::id()));
        let (src, user) = (root.join("src"), root.join("user"));
        let fzh = user.join("patches").join("fzh");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::create_dir_all(&fzh).unwrap();
        std::fs::write(src.join("New.xml"), b"<n/>").unwrap();
        for f in ["Old.xml", "Shared.xml", "Other.xml"] {
            std::fs::write(fzh.join(f), b"<o/>").unwrap();
        }
        let before = json!({"Old.xml": ["CUSA11182"], "Shared.xml": ["CUSA11182", "CUSA00001"], "Other.xml": ["CUSA00001"], "New.xml": ["CUSA11182"]});
        write_json(&fzh.join("files.json"), &before).unwrap();
        std::fs::write(fzh.join("New.xml"), b"<n/>").unwrap();
        assert!(!patches_installed(&src, &user), "a stale entry for the game is not a finished installation");
        assert_eq!(install_patches(&src, &user, &Settings::default()).unwrap(), vec!["New.xml"]);
        let index = read_json(&fzh.join("files.json"));
        assert_eq!(index, json!({"Shared.xml": ["CUSA00001"], "Other.xml": ["CUSA00001"], "New.xml": ["CUSA11182"]}));
        assert!(!fzh.join("Old.xml").exists());
        assert!(fzh.join("Shared.xml").is_file() && fzh.join("Other.xml").is_file());
        assert!(patches_installed(&src, &user));
        let _ = std::fs::remove_dir_all(&root);
    }
}
