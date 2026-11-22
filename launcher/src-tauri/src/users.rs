//! The shadNet account in the emulator instance's `users.json` (docs/launcher-contract.md,
//! "Account storage in the emulator"). The emulator reads the credentials of the user with `player_index` 1 from there,
//! password in plain text; the launcher keeps them nowhere else.

use serde_json::{json, Map, Value};

#[derive(Debug, Clone, PartialEq)]
pub struct Credentials {
    pub npid: String,
    pub password: String,
    pub email: String,
}

/// The four users the emulator creates when the file is missing (`CreateDefaultUsers`).
fn default_users() -> Vec<Value> {
    (0..4)
        .map(|i| {
            json!({
                "user_id": 1000 + i,
                "user_name": if i == 0 { "shadPS4".to_string() } else { format!("shadPS4-{}", i + 1) },
                "user_color": i + 1,
                "player_index": i + 1,
            })
        })
        .collect()
}

fn users_mut(root: &mut Map<String, Value>) -> &mut Vec<Value> {
    let users = root.entry("Users").or_insert_with(|| json!({}));
    if !users.is_object() {
        *users = json!({});
    }
    let list = users.as_object_mut().unwrap().entry("user").or_insert_with(|| Value::Array(default_users()));
    if !list.as_array().is_some_and(|a| a.iter().any(Value::is_object)) {
        *list = Value::Array(default_users());
    }
    list.as_array_mut().unwrap()
}

fn first_player_index(list: &[Value]) -> Option<usize> {
    list.iter()
        .position(|u| u.get("player_index").and_then(Value::as_i64) == Some(1))
        .or_else(|| list.iter().position(|u| u.get("user_id").and_then(Value::as_i64) == Some(1000)))
        .or_else(|| list.iter().position(Value::is_object))
}

fn root_of(v: Value) -> Map<String, Value> {
    match v {
        Value::Object(m) => m,
        _ => Map::new(),
    }
}

/// Sets the shadNet fields of the first player and keeps every other key and user.
pub fn set_account(existing: Value, c: &Credentials) -> Value {
    let mut root = root_of(existing);
    let list = users_mut(&mut root);
    let i = first_player_index(list).unwrap_or(0);
    let user = list[i].as_object_mut().unwrap();
    user.insert("player_index".into(), json!(1));
    user.insert("shadnet_npid".into(), json!(c.npid));
    user.insert("shadnet_password".into(), json!(c.password));
    user.insert("shadnet_email".into(), json!(c.email));
    user.insert("shadnet_token".into(), json!(""));
    user.insert("shadnet_enabled".into(), json!(true));
    Value::Object(root)
}

/// Sign-out: the first player keeps its local profile but no longer has a shadNet account.
pub fn clear_account(existing: Value) -> Value {
    let mut root = root_of(existing);
    let has_users = root.get("Users").and_then(|u| u.get("user")).and_then(Value::as_array).is_some();
    if !has_users {
        return Value::Object(root);
    }
    let list = users_mut(&mut root);
    if let Some(i) = first_player_index(list) {
        let user = list[i].as_object_mut().unwrap();
        for k in ["shadnet_npid", "shadnet_password", "shadnet_email", "shadnet_token"] {
            user.insert(k.into(), json!(""));
        }
        user.insert("shadnet_enabled".into(), json!(false));
    }
    Value::Object(root)
}

pub fn account(v: &Value) -> Option<Credentials> {
    let list = v.get("Users")?.get("user")?.as_array()?;
    let user = list.get(first_player_index(list)?)?;
    let s = |k: &str| user.get(k).and_then(Value::as_str).unwrap_or("").to_string();
    let c = Credentials { npid: s("shadnet_npid"), password: s("shadnet_password"), email: s("shadnet_email") };
    let enabled = user.get("shadnet_enabled").and_then(Value::as_bool).unwrap_or(false);
    (enabled && !c.npid.is_empty() && !c.password.is_empty()).then_some(c)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn creds() -> Credentials {
        Credentials { npid: "Synthetic_1".into(), password: "not-a-real-pw".into(), email: "Synthetic_1@fzh.invalid".into() }
    }

    #[test]
    fn new_file_gets_the_emulator_defaults() {
        let v = set_account(Value::Null, &creds());
        let list = v["Users"]["user"].as_array().unwrap();
        assert_eq!(list.len(), 4);
        assert_eq!(list[0]["user_id"], 1000);
        assert_eq!(list[0]["user_name"], "shadPS4");
        assert_eq!(list[0]["shadnet_npid"], "Synthetic_1");
        assert_eq!(list[0]["shadnet_enabled"], true);
        assert_eq!(list[3]["user_name"], "shadPS4-4");
        assert!(list[1].get("shadnet_npid").is_none());
        assert_eq!(account(&v), Some(creds()));
    }

    #[test]
    fn existing_file_keeps_other_keys_and_users() {
        let before = json!({
            "Other": {"x": 1},
            "Users": {
                "commit_hash": "abc",
                "user": [
                    {"user_id": 1001, "user_name": "Second", "player_index": 2, "user_color": 2},
                    {"user_id": 1000, "user_name": "Me", "player_index": 1, "np_age": 30, "shadnet_npid": "Old"}
                ]
            }
        });
        let v = set_account(before, &creds());
        assert_eq!(v["Other"]["x"], 1);
        assert_eq!(v["Users"]["commit_hash"], "abc");
        let list = v["Users"]["user"].as_array().unwrap();
        assert_eq!(list.len(), 2);
        assert_eq!(list[0]["user_name"], "Second");
        assert!(list[0].get("shadnet_npid").is_none());
        assert_eq!(list[1]["user_name"], "Me");
        assert_eq!(list[1]["np_age"], 30);
        assert_eq!(list[1]["shadnet_password"], "not-a-real-pw");

        let cleared = clear_account(v);
        let me = &cleared["Users"]["user"][1];
        assert_eq!(me["shadnet_npid"], "");
        assert_eq!(me["shadnet_password"], "");
        assert_eq!(me["shadnet_enabled"], false);
        assert_eq!(me["user_name"], "Me");
        assert_eq!(account(&cleared), None);
    }

    #[test]
    fn without_player_one_the_first_user_is_used() {
        let before = json!({"Users": {"user": [{"user_id": 1003, "player_index": -1}]}});
        let v = set_account(before, &creds());
        assert_eq!(v["Users"]["user"][0]["player_index"], 1);
        assert_eq!(account(&v).unwrap().npid, "Synthetic_1");
    }

    #[test]
    fn disabled_or_incomplete_accounts_do_not_count() {
        let v = json!({"Users": {"user": [{"player_index": 1, "shadnet_npid": "A", "shadnet_password": "b", "shadnet_enabled": false}]}});
        assert_eq!(account(&v), None);
        let v = json!({"Users": {"user": [{"player_index": 1, "shadnet_npid": "A", "shadnet_enabled": true}]}});
        assert_eq!(account(&v), None);
        assert_eq!(clear_account(json!({"a": 1})), json!({"a": 1}));
        assert_eq!(account(&Value::Null), None);
    }
}
