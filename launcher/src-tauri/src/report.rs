//! The problem report: one text file the player saves and sends to whoever gave them the launcher.
//! It keeps the Online ID; the password, tokens and keys never go in it.

use crate::game::CREATE_NO_WINDOW;
use crate::layout::Layout;
use crate::{clock, instance, users};
use serde::Deserialize;
use serde_json::Value;
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
            "Created: {}\nLauncher: {version}\nEmulator commit: {}\nOnline ID: {}\nSign-in credentials are left out of this report.\n",
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
    redact(&s, &secrets)
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

    #[test]
    fn report_from_a_data_folder() {
        let dir = std::env::temp_dir().join(format!("fzh-report-{}", std::process::id()));
        let layout = Layout::resolve(dir.clone(), None);
        std::fs::create_dir_all(layout.user_dir().join("log")).unwrap();
        std::fs::create_dir_all(layout.logs_dir()).unwrap();
        let creds = users::Credentials { npid: "Synthetic_1".into(), password: "not-a-real-pw".into(), email: String::new() };
        instance::write_json(&layout.users_json(), &users::set_account(Value::Null, &creds)).unwrap();
        std::fs::write(layout.log_file(), "x Logged in npid='Synthetic_1' with not-a-real-pw\n").unwrap();
        std::fs::write(layout.logs_dir().join("launcher.log"), "12:00:00 Launcher: game started\n").unwrap();
        let r = ProblemReport { description: "It froze".into(), attach_logs: true, attach_system: false };
        let text = build(&layout, &r, "0.1.0");
        assert!(text.contains("Online ID: Synthetic_1"));
        assert!(text.contains("It froze"));
        assert!(text.contains("Launcher: game started"));
        assert!(!text.contains("not-a-real-pw"));
        assert!(!text.contains("===== System"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
