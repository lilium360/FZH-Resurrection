//! The `FriendsState` of the contract, built from `LoginReply` and kept current by notifications.

use super::proto::{LoginReply, Notification};
use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "state", rename_all = "kebab-case")]
pub enum Presence {
    Offline {
        #[serde(rename = "lastSeen")]
        last_seen: Option<String>,
    },
    Online,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Friend {
    pub online_id: String,
    pub online: bool,
    pub presence: Presence,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct FriendsState {
    pub friends: Vec<Friend>,
    pub incoming: Vec<String>,
    pub outgoing: Vec<String>,
}

fn friend(npid: &str, online: bool, last_seen: Option<String>) -> Friend {
    Friend {
        online_id: npid.to_string(),
        online,
        presence: if online { Presence::Online } else { Presence::Offline { last_seen } },
    }
}

fn same(a: &str, b: &str) -> bool {
    a.eq_ignore_ascii_case(b)
}

impl FriendsState {
    pub fn from_login(r: &LoginReply) -> FriendsState {
        FriendsState {
            friends: r.friends.iter().map(|f| friend(&f.npid, f.online, None)).collect(),
            incoming: r.requests_received.clone(),
            outgoing: r.requests_sent.clone(),
        }
    }

    fn forget(&mut self, npid: &str) {
        self.friends.retain(|f| !same(&f.online_id, npid));
        self.incoming.retain(|n| !same(n, npid));
        self.outgoing.retain(|n| !same(n, npid));
    }

    fn befriend(&mut self, npid: &str, online: Option<bool>) {
        let known = self.friends.iter().position(|f| same(&f.online_id, npid));
        let online = online.unwrap_or_else(|| known.is_some_and(|i| self.friends[i].online));
        self.forget(npid);
        self.friends.insert(known.unwrap_or(0).min(self.friends.len()), friend(npid, online, None));
    }

    pub fn is_friend(&self, npid: &str) -> bool {
        self.friends.iter().any(|f| same(&f.online_id, npid))
    }

    fn canonical(&self, npid: &str) -> Option<String> {
        self.incoming.iter().find(|n| same(n, npid)).cloned()
    }

    /// `AddFriend` succeeded: a pending request from `npid` became a friendship, or ours was sent.
    pub fn after_add(&mut self, npid: &str) {
        if self.is_friend(npid) {
            return;
        }
        match self.canonical(npid) {
            Some(name) => self.befriend(&name, None),
            None if !self.outgoing.iter().any(|n| same(n, npid)) => self.outgoing.push(npid.to_string()),
            None => {}
        }
    }

    /// `RemoveFriend` succeeded: a friend removed, our request cancelled or theirs declined.
    pub fn after_remove(&mut self, npid: &str) {
        self.forget(npid);
    }

    /// Returns whether the list changed.
    pub fn apply(&mut self, n: &Notification, now_iso: impl FnOnce(u64) -> String) -> bool {
        let before = self.clone();
        match n {
            Notification::FriendQuery { from } => {
                if !self.is_friend(from) && !self.incoming.iter().any(|x| same(x, from)) {
                    self.outgoing.retain(|x| !same(x, from));
                    self.incoming.push(from.clone());
                }
            }
            Notification::FriendNew { npid, online } => self.befriend(npid, Some(*online)),
            Notification::FriendLost { npid } => self.forget(npid),
            Notification::FriendStatus { npid, online, timestamp_ns } => {
                if let Some(f) = self.friends.iter_mut().find(|f| same(&f.online_id, npid)) {
                    let seen = (!online).then(|| now_iso(*timestamp_ns));
                    *f = friend(&f.online_id.clone(), *online, seen);
                }
            }
            Notification::ClientRejected { .. } | Notification::Invitation { .. } => {}
        }
        *self != before
    }

    /// Offline with "last seen" unknown: what the launcher can say while it has no connection.
    pub fn all_offline(&mut self) {
        for f in &mut self.friends {
            if f.online {
                *f = friend(&f.online_id.clone(), false, None);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shadnet::proto::FriendEntry;
    use serde_json::json;

    fn iso(ns: u64) -> String {
        format!("t{}", ns / 1_000_000_000)
    }

    fn state() -> FriendsState {
        FriendsState::from_login(&LoginReply {
            friends: vec![FriendEntry { npid: "Alpha".into(), online: true }, FriendEntry { npid: "Beta".into(), online: false }],
            requests_sent: vec!["Gamma".into()],
            requests_received: vec!["Delta".into()],
            ..LoginReply::default()
        })
    }

    #[test]
    fn serializes_as_the_contract() {
        let v = serde_json::to_value(state()).unwrap();
        assert_eq!(
            v,
            json!({
                "friends": [
                    {"onlineId": "Alpha", "online": true, "presence": {"state": "online"}},
                    {"onlineId": "Beta", "online": false, "presence": {"state": "offline", "lastSeen": null}}
                ],
                "incoming": ["Delta"],
                "outgoing": ["Gamma"]
            })
        );
    }

    #[test]
    fn notifications_update_the_lists() {
        let mut s = state();
        assert!(s.apply(&Notification::FriendQuery { from: "Kilo".into() }, iso));
        assert_eq!(s.incoming, ["Delta", "Kilo"]);
        assert!(!s.apply(&Notification::FriendQuery { from: "kilo".into() }, iso));
        assert!(s.apply(&Notification::FriendNew { npid: "Gamma".into(), online: true }, iso));
        assert!(s.outgoing.is_empty());
        assert_eq!(s.friends[0], friend("Gamma", true, None));
        assert!(s.apply(&Notification::FriendStatus { npid: "Alpha".into(), online: false, timestamp_ns: 5_000_000_000 }, iso));
        assert_eq!(s.friends[1].presence, Presence::Offline { last_seen: Some("t5".into()) });
        assert!(s.apply(&Notification::FriendLost { npid: "Beta".into() }, iso));
        assert!(!s.is_friend("Beta"));
        assert!(!s.apply(&Notification::FriendStatus { npid: "Nobody".into(), online: true, timestamp_ns: 0 }, iso));
        assert!(s.apply(&Notification::FriendLost { npid: "Kilo".into() }, iso));
        assert_eq!(s.incoming, ["Delta"]);
    }

    #[test]
    fn replies_update_the_lists() {
        let mut s = state();
        s.after_add("delta");
        assert!(s.incoming.is_empty());
        assert_eq!(s.friends[0].online_id, "Delta");
        s.after_add("Zulu");
        s.after_add("Zulu");
        assert_eq!(s.outgoing, ["Gamma", "Zulu"]);
        s.after_add("Alpha");
        assert_eq!(s.friends.len(), 3);
        s.after_remove("Zulu");
        s.after_remove("Alpha");
        assert_eq!(s.outgoing, ["Gamma"]);
        assert!(!s.is_friend("Alpha"));
        let mut t = state();
        t.apply(&Notification::FriendNew { npid: "Delta".into(), online: false }, iso);
        t.after_add("Delta");
        assert_eq!(t.friends.iter().filter(|f| f.online_id == "Delta").count(), 1);
        t.all_offline();
        assert!(t.friends.iter().all(|f| !f.online));
    }
}
