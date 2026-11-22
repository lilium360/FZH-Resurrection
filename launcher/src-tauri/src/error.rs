use serde::Serialize;
use serde_json::{json, Map, Value};
use std::fmt;

/// A text for the player: an i18n key of `copy/en.json` with named parameters, translated by the
/// front end, and the English text for the launcher's own log file and the problem report.
/// An empty key means the text is shown as it is (emulator output, names, paths).
#[derive(Debug, Clone, Serialize, PartialEq, Default)]
pub struct Msg {
    pub key: &'static str,
    #[serde(skip_serializing_if = "Map::is_empty")]
    pub params: Map<String, Value>,
    pub message: String,
}

impl Msg {
    pub fn new(key: &'static str, message: impl Into<String>) -> Self {
        Self { key, params: Map::new(), message: message.into() }
    }
    pub fn raw(text: impl Into<String>) -> Self {
        Self::new("", text)
    }
    pub fn with(mut self, name: &str, value: impl Into<Value>) -> Self {
        self.params.insert(name.to_string(), value.into());
        self
    }
}

impl From<String> for Msg {
    fn from(text: String) -> Msg {
        Msg::raw(text)
    }
}

impl From<&str> for Msg {
    fn from(text: &str) -> Msg {
        Msg::raw(text)
    }
}

impl From<Msg> for Value {
    fn from(m: Msg) -> Value {
        json!({ "key": m.key, "params": m.params, "message": m.message })
    }
}

/// A size parameter, formatted by the front end in the player's language.
pub fn bytes(n: u64) -> Value {
    json!({ "bytes": n })
}

/// A duration parameter, formatted by the front end.
pub fn seconds(s: u64) -> Value {
    json!({ "seconds": s })
}

/// Parameters joined with ", " by the front end.
pub fn list(items: Vec<Msg>) -> Value {
    json!({ "list": items.into_iter().map(Value::from).collect::<Vec<_>>() })
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct BackendError {
    pub code: &'static str,
    pub key: &'static str,
    #[serde(skip_serializing_if = "Map::is_empty")]
    pub params: Map<String, Value>,
    pub message: String,
}

pub type Res<T> = Result<T, BackendError>;

impl BackendError {
    pub fn new(code: &'static str, key: &'static str, message: impl Into<String>) -> Self {
        Self { code, key, params: Map::new(), message: message.into() }
    }
    pub fn from_msg(code: &'static str, m: Msg) -> Self {
        Self { code, key: m.key, params: m.params, message: m.message }
    }
    pub fn with(mut self, name: &str, value: impl Into<Value>) -> Self {
        self.params.insert(name.to_string(), value.into());
        self
    }
    pub fn msg(&self) -> Msg {
        Msg { key: self.key, params: self.params.clone(), message: self.message.clone() }
    }
    pub fn invalid(key: &'static str, message: impl Into<String>) -> Self {
        Self::new("invalid_input", key, message)
    }
    /// Something the player cannot fix; `detail` is technical and stays untranslated.
    pub fn internal(detail: impl Into<String>) -> Self {
        let detail = detail.into();
        Self::new("internal", "backend.internal", format!("Something went wrong in the launcher: {detail}")).with("detail", detail)
    }
    pub fn unsupported(key: &'static str, message: impl Into<String>) -> Self {
        Self::new("unsupported", key, message)
    }
    /// A file operation on `path` failed; `e` is the system's own text.
    pub fn io_path(path: impl fmt::Display, e: impl fmt::Display) -> Self {
        let (path, detail) = (path.to_string(), e.to_string());
        Self::new("io", "backend.io_path", format!("Cannot use {path}: {detail}")).with("path", path).with("detail", detail)
    }
}

impl fmt::Display for BackendError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} ({})", self.message, self.code)
    }
}

impl From<BackendError> for Value {
    fn from(e: BackendError) -> Value {
        e.msg().into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn errors_serialize_code_key_params_and_english_text() {
        let e = BackendError::new("io", "backend.install_no_space", "Not enough space").with("free", bytes(5)).with("drive", "D:");
        let v = serde_json::to_value(&e).unwrap();
        assert_eq!(v["code"], "io");
        assert_eq!(v["key"], "backend.install_no_space");
        assert_eq!(v["params"]["free"]["bytes"], 5);
        assert_eq!(v["params"]["drive"], "D:");
        assert_eq!(v["message"], "Not enough space");
        let plain = serde_json::to_value(BackendError::new("busy", "backend.busy_preparing", "Busy")).unwrap();
        assert!(plain.get("params").is_none());
    }

    #[test]
    fn nested_messages_keep_their_key() {
        let inner = Msg::new("server.official", "the official server");
        let m = Msg::new("log.event.playingOn", "Playing on the official server").with("server", inner);
        let v = serde_json::to_value(&m).unwrap();
        assert_eq!(v["params"]["server"]["key"], "server.official");
    }
}
