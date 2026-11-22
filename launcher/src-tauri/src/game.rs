//! Starting, watching and stopping the emulator.

use crate::error::Msg;
use std::collections::HashSet;
use std::ffi::OsString;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

#[cfg(windows)]
use std::os::windows::process::CommandExt;

pub const CREATE_NO_WINDOW: u32 = 0x0800_0000;

#[derive(Debug, Clone, PartialEq)]
pub struct LaunchPlan {
    pub program: PathBuf,
    pub args: Vec<OsString>,
    pub cwd: PathBuf,
    /// Variables to set; an empty value removes the variable.
    pub env: Vec<(&'static str, String)>,
}

impl LaunchPlan {
    pub fn new(emulator: &Path, eboot: &Path, run_dir: &Path, env: Vec<(&'static str, String)>) -> Self {
        LaunchPlan {
            program: emulator.to_path_buf(),
            args: vec!["-g".into(), eboot.as_os_str().to_owned()],
            cwd: run_dir.to_path_buf(),
            env,
        }
    }

    pub fn command_line(&self) -> String {
        let mut s = format!("\"{}\"", self.program.display());
        for a in &self.args {
            let a = a.to_string_lossy();
            if a.contains(' ') { s.push_str(&format!(" \"{a}\"")) } else { s.push_str(&format!(" {a}")) }
        }
        s
    }

    pub fn spawn(&self) -> std::io::Result<Child> {
        let mut cmd = Command::new(&self.program);
        cmd.args(&self.args).current_dir(&self.cwd).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
        for (k, v) in &self.env {
            if v.is_empty() {
                cmd.env_remove(k);
            } else {
                cmd.env(k, v);
            }
        }
        #[cfg(windows)]
        cmd.creation_flags(CREATE_NO_WINDOW);
        cmd.spawn()
    }
}

/// A child process that Windows kills when the launcher goes away, even if the launcher crashes
/// (a job object with `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`), and that is killed when this is dropped.
pub struct Owned {
    pub child: Child,
    #[cfg(windows)]
    _job: Option<Job>,
}

impl Owned {
    pub fn new(child: Child) -> Owned {
        #[cfg(windows)]
        {
            let job = Job::for_child(&child);
            Owned { child, _job: job }
        }
        #[cfg(not(windows))]
        Owned { child }
    }

    pub fn kill(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for Owned {
    fn drop(&mut self) {
        if matches!(self.child.try_wait(), Ok(None)) {
            self.kill();
        }
    }
}

#[cfg(windows)]
struct Job(windows_sys::Win32::Foundation::HANDLE);

#[cfg(windows)]
unsafe impl Send for Job {}

#[cfg(windows)]
impl Job {
    fn for_child(child: &Child) -> Option<Job> {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::System::JobObjects::{
            AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation, SetInformationJobObject,
            JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        };
        unsafe {
            let handle = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if handle.is_null() {
                return None;
            }
            let job = Job(handle);
            let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            let ok = SetInformationJobObject(
                handle,
                JobObjectExtendedLimitInformation,
                &info as *const _ as *const std::ffi::c_void,
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            ) != 0
                && AssignProcessToJobObject(handle, child.as_raw_handle()) != 0;
            ok.then_some(job)
        }
    }
}

#[cfg(windows)]
impl Drop for Job {
    fn drop(&mut self) {
        unsafe { windows_sys::Win32::Foundation::CloseHandle(self.0) };
    }
}

/// What one warm-up round did, read from its shad_log.txt.
#[derive(Debug, Default, PartialEq)]
pub struct RoundLog {
    pub new_pipelines: usize,
    pub preloaded: Option<u64>,
    pub matched: bool,
    pub signed_in: bool,
    pub crashed: bool,
}

pub fn read_round(log: &str) -> RoundLog {
    let mut r = RoundLog::default();
    for line in log.lines() {
        if line.contains("Compiling graphics pipeline") {
            r.new_pipelines += 1;
        } else if line.contains("sceNpMatching2SearchRoom: called") {
            r.matched = true;
        } else if line.contains("HandleLoginReply: Logged in npid=") {
            r.signed_in = true;
        } else if line.contains("Assertion Failed") || line.contains("Unhandled Exception code 0xc") {
            r.crashed = true;
        } else if r.preloaded.is_none() {
            r.preloaded = between(line, "Preloaded ", ' ').and_then(|n| n.parse().ok()).filter(|_| line.contains(" pipelines"));
        }
    }
    r
}

/// Seconds a warm-up round lasts: the end of the script's last input plus 10 s, rounded up
/// (`tools/input/warmup.txt` ends at 334.9 s, so a round lasts 345 s).
pub fn script_seconds(script: &str) -> Option<u64> {
    let end = script
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .filter_map(|l| {
            let mut it = l.split_whitespace();
            Some(it.next()?.parse::<f64>().ok()? + it.next()?.parse::<f64>().ok()?)
        })
        .fold(None, |m: Option<f64>, v| Some(m.map_or(v, |m| m.max(v))))?;
    Some(end.ceil() as u64 + 10)
}

/// Whether any shadps4.exe runs on this PC, asked through tasklist.
pub fn any_emulator_running() -> bool {
    let mut cmd = Command::new("tasklist");
    cmd.args(["/FI", "IMAGENAME eq shadps4.exe", "/NH"]).stdin(Stdio::null()).stderr(Stdio::null());
    #[cfg(windows)]
    cmd.creation_flags(CREATE_NO_WINDOW);
    cmd.output()
        .map(|o| String::from_utf8_lossy(&o.stdout).to_ascii_lowercase().contains("shadps4.exe"))
        .unwrap_or(false)
}

fn between<'a>(line: &'a str, start: &str, end: char) -> Option<&'a str> {
    let i = line.find(start)? + start.len();
    let rest = &line[i..];
    Some(&rest[..rest.find(end).unwrap_or(rest.len())])
}

/// Translates an emulator log line into a friendly milestone. The key makes one-off milestones appear once.
pub fn milestone(line: &str) -> Option<(&'static str, Msg)> {
    let (key, text) = match between(line, "Connect: ", '\n').and_then(|rest| rest.split_once(" has no headset connected yet")) {
        // The runtime is named: a headset that runs through another program never shows up in this one.
        Some((runtime, _)) => {
            let english = format!("OpenXR: {runtime} has no headset connected. The game starts in a window. If your headset runs through a different program, set that one as the active OpenXR runtime and start the game again.");
            return Some(("xr-wait", Msg::new("log.event.headsetWaiting", english).with("runtime", runtime)));
        }
        None => plain_milestone(line)?,
    };
    Some((key, Msg::raw(text)))
}

fn plain_milestone(line: &str) -> Option<(&'static str, String)> {
    if line.contains("Run: Starting shadps4 emulator") {
        return Some(("start", "Emulator: started".into()));
    }
    if let Some(n) = between(line, "WarmUp: Preloaded ", ' ') {
        return Some(("cache", format!("Shader cache: {n} pipelines loaded")));
    }
    if line.contains("Session with the headset made") {
        return Some(("xr", "OpenXR: session opened".into()));
    }
    if line.contains("No OpenXR runtime that could show frames") {
        return Some(("xr", "OpenXR: no headset runtime, the game is shown in the window".into()));
    }
    if line.contains("ProbeServer: probe: server ") && line.contains("unreachable") {
        return Some(("netfail", "Network: the server is unreachable, playing offline".into()));
    }
    if let Some(id) = between(line, "HandleLoginReply: Logged in npid='", '\'') {
        return Some(("login", format!("Network: {id} signed in")));
    }
    if line.contains("shadNet enabled but credentials missing") {
        return Some(("login", "Network: no shadNet account is set for the game".into()));
    }
    if line.contains("connection failed (state=") {
        return Some(("netfail", "Network: the server cannot be reached".into()));
    }
    if line.contains("authentication failed (state=") {
        return Some(("netfail", "Network: the server refused the sign-in".into()));
    }
    if line.contains("sceNpMatching2SearchRoom: called") {
        return Some(("search", "Network: looking for a lobby".into()));
    }
    if line.contains("Unhandled Exception") || line.contains("Assertion Failed") {
        return Some(("crash", "Emulator: fatal error (details in shad_log.txt)".into()));
    }
    None
}

/// Follows shad_log.txt from the start; restarts when the file is replaced or truncated.
pub struct LogTail {
    path: PathBuf,
    offset: u64,
    partial: Vec<u8>,
    seen: HashSet<&'static str>,
}

impl LogTail {
    pub fn new(path: PathBuf) -> Self {
        LogTail { path, offset: 0, partial: Vec::new(), seen: HashSet::new() }
    }

    pub fn poll(&mut self) -> Vec<Msg> {
        let mut out = Vec::new();
        let Ok(mut f) = std::fs::File::open(&self.path) else { return out };
        let len = f.metadata().map(|m| m.len()).unwrap_or(0);
        if len < self.offset {
            self.offset = 0;
            self.partial.clear();
        }
        if len == self.offset || f.seek(SeekFrom::Start(self.offset)).is_err() {
            return out;
        }
        let mut buf = Vec::new();
        let read = (&mut f).take((len - self.offset).min(64 << 20)).read_to_end(&mut buf).unwrap_or(0);
        self.offset += read as u64;
        self.partial.extend_from_slice(&buf);
        let Some(last_nl) = self.partial.iter().rposition(|&b| b == b'\n') else { return out };
        let complete: Vec<u8> = self.partial.drain(..=last_nl).collect();
        for raw in complete.split(|&b| b == b'\n') {
            let line = String::from_utf8_lossy(raw);
            if let Some((key, text)) = milestone(&line) {
                let repeatable = key == "cache";
                if repeatable || self.seen.insert(key) {
                    out.push(text);
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn milestones_from_real_lines() {
        let l = "[ShadNet] <Info> (ShadNet:Reader) client.cpp:720 HandleLoginReply: Logged in npid='Player_1' userId=1 friends=0";
        assert_eq!(milestone(l), Some(("login", Msg::raw("Network: Player_1 signed in"))));
        let l = "[Render] <Info> (shadPS4:Main) vk_pipeline_serialization.cpp:416 WarmUp: Preloaded 1548 pipelines";
        assert_eq!(milestone(l).unwrap().1.message, "Shader cache: 1548 pipelines loaded");
        let l = "[Loader] <Info> (shadPS4:Main) emulator.cpp:330 Run: Starting shadps4 emulator v0.16.1 WIP ";
        assert_eq!(milestone(l).unwrap().1.message, "Emulator: started");
        let l = "[ShadNet] <Warning> (shadPS4:Main) server_probe.cpp:109 ProbeServer: probe: server 127.0.0.1:31313 is unreachable (timeout 3000 ms)";
        assert_eq!(milestone(l).unwrap().1.message, "Network: the server is unreachable, playing offline");
        let l = "[ShadNet] <Info> (shadPS4:Main) server_probe.cpp:181 ProbeServer: probe: server 127.0.0.1:31313 is reachable (protocol v1)";
        assert!(milestone(l).is_none());
        let l = "[Core.Vr] <Info> (shadPS4:Main) openxr_host.cpp:2768 Connect: VirtualDesktopXR 1.1.0 has no headset connected yet. The game starts";
        let (key, m) = milestone(l).unwrap();
        assert_eq!((key, m.key, m.params["runtime"].as_str()), ("xr-wait", "log.event.headsetWaiting", Some("VirtualDesktopXR 1.1.0")));
        assert!(m.message.starts_with("OpenXR: VirtualDesktopXR 1.1.0 has no headset connected"));
        assert!(milestone("[Core] <Info> nothing to see").is_none());
    }

    #[test]
    fn warmup_round_log_and_script_length() {
        let log = "\
[Render] <Info> (shadPS4:Main) vk_pipeline_serialization.cpp:416 WarmUp: Preloaded 1548 pipelines
[ShadNet] <Info> (ShadNet:Reader) client.cpp:720 HandleLoginReply: Logged in npid='P' userId=1 friends=0
[Render.Vulkan] <Info> (shadPS4:Main) vk_pipeline_cache.cpp:471 GetGraphicsPipeline: Compiling graphics pipeline 0x1
[Render.Vulkan] <Info> (shadPS4:Main) vk_pipeline_cache.cpp:442 GetGraphicsPipeline: Compiling graphics pipeline 0x2 (off the draw)
[Lib.NpMatching2] <Info> (shadPS4:Main) np_matching2.cpp:1 sceNpMatching2SearchRoom: called
";
        let r = read_round(log);
        assert_eq!(r, RoundLog { new_pipelines: 2, preloaded: Some(1548), matched: true, signed_in: true, crashed: false });
        assert_eq!(read_round(""), RoundLog::default());
        let script = "# comment\n1 0.2 worn=1\n333.4 1.0 r2 padrot=0,-15,0\n334.6 0.3 up padrot=0,0,0\n";
        assert_eq!(script_seconds(script), Some(345));
        assert_eq!(script_seconds("# only comments\n"), None);
    }

    #[test]
    fn plan_and_command_line() {
        let p = LaunchPlan::new(Path::new("C:\\e\\shadps4.exe"), Path::new("C:\\d d\\eboot.bin"), Path::new("C:\\r"), vec![]);
        assert_eq!(p.command_line(), "\"C:\\e\\shadps4.exe\" -g \"C:\\d d\\eboot.bin\"");
    }

    #[test]
    fn tail_reads_complete_lines_once() {
        let dir = std::env::temp_dir().join(format!("fzh-tail-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("shad_log.txt");
        let mut f = std::fs::File::create(&path).unwrap();
        f.write_all(b"x Run: Starting shadps4 emulator v1\nx Session with the head").unwrap();
        f.flush().unwrap();
        let mut t = LogTail::new(path.clone());
        assert_eq!(t.poll(), vec![Msg::raw("Emulator: started")]);
        f.write_all(b"set made\nx Run: Starting shadps4 emulator v1\n").unwrap();
        f.flush().unwrap();
        assert_eq!(t.poll(), vec![Msg::raw("OpenXR: session opened")]);
        drop(f);
        std::fs::write(&path, b"y\n").unwrap();
        assert!(t.poll().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
