//! The account each server was left with, in `<data>\accounts.json`, so that going back to a
//! server signs in again without the recovery code. Only the Online ID and the email: the password
//! of each one is the Credential Manager's entry for that server and Online ID (`vault.rs`).
//! `users.json` still holds the one account in use; earlier launcher versions do not know this
//! file and leave it alone.

use crate::error::{BackendError, Res};
use crate::store;
use crate::vault::server_key;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::BTreeMap;
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Remembered {
    pub online_id: String,
    #[serde(default)]
    pub email: String,
}

/// `servers` is keyed by `vault::server_key`. Keys this version does not know are written back.
#[derive(Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Accounts {
    servers: BTreeMap<String, Remembered>,
    #[serde(flatten)]
    other: Map<String, Value>,
}

impl Accounts {
    pub fn get(&self, server: &str) -> Option<&Remembered> {
        self.servers.get(&server_key(server)).filter(|r| !r.online_id.is_empty())
    }

    pub fn remember(&mut self, server: &str, online_id: &str, email: &str) {
        self.servers.insert(server_key(server), Remembered { online_id: online_id.into(), email: email.into() });
    }

    pub fn forget(&mut self, server: &str) {
        self.servers.remove(&server_key(server));
    }

    /// The hosts of every remembered server, for the problem report to leave out.
    pub fn hosts(&self) -> Vec<String> {
        self.servers.keys().filter_map(|k| k.rsplit_once(':')).map(|(host, _)| host.to_string()).collect()
    }
}

/// A missing or unreadable file remembers nothing.
pub fn load(path: &Path) -> Accounts {
    std::fs::read(path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

pub fn save(path: &Path, accounts: &Accounts) -> Res<()> {
    let bytes = serde_json::to_vec_pretty(accounts).map_err(|e| BackendError::internal(e.to_string()))?;
    store::write_atomic(path, &bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn one_account_per_server_whatever_the_relay_or_case() {
        let mut a = Accounts::default();
        a.remember("Play.Example/relay.example", "Synthetic_1", "synthetic_1@users.invalid");
        a.remember("10.0.0.2", "Synthetic_1", "");
        a.remember("10.0.0.2:40001:40002:40003:40004:40005", "Other_2", "");
        assert_eq!(a.get("play.example").unwrap().online_id, "Synthetic_1");
        assert_eq!(a.get("10.0.0.2").unwrap().online_id, "Synthetic_1");
        assert_eq!(a.get("10.0.0.2:40001:40002:40003:40004:40005").unwrap().online_id, "Other_2");
        assert!(a.get("10.0.0.3").is_none());
        a.forget("PLAY.example");
        assert!(a.get("play.example").is_none());
        assert_eq!(a.hosts(), vec!["10.0.0.2", "10.0.0.2"]);
    }

    #[test]
    fn file_shape_and_unknown_keys() {
        let stored = json!({"servers": {"play.example:31313": {"onlineId": "Synthetic_1"}, "empty.example:31313": {"onlineId": ""}}, "later": {"a": 1}});
        let mut a: Accounts = serde_json::from_value(stored).unwrap();
        assert_eq!(a.get("play.example"), Some(&Remembered { online_id: "Synthetic_1".into(), email: String::new() }));
        assert!(a.get("empty.example").is_none());
        a.forget("empty.example/relay.example");
        let back = serde_json::to_value(&a).unwrap();
        assert_eq!(back, json!({"servers": {"play.example:31313": {"onlineId": "Synthetic_1", "email": ""}}, "later": {"a": 1}}));
        assert_eq!(serde_json::from_value::<Accounts>(json!({"servers": 3})).ok(), None);
    }

    #[test]
    fn a_missing_or_broken_file_remembers_nothing() {
        let dir = std::env::temp_dir().join(format!("fzh-accounts-{}", std::process::id()));
        let path = dir.join("accounts.json");
        assert_eq!(load(&path), Accounts::default());
        let mut a = Accounts::default();
        a.remember("play.example", "Synthetic_1", "");
        save(&path, &a).unwrap();
        assert_eq!(load(&path), a);
        std::fs::write(&path, b"{ not json").unwrap();
        assert_eq!(load(&path), Accounts::default());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
