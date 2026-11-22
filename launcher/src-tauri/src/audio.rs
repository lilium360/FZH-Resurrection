//! Audio devices with the names SDL3 gives them on Windows (WASAPI friendly names, e.g.
//! "Speakers (USB Audio Device)"): the emulator picks `Audio.sdl_mic_device` and
//! `Audio.sdl_main_output_device` by exact name (`sdl_audio_out.cpp`, `sdl_audio_in.cpp`).
//! The names come from the MMDevices registry keys Windows keeps for each endpoint: the device
//! description plus the adapter name in brackets, active endpoints only.

use serde::Serialize;

#[derive(Serialize, Debug, Clone, PartialEq)]
pub struct AudioDevice {
    pub id: String,
    pub label: String,
}

#[derive(Serialize, Debug, Default)]
pub struct AudioDevices {
    pub inputs: Vec<AudioDevice>,
    pub outputs: Vec<AudioDevice>,
}

/// `DEVICE_STATE_ACTIVE`; the upper bits of `DeviceState` are flags.
const ACTIVE: u32 = 1;

pub fn friendly_name(description: &str, adapter: &str) -> String {
    if adapter.is_empty() { description.to_string() } else { format!("{description} ({adapter})") }
}

pub fn list() -> AudioDevices {
    AudioDevices { inputs: endpoints("Capture"), outputs: endpoints("Render") }
}

#[cfg(windows)]
fn endpoints(kind: &str) -> Vec<AudioDevice> {
    use winreg::enums::{HKEY_LOCAL_MACHINE, KEY_READ};
    use winreg::RegKey;
    const DESCRIPTION: &str = "{a45c254e-df1c-4efd-8020-67d146a850e0},2";
    const ADAPTER: &str = "{b3f8fa53-0004-438e-9003-51a46e139bfc},6";
    let path = format!(r"SOFTWARE\Microsoft\Windows\CurrentVersion\MMDevices\Audio\{kind}");
    let Ok(root) = RegKey::predef(HKEY_LOCAL_MACHINE).open_subkey_with_flags(path, KEY_READ) else { return Vec::new() };
    let mut out: Vec<AudioDevice> = root
        .enum_keys()
        .flatten()
        .filter_map(|id| {
            let key = root.open_subkey_with_flags(&id, KEY_READ).ok()?;
            let state: u32 = key.get_value("DeviceState").ok()?;
            if state & 0xF != ACTIVE {
                return None;
            }
            let props = key.open_subkey_with_flags("Properties", KEY_READ).ok()?;
            let description: String = props.get_value(DESCRIPTION).ok()?;
            let adapter: String = props.get_value(ADAPTER).unwrap_or_default();
            let name = friendly_name(description.trim(), adapter.trim());
            (!name.is_empty()).then(|| AudioDevice { id: name.clone(), label: name })
        })
        .collect();
    out.sort_by(|a, b| a.label.to_lowercase().cmp(&b.label.to_lowercase()));
    out.dedup_by(|a, b| a.id == b.id);
    out
}

#[cfg(not(windows))]
fn endpoints(_kind: &str) -> Vec<AudioDevice> {
    Vec::new()
}

#[cfg(test)]
mod tests {
    #[test]
    fn names_as_sdl_shows_them() {
        assert_eq!(super::friendly_name("Speakers", "USB Audio Device"), "Speakers (USB Audio Device)");
        assert_eq!(super::friendly_name("Speakers", ""), "Speakers");
    }
}
