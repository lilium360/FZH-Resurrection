//! shadNet client: framing (`packet`), messages (`proto`), the TCP connection (`connection`), the
//! friends list (`friends`) and the commands the launcher sends, with their errors as user messages.

pub mod connection;
pub mod friends;
pub mod packet;
pub mod proto;

use crate::error::{BackendError, Res};
use crate::instance::Server;
use crate::settings::default_server;
use connection::{Connection, NetError};
use std::time::Duration;

pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(8);

pub fn valid_online_id(s: &str) -> bool {
    (3..=16).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// The default server is only ever "the community server": its address is not shown to the player.
pub fn net_error(host: &str, e: &NetError) -> BackendError {
    let official = host == default_server();
    let at = Server::of(host);
    let err = |key, english: String| BackendError::new("server_unreachable", key, english);
    match e {
        NetError::Closed => err("backend.net_closed", "The connection to the server was lost. Try again in a moment.".into()),
        NetError::Unreachable(_) | NetError::Timeout if official => {
            err("backend.net_official_unreachable", "Cannot reach the community server. Check your connection and try again.".into())
        }
        NetError::Unreachable(_) | NetError::Timeout => err(
            "backend.net_private_unreachable",
            format!("Cannot reach the private server {} (port {}). Check the address and your connection.", at.host, at.game),
        )
        .with("host", at.host.as_str())
        .with("port", at.game),
        NetError::Protocol(_) if official => {
            err("backend.net_official_protocol", "The community server did not answer as expected. Try again later.".into())
        }
        NetError::Protocol(m) => err("backend.net_private_protocol", format!("The private server {host} did not answer as a shadNet server ({m})."))
            .with("host", host)
            .with("detail", m.clone()),
    }
}

pub fn unreachable_server(host: &str) -> BackendError {
    net_error(host, &NetError::Timeout)
}

fn common_error(code: u8) -> BackendError {
    match code {
        4 => BackendError::new("busy", "backend.net_too_many", "Too many attempts: wait a moment and try again."),
        23 => BackendError::new("not_signed_in", "backend.net_session_expired", "The server asks you to sign in again."),
        24 => BackendError::new("internal", "backend.net_server_db", "The server could not read or write its database."),
        33 => BackendError::new("unsupported", "backend.net_unsupported", "The server does not offer this feature."),
        c => BackendError::new("internal", "backend.net_refused", format!("The server refused the request (error {c}).")).with("code", c),
    }
}

pub fn login_error(code: u8) -> BackendError {
    match code {
        8 | 7 => BackendError::new(
            "wrong_password",
            "backend.login_wrong",
            "Wrong Online ID or password. The Online ID must be typed with the same capital letters as when it was created.",
        ),
        9 => BackendError::new("wrong_password", "backend.login_token", "This server needs an account token the launcher does not have."),
        6 => BackendError::new(
            "already_signed_in",
            "backend.login_already",
            "This account is already signed in elsewhere: in a running game or on another PC.",
        ),
        5 => BackendError::new(
            "unauthorized",
            "backend.login_banned",
            "The server does not let this account sign in: it is banned, or blocked for a few minutes.",
        ),
        c => common_error(c),
    }
}

/// shadNet answered a `Login` with `LoginAlreadyLoggedIn` (6): the account has a session already,
/// in another launcher, in a game or on another PC, and the game's own login would get the same answer.
pub fn in_use_elsewhere(e: &BackendError) -> bool {
    e.code == "already_signed_in"
}

pub fn friend_error(command: u16, code: u8) -> BackendError {
    match (command, code) {
        (packet::ADD_FRIEND, 26) => BackendError::new("not_found", "backend.friend_not_found", "No player with that Online ID."),
        (_, 26) => BackendError::new("not_found", "backend.friend_not_in_list", "That player is not in your friends or requests."),
        (_, 27) => BackendError::new("blocked", "backend.friend_blocked", "You cannot be friends with that player: one of you blocked the other."),
        (_, 28) => BackendError::new("already_friend", "backend.friend_already", "Already your friend, or your request is waiting for an answer."),
        (_, 3) => BackendError::invalid("toast.friendSelf", "That is your own Online ID."),
        (_, c) => common_error(c),
    }
}

fn send(conn: &Connection, host: &str, command: u16, proto: &[u8]) -> Res<connection::Reply> {
    conn.request(command, &packet::blob(proto), REQUEST_TIMEOUT).map_err(|e| net_error(host, &e))
}

pub fn login(conn: &Connection, host: &str, npid: &str, password: &str) -> Res<proto::LoginReply> {
    let r = send(conn, host, packet::LOGIN, &proto::login_request(npid, password))?;
    if r.error != 0 {
        return Err(login_error(r.error));
    }
    packet::read_blob(&r.payload)
        .and_then(|b| proto::LoginReply::decode(b).ok())
        .ok_or_else(|| BackendError::new("internal", "backend.net_official_protocol", "The server's sign-in reply could not be read."))
}

/// A single-use code for a game backend, issued to the signed-in session once its client version
/// was accepted (else error 23).
pub fn authorization_code(conn: &Connection, host: &str, client_id: &str, scope: &str) -> Res<proto::AuthorizationCodeReply> {
    let r = send(conn, host, packet::GET_AUTHORIZATION_CODE, &proto::authorization_code_request(client_id, scope))?;
    if r.error != 0 {
        return Err(common_error(r.error));
    }
    packet::read_blob(&r.payload)
        .and_then(|b| proto::AuthorizationCodeReply::decode(b).ok())
        .filter(|a| !a.code.is_empty())
        .ok_or_else(|| BackendError::new("internal", "backend.net_official_protocol", "The server's authorization reply could not be read."))
}

/// The bearer token of the signed-in session for shadNet's WebAPI. The request has no payload at all,
/// as the emulator sends it.
pub fn get_token(conn: &Connection, host: &str) -> Res<proto::TokenReply> {
    let r = conn.request(packet::GET_TOKEN, &[], REQUEST_TIMEOUT).map_err(|e| net_error(host, &e))?;
    if r.error != 0 {
        return Err(common_error(r.error));
    }
    packet::read_blob(&r.payload)
        .and_then(|b| proto::TokenReply::decode(b).ok())
        .filter(|t| !t.token.is_empty())
        .ok_or_else(|| BackendError::new("internal", "backend.net_official_protocol", "The server's token reply could not be read."))
}

/// shadNet keeps a new session hidden until the client reports a version its `clientversions.cfg`
/// accepts (the default denies "empty"), and rejects it after `ClientVersionTimeoutSeconds`.
pub fn report_version(conn: &Connection, host: &str, version: &str) -> Res<()> {
    let r = send(conn, host, packet::SET_CLIENT_VERSION, &proto::client_version(version))?;
    if r.error != 0 {
        return Err(common_error(r.error));
    }
    Ok(())
}

pub fn friend_command(conn: &Connection, host: &str, command: u16, npid: &str) -> Res<()> {
    let r = send(conn, host, command, &proto::friend_command(npid))?;
    if r.error != 0 {
        return Err(friend_error(command, r.error));
    }
    Ok(())
}

pub fn decode_notification(kind: u16, payload: &[u8]) -> Option<proto::Notification> {
    if kind == proto::WEB_API_PUSH_EVENT {
        return proto::invitation(payload);
    }
    proto::Notification::decode(kind, packet::read_blob(payload)?).ok().flatten()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn online_ids() {
        assert!(valid_online_id("Player_1"));
        assert!(valid_online_id("a-b"));
        assert!(!valid_online_id("ab"));
        assert!(!valid_online_id("seventeen_chars_x"));
        assert!(!valid_online_id("with space"));
        assert!(!valid_online_id("accentè"));
    }

    #[test]
    fn error_codes_become_contract_codes() {
        assert_eq!(login_error(8).code, "wrong_password");
        assert_eq!(login_error(6).code, "already_signed_in");
        assert!(in_use_elsewhere(&login_error(6)));
        assert!(!(0..=u8::MAX).any(|c| c != 6 && in_use_elsewhere(&login_error(c))), "only the answer 6");
        assert!(!in_use_elsewhere(&net_error("10.0.0.1", &NetError::Timeout)));
        assert_eq!(login_error(5).code, "unauthorized");
        assert_eq!(friend_error(packet::ADD_FRIEND, 26).code, "not_found");
        assert_eq!(friend_error(packet::ADD_FRIEND, 28).code, "already_friend");
        assert_eq!(friend_error(packet::REMOVE_FRIEND, 27).code, "blocked");
        assert_eq!(net_error("10.0.0.1", &NetError::Timeout).code, "server_unreachable");
        assert!(net_error("10.0.0.1", &NetError::Timeout).message.contains("private server 10.0.0.1"));
        let official = net_error(default_server(), &NetError::Timeout).message;
        assert_eq!(official, "Cannot reach the community server. Check your connection and try again.");
        assert!(!net_error(default_server(), &NetError::Protocol("x".into())).message.contains(default_server()));
    }

    #[test]
    fn errors_carry_i18n_keys_and_hide_the_official_address() {
        assert_eq!(login_error(8).key, "backend.login_wrong");
        let private = net_error("10.0.0.1", &NetError::Timeout);
        assert_eq!((private.key, private.params["host"].as_str(), private.params["port"].as_u64()), ("backend.net_private_unreachable", Some("10.0.0.1"), Some(31313)));
        let official = net_error(default_server(), &NetError::Timeout);
        assert_eq!(official.key, "backend.net_official_unreachable");
        assert!(official.params.is_empty());
        assert_eq!(common_error(99).params["code"], 99);
    }
}
