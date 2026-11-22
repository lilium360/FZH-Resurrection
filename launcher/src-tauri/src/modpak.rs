//! The mod pak (reference implementation: `tools/modpak.py`): the game's own ini files with a few keys changed.

use crate::instance::Server;
use crate::pak;
use std::path::Path;

pub const DEFAULT_ENGINE: &str = "firewall/Config/DefaultEngine.ini";
pub const PS4_ENGINE: &str = "firewall/Config/PS4/PS4Engine.ini";

pub fn backend_url(server: &str) -> String {
    let at = Server::of(server);
    let host = if at.host.contains(':') && !at.host.starts_with('[') { format!("[{}]", at.host) } else { at.host };
    format!("http://{host}:{}/parse/", at.backend)
}

type Change = (String, String, String);

fn change(section: &str, key: &str, value: &str) -> Change {
    (section.to_string(), key.to_string(), value.to_string())
}

/// The changes per file for the inputs `[<players> [<countdown>]] <entries...> [--server=s]`. An
/// entry always holds `=`; without players and countdown the game's own lobby rules apply.
pub fn changes(inputs: &[String]) -> (Vec<Change>, Vec<Change>) {
    let server = inputs.iter().filter_map(|a| a.strip_prefix("--server=")).last().filter(|s| !s.is_empty());
    let backend = backend_url(server.unwrap_or("127.0.0.1"));
    let quoted = format!("\"{backend}\"");
    let mut default_engine = vec![
        change("WWSZygo", "bEnabled", "false"),
        change("FCBackend", "ApiBaseURL", &quoted),
        change("FCBackend", "GatewayURL", &quoted),
    ];
    let mut ps4_engine = vec![
        change("/Script/OnlineSubsystemUtils.IpNetDriver", "ConnectionTimeout", "300.0"),
        change("/Script/OnlineSubsystemUtils.IpNetDriver", "InitialConnectTimeout", "300.0"),
    ];
    let args: Vec<&String> = inputs.iter().filter(|a| !a.starts_with("--server=")).collect();
    let lobby: Vec<&String> = args.iter().copied().filter(|a| !a.contains('=')).collect();
    if let Some(players) = lobby.first() {
        default_engine.push(change("SystemSettings", "FCDebug.Session.PVEPlayers", players));
    }
    if let Some(countdown) = lobby.get(1) {
        default_engine.push(change("SystemSettings", "FCDebug.Session.LobbyLifeTime", countdown));
    }
    for entry in args.iter().filter(|a| a.contains('=')) {
        if let Some(rest) = entry.strip_prefix('[') {
            let (section, rest) = rest.split_once(']').unwrap_or((rest, ""));
            let (name, value) = rest.split_once('=').unwrap_or((rest, ""));
            default_engine.push(change(section, name, value));
        } else {
            let (name, value) = entry.split_once('=').unwrap_or((entry, ""));
            ps4_engine.push(change("SystemSettings", name, value));
        }
    }
    (default_engine, ps4_engine)
}

fn key_matches(line: &str, key: &str) -> bool {
    let rest = line.trim_start();
    let (rb, kb) = (rest.as_bytes(), key.as_bytes());
    if rb.len() < kb.len() || !rb[..kb.len()].eq_ignore_ascii_case(kb) {
        return false;
    }
    rest[kb.len()..].trim_start().starts_with('=')
}

/// Sets key=value in [section], creating the section at the end if missing; keeps line endings.
pub fn set_ini(text: &str, section: &str, key: &str, value: &str) -> String {
    let mut lines: Vec<String> = text.split('\n').map(str::to_string).collect();
    let header = format!("[{section}]");
    let Some(start) = lines.iter().position(|l| l.trim() == header) else {
        return format!("{}\r\n\r\n[{section}]\r\n{key}={value}\r\n", text.trim_end_matches(['\r', '\n']));
    };
    let end = (start + 1..lines.len()).find(|&i| lines[i].trim().starts_with('[')).unwrap_or(lines.len());
    let cr = if lines[start].ends_with('\r') { "\r" } else { "" };
    let new_line = format!("{key}={value}{cr}");
    match (start + 1..end).find(|&i| key_matches(&lines[i], key)) {
        Some(i) => lines[i] = new_line,
        None => lines.insert(start + 1, new_line),
    }
    lines.join("\n")
}

fn decode(bytes: &[u8]) -> Result<String, String> {
    let bytes = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes);
    String::from_utf8(bytes.to_vec()).map_err(|_| "the game's ini file is not UTF-8".to_string())
}

/// Applies the changes to the two ini files and packs them.
pub fn build_from_ini(default_engine: &[u8], ps4_engine: &[u8], inputs: &[String]) -> Result<Vec<u8>, String> {
    let (de_changes, ps_changes) = changes(inputs);
    let mut de = decode(default_engine)?;
    for (s, k, v) in &de_changes {
        de = set_ini(&de, s, k, v);
    }
    let mut ps = decode(ps4_engine)?;
    for (s, k, v) in &ps_changes {
        ps = set_ini(&ps, s, k, v);
    }
    Ok(pak::write(&[(DEFAULT_ENGINE.to_string(), de.into_bytes()), (PS4_ENGINE.to_string(), ps.into_bytes())]))
}

pub fn build(game_pak: &Path, inputs: &[String]) -> Result<Vec<u8>, String> {
    let files = pak::read_files(game_pak, &[DEFAULT_ENGINE, PS4_ENGINE])?;
    build_from_ini(&files[0], &files[1], inputs)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_existing_key_keeps_crlf() {
        let t = "[A]\r\nfoo=1\r\n[B]\r\nfoo=2\r\n";
        assert_eq!(set_ini(t, "B", "FOO", "3"), "[A]\r\nfoo=1\r\n[B]\r\nFOO=3\r\n");
        assert_eq!(set_ini(t, "A", "foo", "9"), "[A]\r\nfoo=9\r\n[B]\r\nfoo=2\r\n");
    }

    #[test]
    fn set_missing_key_inserts_after_header() {
        let t = "[A]\r\nfoo=1\r\n";
        assert_eq!(set_ini(t, "A", "bar", "x"), "[A]\r\nbar=x\r\nfoo=1\r\n");
        assert_eq!(set_ini("[A]\nfoo=1\n", "A", "bar", "x"), "[A]\nbar=x\nfoo=1\n");
    }

    #[test]
    fn set_missing_section_appends() {
        assert_eq!(set_ini("[A]\r\nfoo=1\r\n\r\n", "B", "k", "v"), "[A]\r\nfoo=1\r\n\r\n[B]\r\nk=v\r\n");
    }

    #[test]
    fn commented_and_prefixed_keys_do_not_match() {
        let t = "[A]\r\n;foo=1\r\nfoobar=2\r\n  foo  = 3\r\n";
        assert_eq!(set_ini(t, "A", "foo", "4"), "[A]\r\n;foo=1\r\nfoobar=2\r\nfoo=4\r\n");
    }

    #[test]
    fn changes_follow_the_inputs() {
        let strings = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<String>>();
        let inputs = strings(&["vr.PixelDensity=2.0", "[/Script/Engine.Engine]Smoothed=(A=1,B=2)", "--server=10.0.0.5"]);
        let (de, ps) = changes(&inputs);
        assert_eq!(de[1], change("FCBackend", "ApiBaseURL", "\"http://10.0.0.5:1338/parse/\""));
        assert_eq!(de[3], change("/Script/Engine.Engine", "Smoothed", "(A=1,B=2)"));
        assert_eq!(de.len(), 4, "no lobby keys without players and countdown");
        assert_eq!(ps[2], change("SystemSettings", "vr.PixelDensity", "2.0"));
        let (de, ps) = changes(&strings(&["1", "8", "vr.PixelDensity=2.0"]));
        assert_eq!(de[2].2, "\"http://127.0.0.1:1338/parse/\"");
        assert_eq!(de[3], change("SystemSettings", "FCDebug.Session.PVEPlayers", "1"));
        assert_eq!(de[4], change("SystemSettings", "FCDebug.Session.LobbyLifeTime", "8"));
        assert_eq!(ps[2], change("SystemSettings", "vr.PixelDensity", "2.0"));
    }

    #[test]
    fn ipv6_backend_is_bracketed() {
        assert_eq!(backend_url("::1"), "http://[::1]:1338/parse/");
        assert_eq!(backend_url("box.example:40001:40002:40003:40004:40001"), "http://box.example:40004/parse/");
    }

    /// Golden pak written by `tools/modpak.py mod --ini-dir tests/fixtures/pak-src` with the lines of mod-inputs.txt.
    #[test]
    fn byte_identical_to_the_reference() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests").join("fixtures");
        let src = root.join("pak-src").join("firewall").join("Config");
        let de = std::fs::read(src.join("DefaultEngine.ini")).unwrap();
        let ps = std::fs::read(src.join("PS4").join("PS4Engine.ini")).unwrap();
        let inputs: Vec<String> = std::fs::read_to_string(root.join("mod-inputs.txt"))
            .unwrap()
            .lines()
            .filter(|l| !l.is_empty())
            .map(str::to_string)
            .collect();
        let golden = std::fs::read(root.join("mod-reference.pak")).unwrap();
        assert_eq!(build_from_ini(&de, &ps, &inputs).unwrap(), golden);
    }
}
