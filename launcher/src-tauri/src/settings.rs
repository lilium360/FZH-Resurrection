use crate::error::{BackendError, Msg, Res};
use crate::instance::Server;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

pub const LOCAL_SERVER: &str = "127.0.0.1";

/// The server used when "private server" is off, fixed at build time by `FZH_DEFAULT_SERVER`.
pub fn default_server() -> &'static str {
    match option_env!("FZH_DEFAULT_SERVER") {
        Some(s) if Server::parse(s).is_some() => s,
        _ => LOCAL_SERVER,
    }
}

/// "official server" for the build-time default, else "private server <address>".
pub fn server_name(host: &str) -> Msg {
    if host == default_server() {
        Msg::new("server.official", "official server")
    } else {
        Msg::new("server.private", format!("private server {host}")).with("address", host)
    }
}

/// The launcher language on the first start: Italian when Windows shows Italian, else English.
pub fn system_language() -> &'static str {
    #[cfg(windows)]
    {
        let id = unsafe { windows_sys::Win32::Globalization::GetUserDefaultUILanguage() };
        if id & 0x3ff == 0x10 {
            return "it";
        }
    }
    "en"
}

pub const WARMUP_PLAYERS: u32 = 1;
pub const WARMUP_COUNTDOWN: u32 = 8;

const LAUNCHER_LANGUAGES: &[&str] = &["en", "it"];
const GAME_LANGUAGES: &[&str] =
    &["default", "it-IT", "en-US", "en-GB", "fr-FR", "de-DE", "es-ES", "pt-PT", "ru-RU", "pl-PL"];
const CONTROLLERS: &[&str] = &["aim", "pad"];
const FRAME_RATES: &[&str] = &["60", "90", "120"];
const PIXEL_DENSITIES: &[&str] = &["default", "0.5", "0.7", "0.85", "1.2", "1.4", "1.6", "1.8", "2.0"];
const SHADOW_QUALITIES: &[&str] = &["default", "0"];
const ANISOTROPIC_FILTERS: &[&str] = &["default", "4", "8", "16"];
// (No FXAA: measured on 2026-11-21, it leaves the edges as they are without it.)
const ANTIALIASING_MODES: &[&str] = &["default", "none", "taa", "msaa"];

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    pub launcher_language: String,
    pub microphone: String,
    pub audio_output: String,
    pub game_language: String,
    pub controller: String,
    pub grip_right: f64,
    pub grip_up: f64,
    pub grip_forward: f64,
    /// Degrees, in the controller's own frame: muzzle up, muzzle to the right, clockwise seen from behind.
    pub grip_pitch: f64,
    pub grip_yaw: f64,
    pub grip_roll: f64,
    pub frame_rate: String,
    pub async_shaders: bool,
    pub pixel_density: String,
    pub sharpness: f64,
    pub field_of_view: f64,
    pub shadow_quality: String,
    pub anisotropic_filtering: String,
    pub antialiasing: String,
    /// `contatore_fps`: `--show-fps` on the emulator command line.
    pub fps_counter: bool,
    /// `diagnostica_fotogrammi`: `SHADPS4_FRAME_STATS=1`.
    pub frame_stats: bool,
    /// Mod: the optional game patch that removes the "face the camera" warning.
    pub mod_free_turn: bool,
    pub use_private_server: bool,
    pub private_server: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            launcher_language: system_language().into(),
            microphone: "default".into(),
            audio_output: "default".into(),
            game_language: "default".into(),
            controller: "aim".into(),
            grip_right: 0.0,
            grip_up: 0.07,
            grip_forward: 0.3,
            grip_pitch: 0.0,
            grip_yaw: 0.0,
            grip_roll: 0.0,
            frame_rate: "90".into(),
            async_shaders: true,
            pixel_density: "default".into(),
            sharpness: 0.0,
            field_of_view: 100.0,
            shadow_quality: "default".into(),
            anisotropic_filtering: "default".into(),
            antialiasing: "default".into(),
            fps_counter: false,
            frame_stats: false,
            mod_free_turn: false,
            use_private_server: false,
            private_server: String::new(),
        }
    }
}

/// `setting` is the key of the setting's label, e.g. "meta.frameRate.label".
fn bad_value(setting: &'static str, english: String) -> BackendError {
    BackendError::invalid("backend.invalid_setting", english).with("setting", Msg::new(setting, ""))
}

fn choice(setting: &'static str, name: &str, v: &str, allowed: &[&str]) -> Res<String> {
    if allowed.contains(&v) {
        Ok(v.to_string())
    } else {
        Err(bad_value(setting, format!("{name}: allowed values are {}.", allowed.join(", "))))
    }
}

fn range(setting: &'static str, name: &str, v: f64, min: f64, max: f64) -> Res<f64> {
    if v.is_finite() && v >= min - 1e-9 && v <= max + 1e-9 {
        Ok(v.clamp(min, max))
    } else {
        Err(bad_value(setting, format!("{name}: must be between {min} and {max}.")))
    }
}

fn device(setting: &'static str, name: &str, v: &str, special: &[&str]) -> Res<String> {
    let v = v.trim();
    if special.contains(&v) || (!v.is_empty() && v.len() <= 256 && !v.contains(['\r', '\n', '\0'])) {
        Ok(v.to_string())
    } else {
        Err(bad_value(setting, format!("{name}: not a valid device.")))
    }
}

pub fn valid_host(s: &str) -> bool {
    !s.is_empty() && s.len() <= 253 && s.bytes().all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
}

impl Settings {
    /// Validates every field and returns the normalized settings.
    pub fn normalized(&self) -> Res<Settings> {
        let private_server = self.private_server.trim().to_string();
        if !private_server.is_empty() && Server::parse(&private_server).is_none() {
            return Err(BackendError::invalid(
                "backend.invalid_server_address",
                "Private server: expected an IP address or a host name (letters, digits, dots, - and _), alone or followed by five ports (host:game:matching:webapi:backend:relay), then optionally /relayhost[:port] or a list of up to 8 different relays separated by commas without spaces (/relayA[:port],relayB[:port]).",
            ));
        }
        if self.use_private_server && private_server.is_empty() {
            return Err(BackendError::invalid("toast.emptyServer", "Enter the address of the private server."));
        }
        let fov = range("meta.fieldOfView.label", "Field of view", self.field_of_view, 50.0, 100.0)?;
        if fov.fract() != 0.0 {
            return Err(bad_value("meta.fieldOfView.label", "Field of view: must be a whole number.".into()));
        }
        Ok(Settings {
            launcher_language: choice("meta.launcherLanguage.label", "Launcher language", &self.launcher_language, LAUNCHER_LANGUAGES)?,
            microphone: device("meta.microphone.label", "Microphone", &self.microphone, &["off", "default"])?,
            audio_output: device("meta.audioOutput.label", "Audio output", &self.audio_output, &["default"])?,
            game_language: choice("meta.gameLanguage.label", "Game language", &self.game_language, GAME_LANGUAGES)?,
            controller: choice("screen.gamepad.title", "Controller", &self.controller, CONTROLLERS)?,
            grip_right: range("meta.gripRight.label", "Grip offset to the right", self.grip_right, -0.5, 0.5)?,
            grip_up: range("meta.gripUp.label", "Grip offset up", self.grip_up, -0.5, 0.5)?,
            grip_forward: range("meta.gripForward.label", "Grip offset forward", self.grip_forward, -0.2, 0.8)?,
            grip_pitch: range("meta.gripPitch.label", "Grip tilt up", self.grip_pitch, -90.0, 90.0)?,
            grip_yaw: range("meta.gripYaw.label", "Grip turn right", self.grip_yaw, -90.0, 90.0)?,
            grip_roll: range("meta.gripRoll.label", "Grip roll", self.grip_roll, -90.0, 90.0)?,
            frame_rate: choice("meta.frameRate.label", "Frame rate", &self.frame_rate, FRAME_RATES)?,
            async_shaders: self.async_shaders,
            pixel_density: choice("meta.pixelDensity.label", "Pixel density", &self.pixel_density, PIXEL_DENSITIES)?,
            sharpness: range("meta.sharpness.label", "Sharpness", self.sharpness, 0.0, 1.0)?,
            field_of_view: fov,
            shadow_quality: choice("meta.shadowQuality.label", "Shadow quality", &self.shadow_quality, SHADOW_QUALITIES)?,
            anisotropic_filtering: choice("meta.anisotropicFiltering.label", "Anisotropic filtering", &self.anisotropic_filtering, ANISOTROPIC_FILTERS)?,
            antialiasing: choice("meta.antialiasing.label", "Antialiasing", &self.antialiasing, ANTIALIASING_MODES)?,
            fps_counter: self.fps_counter,
            frame_stats: self.frame_stats,
            mod_free_turn: self.mod_free_turn,
            use_private_server: self.use_private_server,
            private_server,
        })
    }

    /// Reads stored settings field by field; a missing or invalid value falls back to its default.
    pub fn from_stored(v: &Value) -> Settings {
        let defaults = Settings::default();
        let Some(stored) = v.as_object() else { return defaults };
        let default_map = match serde_json::to_value(&defaults) {
            Ok(Value::Object(m)) => m,
            _ => return defaults,
        };
        let mut whole = default_map.clone();
        whole.extend(stored.iter().filter(|(k, _)| default_map.contains_key(*k)).map(|(k, v)| (k.clone(), v.clone())));
        if let Some(ok) = serde_json::from_value::<Settings>(Value::Object(whole)).ok().and_then(|s| s.normalized().ok()) {
            return ok;
        }
        let mut result = defaults.clone();
        let mut keys: Vec<&String> = default_map.keys().collect();
        keys.sort_by_key(|k| k.as_str() != "privateServer");
        for key in keys {
            let Some(value) = stored.get(key) else { continue };
            let mut candidate: Map<String, Value> = match serde_json::to_value(&result) {
                Ok(Value::Object(m)) => m,
                _ => continue,
            };
            candidate.insert(key.clone(), value.clone());
            let Ok(parsed) = serde_json::from_value::<Settings>(Value::Object(candidate)) else { continue };
            if let Ok(ok) = parsed.normalized() {
                result = ok;
            }
        }
        result
    }

    pub fn server(&self) -> String {
        if self.use_private_server && !self.private_server.is_empty() {
            self.private_server.clone()
        } else {
            default_server().to_string()
        }
    }

    /// What the player sees for the configured server: the default address is never shown.
    pub fn server_name(&self) -> Msg {
        server_name(&self.server())
    }

    /// The settings of a warm-up round: gamepad, no microphone.
    pub fn for_warmup(&self) -> Settings {
        Settings { controller: "pad".into(), microphone: "off".into(), fps_counter: false, frame_stats: false, ..self.clone() }
    }

    /// The emulator environment of a warm-up round: no headset (`SHADPS4_OPENXR=0`, the game shows
    /// in its window) and the input script that plays it.
    pub fn warmup_environment(&self, script: &str) -> Vec<(&'static str, String)> {
        let mut env = self.for_warmup().environment();
        for (k, v) in env.iter_mut() {
            match *k {
                "SHADPS4_OPENXR" => *v = "0".into(),
                "SHADPS4_INPUT_SCRIPT" => *v = script.into(),
                "SHADPS4_VR_DESKTOP_VIEW" | "SHADPS4_VR_DESKTOP_CROP" => v.clear(),
                _ => {}
            }
        }
        env
    }

    /// `pak_inputs()` with the warm-up lobby (1 player, 8 s countdown).
    pub fn warmup_pak_inputs(&self) -> Vec<String> {
        let mut v = vec![WARMUP_PLAYERS.to_string(), WARMUP_COUNTDOWN.to_string()];
        v.extend(self.pak_inputs());
        v
    }

    /// Emulator arguments after `-g <eboot>`: `--show-fps` for the FPS counter.
    pub fn extra_args(&self) -> Vec<&'static str> {
        if self.fps_counter { vec!["--show-fps"] } else { Vec::new() }
    }

    /// Environment of the emulator: a fixed part plus the settings.
    /// An empty value means the variable is removed.
    pub fn environment(&self) -> Vec<(&'static str, String)> {
        let aim = self.controller == "aim";
        let mut env: Vec<(&'static str, String)> = vec![
            ("SHADPS4_VR", "1".into()),
            ("SHADPS4_OPENXR", "1".into()),
            ("SHADPS4_XR_WAIT", "0".into()),
            ("SHADPS4_XR_PAUSE", "0".into()),
            ("SHADPS4_XR_GAMEPAD_LAYOUT", "1".into()),
            ("SHADPS4_XR_PAD_HAND", "right".into()),
            ("SHADPS4_VR_PACE", "1".into()),
            ("SHADPS4_NATIVE_CLIP_DISTANCE", "1".into()),
            ("SHADPS4_TITLE_RESOLUTION", "title".into()),
            ("SHADPS4_TITLE_EYE_WIDTH", String::new()),
            ("SHADPS4_INPUT_SCRIPT", String::new()),
            ("SHADPS4_LIVE_INPUT", String::new()),
            // The window on the monitor shows one eye filling it, as a TV next to a PS4 does;
            // the headset has a frame of its own and is not affected.
            ("SHADPS4_VR_DESKTOP_VIEW", "spectator".into()),
            ("SHADPS4_VR_DESKTOP_CROP", "1".into()),
        ];
        env.push((
            "SHADPS4_XR_PAD_OFFSET",
            if aim {
                format!("{},{},{}", fmt_g(self.grip_right), fmt_g(self.grip_up), fmt_g(-self.grip_forward))
            } else {
                "0,0,0".into()
            },
        ));
        env.push((
            "SHADPS4_XR_PAD_ROTATION",
            if aim {
                format!("{},{},{}", fmt_g(self.grip_pitch), fmt_g(self.grip_yaw), fmt_g(self.grip_roll))
            } else {
                "0,0,0".into()
            },
        ));
        env.push(("SHADPS4_ASYNC_PIPELINES", if self.async_shaders { "1" } else { "0" }.into()));
        env.push((
            "SHADPS4_CONSOLE_LANGUAGE",
            if self.game_language == "default" { String::new() } else { self.game_language.clone() },
        ));
        env.push(("SHADPS4_FRAME_STATS", if self.frame_stats { "1" } else { "" }.into()));
        let at = Server::of(&self.server());
        let udp = if at.matching == crate::instance::SHADNET_PORT + 1 { String::new() } else { at.matching.to_string() };
        env.push(("SHADPS4_SHADNET_UDP_PORT", udp));
        env.push(("SHADPS4_P2P_RELAY", at.relay_override().unwrap_or_default()));
        env.push(("SHADPS4_VR_SHARPEN", fmt_g(self.sharpness)));
        env.push((
            "SHADPS4_VR_FOV",
            if self.field_of_view == 100.0 { String::new() } else { format!("{}", self.field_of_view as i64) },
        ));
        env.push((
            "SHADPS4_VR_REFRESH_RATE",
            if self.frame_rate == "60" { String::new() } else { self.frame_rate.clone() },
        ));
        env
    }

    /// Engine settings that go through the game's ini files.
    pub fn engine_entries(&self) -> Vec<String> {
        let mut v = Vec::new();
        if self.pixel_density != "default" {
            v.push(format!("vr.PixelDensity={}", self.pixel_density));
        }
        if self.shadow_quality != "default" {
            v.push(format!("r.ShadowQuality={}", self.shadow_quality));
        }
        if self.frame_rate != "60" {
            v.push(format!(
                "[/Script/MorpheusEditor.MorpheusRuntimeSettings]PSVRFrameSequence={}",
                if self.frame_rate == "90" { "Render90Scanout90" } else { "Render120Scanout120" }
            ));
            v.push(
                "[/Script/Engine.Engine]SmoothedFrameRateRange=(LowerBound=(Type=Inclusive,Value=22.000000),\
                 UpperBound=(Type=Exclusive,Value=240.000000))"
                    .into(),
            );
            v.push("r.NumBufferedOcclusionQueries=2".into());
        }
        if self.anisotropic_filtering != "default" {
            v.push(format!("r.MaxAnisotropy={}", self.anisotropic_filtering));
        }
        if let Some(i) = ["none", "fxaa", "taa", "msaa"].iter().position(|m| *m == self.antialiasing) {
            v.push(format!("r.DefaultFeature.AntiAliasing={i}"));
            // The game's own quality level leaves post-process antialiasing off (as on a base
            // PS4), which silences FXAA and TAA: this turns it on for them. MSAA needs none.
            if let Some(quality) = [Some(0), Some(4), Some(3), None][i] {
                v.push(format!("r.PostProcessAAQuality={quality}"));
            }
        }
        v
    }

    /// The pak inputs; the pak is rebuilt when this changes. The lobby keeps the game's own rules.
    pub fn pak_inputs(&self) -> Vec<String> {
        let mut v = self.engine_entries();
        let address = self.server();
        let server = Server::services(&address);
        if server != LOCAL_SERVER {
            v.push(format!("--server={server}"));
        }
        v
    }

    pub fn mic_device(&self) -> String {
        match self.microphone.as_str() {
            "off" => "None".into(),
            "default" => "Default Device".into(),
            other => other.into(),
        }
    }

    pub fn output_device(&self) -> String {
        if self.audio_output == "default" { "Default Device".into() } else { self.audio_output.clone() }
    }
}

/// Python's "%g": six significant digits, trailing zeros removed, exponent outside 1e-4..1e6.
pub fn fmt_g(v: f64) -> String {
    if v == 0.0 {
        return if v.is_sign_negative() { "-0".into() } else { "0".into() };
    }
    if !v.is_finite() {
        return if v.is_nan() { "nan".into() } else if v > 0.0 { "inf".into() } else { "-inf".into() };
    }
    let sci = format!("{v:.5e}");
    let (mantissa, exp) = sci.split_once('e').unwrap_or((&sci, "0"));
    let exp: i32 = exp.parse().unwrap_or(0);
    let strip = |s: &str| -> String {
        if s.contains('.') { s.trim_end_matches('0').trim_end_matches('.').to_string() } else { s.to_string() }
    };
    if exp < -4 || exp >= 6 {
        let sign = if exp < 0 { '-' } else { '+' };
        format!("{}e{}{:02}", strip(mantissa), sign, exp.abs())
    } else {
        let decimals = (5 - exp).max(0) as usize;
        strip(&format!("{v:.decimals$}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn g_format_matches_python() {
        assert_eq!(fmt_g(0.0), "0");
        assert_eq!(fmt_g(-0.0), "-0");
        assert_eq!(fmt_g(0.07), "0.07");
        assert_eq!(fmt_g(-0.3), "-0.3");
        assert_eq!(fmt_g(0.30000000000000004), "0.3");
        assert_eq!(fmt_g(0.5), "0.5");
        assert_eq!(fmt_g(1.0), "1");
        assert_eq!(fmt_g(0.05), "0.05");
        assert_eq!(fmt_g(0.123456789), "0.123457");
        assert_eq!(fmt_g(0.00001), "1e-05");
        assert_eq!(fmt_g(1234567.0), "1.23457e+06");
        assert_eq!(fmt_g(-0.8), "-0.8");
    }

    fn env_map(s: &Settings) -> std::collections::HashMap<&'static str, String> {
        s.environment().into_iter().collect()
    }

    #[test]
    fn default_environment() {
        let e = env_map(&Settings::default());
        assert_eq!(e["SHADPS4_XR_PAD_OFFSET"], "0,0.07,-0.3");
        assert_eq!(e["SHADPS4_XR_PAD_ROTATION"], "0,0,0");
        assert_eq!(e["SHADPS4_ASYNC_PIPELINES"], "1");
        assert_eq!(e["SHADPS4_CONSOLE_LANGUAGE"], "");
        assert_eq!(e["SHADPS4_VR_SHARPEN"], "0");
        assert_eq!(e["SHADPS4_VR_FOV"], "");
        assert_eq!(e["SHADPS4_VR_REFRESH_RATE"], "90");
        assert_eq!(e["SHADPS4_OPENXR"], "1");
        assert_eq!(e["SHADPS4_FRAME_STATS"], "");
        assert!(Settings::default().extra_args().is_empty());
    }

    #[test]
    fn diagnostics_match_fzh() {
        let s = Settings { fps_counter: true, frame_stats: true, ..Settings::default() };
        assert_eq!(s.extra_args(), vec!["--show-fps"]);
        assert_eq!(env_map(&s)["SHADPS4_FRAME_STATS"], "1");
        let stored = Settings::from_stored(&json!({"fpsCounter": true}));
        assert!(stored.fps_counter && !stored.frame_stats);
        let parsed: Settings = serde_json::from_value(json!({"frameRate": "60"})).unwrap();
        assert!(!parsed.fps_counter && !parsed.frame_stats);
        let w = s.for_warmup();
        assert!(w.extra_args().is_empty());
        let e: std::collections::HashMap<_, _> = s.warmup_environment("C:\\w.txt").into_iter().collect();
        assert_eq!(e["SHADPS4_FRAME_STATS"], "");
        assert_eq!((e["SHADPS4_SHADNET_UDP_PORT"].as_str(), e["SHADPS4_P2P_RELAY"].as_str()), ("", ""));
        let hosted = Settings { use_private_server: true, private_server: "box.example:40001:40002:40003:40004:40005".into(), ..Settings::default() };
        let h = env_map(&hosted.normalized().unwrap());
        assert_eq!((h["SHADPS4_SHADNET_UDP_PORT"].as_str(), h["SHADPS4_P2P_RELAY"].as_str()), ("40002", "box.example:40005"));
        let apart = Settings { private_server: "play.example/relay.example:9000".into(), ..hosted.clone() };
        let a = env_map(&apart.normalized().unwrap());
        assert_eq!((a["SHADPS4_SHADNET_UDP_PORT"].as_str(), a["SHADPS4_P2P_RELAY"].as_str()), ("", "relay.example:9000"));
        let several = Settings { private_server: "play.example/eu.example,us.example:9000".into(), ..hosted.clone() };
        let r = env_map(&several.normalized().unwrap());
        assert_eq!(r["SHADPS4_P2P_RELAY"], "eu.example:31316,us.example:9000");
        assert_eq!(several.pak_inputs().last().unwrap(), "--server=play.example");
        for bad in ["play.example/eu.example,", "play.example/eu.example, us.example", "play.example/eu.example,eu.example"] {
            assert!(Settings { private_server: bad.into(), ..hosted.clone() }.normalized().is_err(), "{bad}");
        }
        assert_eq!(apart.pak_inputs().last().unwrap(), "--server=play.example");
        assert_eq!(e["SHADPS4_OPENXR"], "0");
        assert_eq!(e["SHADPS4_VR_DESKTOP_VIEW"], "");
        assert_eq!(env_map(&s)["SHADPS4_VR_DESKTOP_VIEW"], "spectator");
        assert_eq!(e["SHADPS4_INPUT_SCRIPT"], "C:\\w.txt");
        assert_eq!(e["SHADPS4_XR_PAD_OFFSET"], "0,0,0");
        assert_eq!(&s.warmup_pak_inputs()[..2], &["1".to_string(), "8".to_string()]);
        assert_eq!(&s.warmup_pak_inputs()[2..], &s.pak_inputs()[..]);
        assert_eq!(w.mic_device(), "None");
    }

    /// Reference values recorded from the earlier script launcher for the same settings (aim, it-IT,
    /// sharpness 0.5, 90 Hz, FPS counter and frame diagnostics on).
    #[test]
    fn matches_the_reference_environment() {
        let s = Settings {
            game_language: "it-IT".into(),
            sharpness: 0.5,
            fps_counter: true,
            frame_stats: true,
            ..Settings::default()
        };
        let e = env_map(&s);
        for (k, v) in [
            ("SHADPS4_XR_PAD_OFFSET", "0,0.07,-0.3"),
            ("SHADPS4_ASYNC_PIPELINES", "1"),
            ("SHADPS4_CONSOLE_LANGUAGE", "it-IT"),
            ("SHADPS4_FRAME_STATS", "1"),
            ("SHADPS4_VR_SHARPEN", "0.5"),
            ("SHADPS4_VR_FOV", ""),
            ("SHADPS4_VR_REFRESH_RATE", "90"),
        ] {
            assert_eq!(e[k], v, "{k}");
        }
        assert_eq!(s.extra_args(), vec!["--show-fps"]);
    }

    #[test]
    fn server_names_hide_the_default_address() {
        let official = server_name(default_server());
        assert_eq!((official.key, official.message.as_str()), ("server.official", "official server"));
        assert!(official.params.is_empty());
        let private = server_name("10.1.2.3");
        assert_eq!((private.key, private.params["address"].as_str()), ("server.private", Some("10.1.2.3")));
    }

    #[test]
    fn pad_and_custom_environment() {
        let s = Settings {
            controller: "pad".into(),
            game_language: "it-IT".into(),
            sharpness: 0.5,
            field_of_view: 80.0,
            frame_rate: "60".into(),
            async_shaders: false,
            ..Settings::default()
        };
        let e = env_map(&s);
        assert_eq!(e["SHADPS4_XR_PAD_OFFSET"], "0,0,0");
        assert_eq!(e["SHADPS4_CONSOLE_LANGUAGE"], "it-IT");
        assert_eq!(e["SHADPS4_VR_SHARPEN"], "0.5");
        assert_eq!(e["SHADPS4_VR_FOV"], "80");
        assert_eq!(e["SHADPS4_VR_REFRESH_RATE"], "");
        assert_eq!(e["SHADPS4_ASYNC_PIPELINES"], "0");
        let e = env_map(&Settings { grip_forward: 0.0, ..Settings::default() });
        assert_eq!(e["SHADPS4_XR_PAD_OFFSET"], "0,0.07,-0");
        let tilted = Settings { grip_pitch: 12.5, grip_yaw: -3.0, grip_roll: 1.0, ..Settings::default() };
        assert_eq!(env_map(&tilted)["SHADPS4_XR_PAD_ROTATION"], "12.5,-3,1");
        assert_eq!(env_map(&Settings { controller: "pad".into(), ..tilted })["SHADPS4_XR_PAD_ROTATION"], "0,0,0");
    }

    #[test]
    fn engine_entries_match_fzh() {
        let s = Settings {
            pixel_density: "2.0".into(),
            shadow_quality: "0".into(),
            frame_rate: "90".into(),
            anisotropic_filtering: "16".into(),
            antialiasing: "taa".into(),
            use_private_server: true,
            private_server: LOCAL_SERVER.into(),
            ..Settings::default()
        };
        assert_eq!(
            s.pak_inputs(),
            vec![
                "vr.PixelDensity=2.0",
                "r.ShadowQuality=0",
                "[/Script/MorpheusEditor.MorpheusRuntimeSettings]PSVRFrameSequence=Render90Scanout90",
                "[/Script/Engine.Engine]SmoothedFrameRateRange=(LowerBound=(Type=Inclusive,Value=22.000000),UpperBound=(Type=Exclusive,Value=240.000000))",
                "r.NumBufferedOcclusionQueries=2",
                "r.MaxAnisotropy=16",
                "r.DefaultFeature.AntiAliasing=2",
                "r.PostProcessAAQuality=3",
            ]
        );
        let s = Settings { frame_rate: "120".into(), antialiasing: "none".into(), ..Settings::default() };
        let e = s.engine_entries();
        assert!(e[0].ends_with("Render120Scanout120"));
        assert_eq!(&e[e.len() - 2..], &["r.DefaultFeature.AntiAliasing=0", "r.PostProcessAAQuality=0"]);
        let msaa = Settings { antialiasing: "msaa".into(), frame_rate: "60".into(), ..Settings::default() };
        assert_eq!(msaa.engine_entries(), vec!["r.DefaultFeature.AntiAliasing=3"]);
        assert!(Settings { antialiasing: "fxaa".into(), ..Settings::default() }.normalized().is_err());
        assert!(Settings { shadow_quality: "5".into(), ..Settings::default() }.normalized().is_err());
        assert!(Settings { frame_rate: "60".into(), ..Settings::default() }.engine_entries().is_empty());
    }

    #[test]
    fn server_selection_and_pak_inputs() {
        let s = Settings { use_private_server: true, private_server: "my-host.example".into(), ..Settings::default() };
        assert_eq!(s.server(), "my-host.example");
        assert_eq!(s.pak_inputs().last().unwrap(), "--server=my-host.example");
        let s = Settings { use_private_server: false, private_server: "10.0.0.2".into(), ..Settings::default() };
        assert_eq!(s.server(), default_server());
        assert_eq!(
            s.pak_inputs().iter().any(|v| v.starts_with("--server")),
            default_server() != LOCAL_SERVER
        );
    }

    #[test]
    fn validation() {
        let bad = Settings { private_server: "a b".into(), ..Settings::default() };
        assert_eq!(bad.normalized().unwrap_err().code, "invalid_input");
        let bad = Settings { use_private_server: true, ..Settings::default() };
        assert!(bad.normalized().is_err());
        let bad = Settings { grip_forward: 0.9, ..Settings::default() };
        assert!(bad.normalized().is_err());
        let bad = Settings { grip_pitch: 91.0, ..Settings::default() };
        assert!(bad.normalized().is_err());
        let bad = Settings { frame_rate: "75".into(), ..Settings::default() };
        assert!(bad.normalized().is_err());
        let ok = Settings { private_server: "  203.0.113.4 ".into(), ..Settings::default() }.normalized().unwrap();
        assert_eq!(ok.private_server, "203.0.113.4");
    }

    #[test]
    fn stored_values_fall_back_one_by_one() {
        let s = Settings::from_stored(&json!({
            "frameRate": "120",
            "pixelDensity": "9.9",
            "sharpness": 2,
            "gripUp": 0.1,
            "unknown": true
        }));
        assert_eq!(s.frame_rate, "120");
        assert_eq!(s.pixel_density, "default");
        assert_eq!(s.sharpness, 0.0);
        assert_eq!(s.grip_up, 0.1);
        assert_eq!(Settings::from_stored(&json!("garbage")), Settings::default());
    }

    #[test]
    fn stored_private_server_survives_loading() {
        let s = Settings::from_stored(&json!({"usePrivateServer": true, "privateServer": "127.0.0.1"}));
        assert!(s.use_private_server);
        assert_eq!(s.server(), "127.0.0.1");
        let s = Settings::from_stored(&json!({"usePrivateServer": true, "privateServer": "10.0.0.2", "frameRate": "75"}));
        assert!(s.use_private_server);
        assert_eq!((s.server().as_str(), s.frame_rate.as_str()), ("10.0.0.2", "90"));
    }

    #[test]
    fn audio_devices() {
        assert_eq!(Settings { microphone: "off".into(), ..Settings::default() }.mic_device(), "None");
        assert_eq!(Settings::default().mic_device(), "Default Device");
        assert_eq!(Settings::default().output_device(), "Default Device");
        assert_eq!(Settings { microphone: "Headset Mic".into(), ..Settings::default() }.mic_device(), "Headset Mic");
    }
}
