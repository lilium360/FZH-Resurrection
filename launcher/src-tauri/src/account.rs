//! The launcher's shadNet session: registration, sign-in with a recovery code, friends, server
//! status, the play grant before a launch.
//!
//! The password lives in the Windows Credential Manager (`vault.rs`), the launcher's own copy, and
//! in the instance's `users.json` (`users.rs`), where the emulator reads it. The recovery code is
//! never stored. The launcher holds one connection while it is open and the game is not running: shadNet
//! refuses a second session of the same account, so `suspend` closes it before a launch and
//! `resume` lets the monitor sign in again after the game exits. While the launcher holds the
//! session, shadNet sends it the invitations to game sessions as well (`invite` event); while the
//! game runs, they reach the game.

use crate::backend_api::{self, Play};
use crate::error::{self, BackendError, Msg, Res};
use crate::instance::{self, Server};
use crate::layout::Layout;
use crate::shadnet::connection::{self, Connection, Event};
use crate::shadnet::friends::FriendsState;
use crate::shadnet::proto::Notification;
use crate::shadnet::{self, packet};
use crate::users::{self, Credentials};
use crate::vault::{self, Vault};
use crate::{avatar, clock, recovery, store};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

/// The email the backend gives every account it registers; older launchers used `fzh.invalid`.
const PLACEHOLDER_EMAIL_DOMAINS: [&str; 2] = ["users.invalid", "fzh.invalid"];
/// The backend redeems only codes issued to this client id (`OAUTH_CLIENT` in backend.py).
const PLAY_CLIENT: &str = "fzh-backend";
const PLAY_SCOPE: &str = "fzh:play";
const MONITOR_INTERVAL: Duration = Duration::from_secs(15);
const PROBE_TIMEOUT: Duration = Duration::from_millis(1500);
const AVATARS: u64 = 5;

/// What the account service needs from the application: the Tauri app, or a test harness.
pub trait Env: Send + Sync + 'static {
    fn layout(&self) -> &Layout;
    fn account(&self) -> &Account;
    fn emit(&self, event: &str, payload: Value);
    fn log(&self, line: Msg);
    /// Reported with `SetClientVersion (13)`.
    fn client_version(&self) -> String;
    fn vault(&self) -> &dyn Vault;
}

pub type EnvRef = Arc<dyn Env>;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Session {
    pub online_id: String,
    pub email: String,
    /// A preset index, or "image".
    pub avatar: Value,
    pub image: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountState {
    pub session: Option<Session>,
    pub last_online_id: String,
}

/// The outcome of a registration. The account exists once the backend said so, so the code is
/// returned even when storing or signing in failed (`problem`); `session` is null when the account
/// could not be stored on this PC.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Registration {
    pub recovery_code: String,
    pub session: Option<Session>,
    pub problem: Option<Msg>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileUpdate {
    pub online_id: String,
    pub avatar: Value,
    #[serde(default)]
    pub image: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerStatus {
    pub online: bool,
    pub players: Option<u32>,
    pub region: String,
    pub network: bool,
    pub progression: bool,
}

#[derive(Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct Prefs {
    last_online_id: String,
    avatar: u64,
    image: bool,
}

#[derive(Default)]
struct Inner {
    conn: Option<Arc<Connection>>,
    host: String,
    active: u64,
    next: u64,
    friends: FriendsState,
    suspended: bool,
    status: Option<ServerStatus>,
    problem: String,
}

#[derive(Default)]
pub struct Account {
    connect: Mutex<()>,
    inner: Mutex<Inner>,
    wake: (Mutex<bool>, Condvar),
}

fn host(env: &EnvRef) -> String {
    store::load_settings(&env.layout().settings_json()).server()
}

fn stored(env: &EnvRef) -> Option<Credentials> {
    users::account(&instance::read_json(&env.layout().users_json()))
}

/// The stored account, with the Credential Manager's password when it has one.
pub fn credentials(env: &EnvRef) -> Option<Credentials> {
    let mut c = stored(env)?;
    if let Some(password) = env.vault().get(&vault::target(&host(env), &c.npid)) {
        c.password = password;
    }
    Some(c)
}

fn edit_users(env: &EnvRef, f: impl FnOnce(Value) -> Value) -> Res<()> {
    let path = env.layout().users_json();
    instance::write_json(&path, &f(instance::read_json(&path)))
}

fn prefs(env: &EnvRef) -> Prefs {
    std::fs::read(env.layout().account_json()).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

fn save_prefs(env: &EnvRef, p: &Prefs) -> Res<()> {
    let bytes = serde_json::to_vec_pretty(p).map_err(|e| BackendError::internal(e.to_string()))?;
    store::write_atomic(&env.layout().account_json(), &bytes)
}

fn registered_email(online_id: &str) -> String {
    format!("{}@{}", online_id.to_ascii_lowercase(), PLACEHOLDER_EMAIL_DOMAINS[0])
}

fn session(env: &EnvRef, c: &Credentials) -> Session {
    let placeholder = PLACEHOLDER_EMAIL_DOMAINS.iter().any(|d| c.email.ends_with(&format!("@{d}")));
    let email = if placeholder { String::new() } else { c.email.clone() };
    let p = prefs(env);
    let image = std::fs::read(env.layout().avatar_png()).ok().filter(|b| avatar::check_png(b).is_ok()).map(|b| avatar::data_url(&b));
    let choice = if p.image && image.is_some() { Value::from("image") } else { Value::from(p.avatar.min(AVATARS - 1)) };
    Session { online_id: c.npid.clone(), email, avatar: choice, image }
}

fn emit_friends(env: &EnvRef, f: &FriendsState) {
    env.emit("friends", serde_json::to_value(f).unwrap_or(Value::Null));
}

fn game_running_error() -> BackendError {
    BackendError::new("game_running", "backend.game_running_friends", "The game is running: use the in-game friends panel (F2) until it closes.")
}

fn not_signed_in() -> BackendError {
    BackendError::new("not_signed_in", "backend.not_signed_in_short", "Sign in first.")
}

fn online_id_format() -> BackendError {
    BackendError::invalid("backend.online_id_format", "Online ID: 3 to 16 letters, digits, - and _.")
}

fn signed_in_line(id: &str, host: &str) -> Msg {
    let server = crate::settings::server_name(host);
    Msg::new("log.event.signedIn", format!("Network: signed in as {id} on the {}", server.message)).with("id", id).with("server", server)
}

impl Account {
    fn next_gen(&self) -> u64 {
        let mut i = self.inner.lock().unwrap();
        i.next += 1;
        i.next
    }

    fn take_conn(&self) -> Option<Arc<Connection>> {
        let mut i = self.inner.lock().unwrap();
        i.active = 0;
        i.conn.take()
    }

    fn live_conn(&self, host: &str) -> Option<Arc<Connection>> {
        let i = self.inner.lock().unwrap();
        i.conn.clone().filter(|c| !c.is_closed() && i.host == host)
    }

    pub fn wake(&self) {
        *self.wake.0.lock().unwrap() = true;
        self.wake.1.notify_all();
    }

    fn on_event(env: &EnvRef, gen: u64, e: Event) {
        let account = env.account();
        match e {
            Event::Notification(kind, payload) => {
                let Some(n) = shadnet::decode_notification(kind, &payload) else { return };
                if let Notification::ClientRejected { message } = &n {
                    env.log(
                        Msg::new("log.event.serverRefused", format!("Network: the server turned the launcher away: {message}"))
                            .with("reason", message.clone()),
                    );
                    return;
                }
                let mut i = account.inner.lock().unwrap();
                if i.active != gen {
                    return;
                }
                if let Notification::Invitation { from } = &n {
                    drop(i);
                    env.log(Msg::new("log.event.invited", format!("Network: {from} invited you to a match")).with("name", from.clone()));
                    env.emit("invite", serde_json::json!({ "from": from }));
                    return;
                }
                if i.friends.apply(&n, clock::iso_utc_from_ns) {
                    emit_friends(env, &i.friends);
                }
            }
            Event::Closed => {
                let mut i = account.inner.lock().unwrap();
                if i.active != gen {
                    return;
                }
                let lost = i.conn.take().is_some_and(|c| !c.is_closing());
                i.active = 0;
                if lost {
                    i.friends.all_offline();
                    emit_friends(env, &i.friends);
                    drop(i);
                    env.log(Msg::new("log.event.connectionLost", "Network: the launcher lost the connection to the server"));
                    account.wake();
                }
            }
        }
    }

    /// Opens a connection, signs in and installs the session. Called with `self.connect` held.
    fn connect_locked(&self, env: &EnvRef, host: &str, c: &Credentials) -> Res<()> {
        if let Some(old) = self.take_conn() {
            old.close();
        }
        if self.inner.lock().unwrap().suspended {
            return Err(game_running_error());
        }
        let gen = self.next_gen();
        let handle = env.clone();
        let at = Server::of(host);
        let conn = Connection::open(&at.host, at.game, shadnet::CONNECT_TIMEOUT, move |e| Account::on_event(&handle, gen, e))
            .map_err(|e| shadnet::net_error(host, &e))?;
        let result = (|| {
            let reply = shadnet::login(&conn, host, &c.npid, &c.password)?;
            {
                let mut i = self.inner.lock().unwrap();
                if i.suspended {
                    return Err(game_running_error());
                }
                i.conn = Some(conn.clone());
                i.host = host.to_string();
                i.active = gen;
                i.friends = FriendsState::from_login(&reply);
                i.problem.clear();
                emit_friends(env, &i.friends);
            }
            shadnet::report_version(&conn, host, &env.client_version())
        })();
        if result.is_err() {
            if self.inner.lock().unwrap().active == gen {
                self.take_conn();
            }
            conn.close();
        }
        result
    }

    /// The signed-in connection, opening it with the stored credentials when needed.
    fn ensure(&self, env: &EnvRef) -> Res<(Arc<Connection>, String)> {
        let host = host(env);
        let _g = self.connect.lock().unwrap();
        if let Some(c) = self.live_conn(&host) {
            return Ok((c, host));
        }
        if self.inner.lock().unwrap().suspended {
            return Err(game_running_error());
        }
        let creds = credentials(env).ok_or_else(not_signed_in)?;
        self.connect_locked(env, &host, &creds)?;
        env.log(Msg::new("log.event.launcherSignedIn", format!("Network: launcher signed in as {}", creds.npid)).with("id", creds.npid.clone()));
        self.live_conn(&host)
            .map(|c| (c, host))
            .ok_or_else(|| BackendError::new("server_unreachable", "backend.connection_lost", "The connection to the server was lost."))
    }

    pub fn get_account(&self, env: &EnvRef) -> AccountState {
        AccountState { session: stored(env).map(|c| session(env, &c)), last_online_id: prefs(env).last_online_id }
    }

    /// Stores the account for the emulator (`users.json`) and the launcher's preferences.
    fn store(&self, env: &EnvRef, c: &Credentials) -> Res<Session> {
        edit_users(env, |v| users::set_account(v, c))?;
        let mut p = prefs(env);
        p.last_online_id = c.npid.clone();
        let _ = save_prefs(env, &p);
        Ok(session(env, c))
    }

    /// The Credential Manager's copy, replacing the one of an account stored before. A failure only
    /// costs that copy: `users.json` still holds the password.
    fn keep_secret(&self, env: &EnvRef, host: &str, c: &Credentials) {
        if let Some(old) = stored(env).filter(|o| !o.npid.eq_ignore_ascii_case(&c.npid)) {
            env.vault().delete(&vault::target(host, &old.npid));
        }
        if let Err(e) = env.vault().set(&vault::target(host, &c.npid), &c.npid, &c.password) {
            env.log(
                Msg::new("log.event.vaultFailed", format!("Launcher: the account is not in the Windows Credential Manager: {}", e.message))
                    .with("reason", e.msg()),
            );
        }
    }

    fn refuse_while_playing(&self) -> Res<()> {
        if self.inner.lock().unwrap().suspended {
            return Err(BackendError::new("game_running", "backend.account_game_running", "Close the game first: it uses the account while it runs."));
        }
        Ok(())
    }

    /// Registers the Online ID through the backend with a password derived from a new recovery
    /// code, stores the account and signs in.
    pub fn register(&self, env: &EnvRef, online_id: &str) -> Res<Registration> {
        let online_id = online_id.trim();
        if !shadnet::valid_online_id(online_id) {
            return Err(online_id_format());
        }
        let code = recovery::generate()?;
        let canonical = recovery::canonical(&code).ok_or_else(|| BackendError::internal("generated recovery code is not canonical"))?;
        let host = host(env);
        let _g = self.connect.lock().unwrap();
        self.refuse_while_playing()?;
        let npid = backend_api::register(&host, online_id, &recovery::password(&canonical, online_id))?;
        let c = Credentials { password: recovery::password(&canonical, &npid), email: registered_email(&npid), npid };
        let server = crate::settings::server_name(&host);
        env.log(
            Msg::new("log.event.accountCreated", format!("Network: account {} created on the {}", c.npid, server.message))
                .with("id", c.npid.clone())
                .with("server", server),
        );
        self.keep_secret(env, &host, &c);
        let (session, problem) = match self.store(env, &c) {
            Err(e) => (None, Some(e.msg())),
            Ok(s) => match self.connect_locked(env, &host, &c) {
                Ok(()) => {
                    env.log(signed_in_line(&c.npid, &host));
                    (Some(s), None)
                }
                Err(e) => (Some(s), Some(e.msg())),
            },
        };
        Ok(Registration { recovery_code: code, session, problem })
    }

    /// Signs in to an existing account with its recovery code, on any PC.
    pub fn sign_in(&self, env: &EnvRef, online_id: &str, code: &str) -> Res<Session> {
        let typed = online_id.trim();
        if !shadnet::valid_online_id(typed) {
            return Err(online_id_format());
        }
        let code = recovery::canonical(code).ok_or_else(|| {
            BackendError::invalid("backend.recovery_code_invalid", "That is not a recovery code: it has 25 letters and digits in five groups.")
        })?;
        let host = host(env);
        let _g = self.connect.lock().unwrap();
        self.refuse_while_playing()?;
        // shadNet needs the registered casing; a name this backend did not register is tried as typed.
        let npid = match backend_api::registered_name(&host, typed) {
            Ok(Some(name)) => name,
            Err(e) if e.code == "busy" => return Err(e),
            _ => typed.to_string(),
        };
        let email = stored(env).filter(|c| c.npid == npid).map(|c| c.email).unwrap_or_else(|| registered_email(&npid));
        let c = Credentials { password: recovery::password(&code, &npid), email, npid };
        self.connect_locked(env, &host, &c).map_err(|e| {
            if e.key == "backend.login_wrong" {
                BackendError::new("wrong_password", "backend.recover_wrong", "Wrong Online ID or recovery code.")
            } else {
                e
            }
        })?;
        self.keep_secret(env, &host, &c);
        env.log(signed_in_line(&c.npid, &host));
        self.store(env, &c).inspect_err(|_| {
            if let Some(conn) = self.take_conn() {
                conn.close();
            }
        })
    }

    /// Closes the session and removes the credentials from users.json. The caller stops the game first.
    pub fn sign_out(&self, env: &EnvRef) -> Res<()> {
        let _g = self.connect.lock().unwrap();
        if let Some(conn) = self.take_conn() {
            conn.close();
        }
        if let Some(c) = stored(env) {
            env.vault().delete(&vault::target(&host(env), &c.npid));
        }
        edit_users(env, users::clear_account)?;
        let mut i = self.inner.lock().unwrap();
        i.friends = FriendsState::default();
        i.problem.clear();
        emit_friends(env, &i.friends);
        drop(i);
        env.log(Msg::new("log.event.signedOut", "Network: signed out"));
        Ok(())
    }

    pub fn update_profile(&self, env: &EnvRef, p: &ProfileUpdate) -> Res<Session> {
        let c = stored(env).ok_or_else(not_signed_in)?;
        if p.online_id.trim() != c.npid {
            return Err(BackendError::unsupported("backend.changing_online_id", "Changing the Online ID is not available yet."));
        }
        let picture = p.avatar.as_str() == Some("image");
        let preset = p.avatar.as_u64().filter(|&a| a < AVATARS);
        let image = p.image.as_deref().map(avatar::from_data_url).transpose()?;
        if (!picture && preset.is_none()) || (picture && image.is_none()) {
            return Err(BackendError::invalid("backend.avatar_choice", "Choose one of the avatars."));
        }
        let file = env.layout().avatar_png();
        match &image {
            Some(png) => store::write_atomic(&file, png)?,
            None if file.exists() => std::fs::remove_file(&file).map_err(|e| BackendError::io_path(file.display(), e))?,
            None => {}
        }
        let mut prefs = prefs(env);
        prefs.image = picture;
        if let Some(a) = preset {
            prefs.avatar = a;
        }
        save_prefs(env, &prefs)?;
        Ok(session(env, &c))
    }

    /// Sends the avatar (a 256x256 PNG data URL) to the backend for the signed-in account and logs
    /// the outcome, which the UI does not otherwise show after a sign-in.
    pub fn upload_avatar(&self, env: &EnvRef, image: &str) -> Res<()> {
        let result = (|| {
            let png = avatar::from_data_url(image)?;
            let c = stored(env).ok_or_else(not_signed_in)?;
            let (conn, host) = self.ensure(env)?;
            let token = shadnet::get_token(&conn, &host)?;
            avatar::upload(&host, &c.npid, &token.token, &png)
        })();
        match &result {
            Ok(()) => env.log(Msg::new("log.event.avatarSent", "Network: avatar sent to the server")),
            Err(e) => env.log(Msg::new("log.event.avatarNotSent", format!("Network: avatar not sent: {}", e.message)).with("reason", e.msg())),
        }
        result
    }

    pub fn get_friends(&self, env: &EnvRef) -> Res<FriendsState> {
        if stored(env).is_none() {
            return Err(BackendError::new("not_signed_in", "backend.not_signed_in_friends", "Sign in to see your friends."));
        }
        if self.live_conn(&host(env)).is_none() {
            self.wake();
        }
        Ok(self.inner.lock().unwrap().friends.clone())
    }

    fn friend_op(&self, env: &EnvRef, command: u16, npid: &str, after: fn(&mut FriendsState, &str)) -> Res<FriendsState> {
        let npid = npid.trim();
        if npid.is_empty() {
            return Err(BackendError::invalid("backend.enter_online_id", "Enter an Online ID."));
        }
        let (conn, host) = self.ensure(env)?;
        shadnet::friend_command(&conn, &host, command, npid)?;
        let mut i = self.inner.lock().unwrap();
        if i.conn.as_ref().is_some_and(|c| Arc::ptr_eq(c, &conn)) {
            after(&mut i.friends, npid);
            emit_friends(env, &i.friends);
        }
        Ok(i.friends.clone())
    }

    pub fn send_friend_request(&self, env: &EnvRef, npid: &str) -> Res<FriendsState> {
        self.friend_op(env, packet::ADD_FRIEND, npid, FriendsState::after_add)
    }

    pub fn accept_friend_request(&self, env: &EnvRef, npid: &str) -> Res<FriendsState> {
        self.friend_op(env, packet::ADD_FRIEND, npid, FriendsState::after_add)
    }

    pub fn decline_friend_request(&self, env: &EnvRef, npid: &str) -> Res<FriendsState> {
        self.friend_op(env, packet::REMOVE_FRIEND, npid, FriendsState::after_remove)
    }

    pub fn remove_friend(&self, env: &EnvRef, npid: &str) -> Res<FriendsState> {
        self.friend_op(env, packet::REMOVE_FRIEND, npid, FriendsState::after_remove)
    }

    pub fn is_signed_in(&self, env: &EnvRef) -> bool {
        stored(env).is_some()
    }

    pub fn is_connected(&self, env: &EnvRef) -> bool {
        self.live_conn(&host(env)).is_some()
    }

    /// Before a launch, while the launcher still holds the session: a single-use code from shadNet,
    /// redeemed at the backend, which then accepts the game's login for this account from this
    /// address. Never blocks the launch, since a backend in `open` mode does not need it; returns
    /// the warning for the player when it failed.
    pub fn play_grant(&self, env: &EnvRef) -> Option<Msg> {
        let result = (|| {
            let (conn, host) = self.ensure(env)?;
            let code = shadnet::authorization_code(&conn, &host, PLAY_CLIENT, PLAY_SCOPE)?;
            backend_api::play(&host, &code.code)
        })();
        match result {
            Ok(Play::Granted(g)) => {
                env.log(
                    Msg::new("log.event.playGranted", format!("Network: the server lets {} play for {} s", g.online_id, g.expires_in))
                        .with("id", g.online_id)
                        .with("duration", error::seconds(g.expires_in)),
                );
                None
            }
            Ok(Play::NotOffered) => {
                env.log(Msg::new("log.event.playGrantNotOffered", "Network: this server does not ask the launcher to vouch for the player"));
                None
            }
            Err(e) => {
                env.log(
                    Msg::new("log.event.playGrantFailed", format!("Network: the server did not confirm the account for this game: {}", e.message))
                        .with("reason", e.msg()),
                );
                Some(
                    Msg::new("backend.play_grant_failed", format!("The server did not confirm your account: {} The game starts anyway, but it may not sign in.", e.message))
                        .with("reason", e.msg()),
                )
            }
        }
    }

    /// Before a launch: the game signs in with the same account, so the launcher lets go of it.
    pub fn suspend(&self, env: &EnvRef) {
        let _g = self.connect.lock().unwrap();
        self.inner.lock().unwrap().suspended = true;
        if let Some(conn) = self.take_conn() {
            conn.close();
            env.log(Msg::new("log.event.handoverToGame", "Network: launcher signed out so that the game can sign in"));
        }
    }

    /// After the game: the monitor signs in again and refreshes the friends list.
    pub fn resume(&self) {
        self.inner.lock().unwrap().suspended = false;
        self.wake();
    }

    pub fn server_status(&self, env: &EnvRef) -> ServerStatus {
        let host = host(env);
        let at = Server::of(&host);
        let network = self.live_conn(&host).is_some() || connection::port_open(&at.host, at.game, PROBE_TIMEOUT);
        let progression = connection::port_open(&at.host, at.backend, PROBE_TIMEOUT);
        let status = ServerStatus { online: network && progression, players: None, region: host, network, progression };
        let mut i = self.inner.lock().unwrap();
        if i.status.as_ref() != Some(&status) {
            i.status = Some(status.clone());
            env.emit("server-status", serde_json::to_value(&status).unwrap_or(Value::Null));
        }
        status
    }

    /// One round of the monitor: server status, and signing in again when the session is gone.
    pub fn tick(&self, env: &EnvRef) {
        let host = host(env);
        let moved = {
            let i = self.inner.lock().unwrap();
            i.conn.is_some() && i.host != host
        };
        if moved {
            let _g = self.connect.lock().unwrap();
            if let Some(conn) = self.take_conn() {
                conn.close();
            }
        }
        let status = self.server_status(env);
        let wanted = !self.inner.lock().unwrap().suspended && self.live_conn(&host).is_none() && stored(env).is_some();
        if !wanted || !status.network {
            return;
        }
        if let Err(e) = self.ensure(env) {
            let mut i = self.inner.lock().unwrap();
            if e.code != "not_signed_in" && i.problem != e.message {
                i.problem = e.message.clone();
                drop(i);
                env.log(Msg::new("log.event.launcherNotSignedIn", format!("Network: launcher not signed in: {}", e.message)).with("reason", e.msg()));
            }
        }
    }

    pub fn start_monitor(env: EnvRef) {
        std::thread::spawn(move || loop {
            let account = env.account();
            account.tick(&env);
            let guard = account.wake.0.lock().unwrap();
            let (mut woken, _) = account.wake.1.wait_timeout_while(guard, MONITOR_INTERVAL, |w| !*w).unwrap();
            *woken = false;
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn placeholder_and_shapes() {
        assert_eq!(registered_email("Player_1"), "player_1@users.invalid");
        let r = Registration { recovery_code: "C".into(), session: None, problem: Some(Msg::new("backend.net_closed", "x")) };
        assert_eq!(serde_json::to_value(r).unwrap(), serde_json::json!({"recoveryCode": "C", "session": null, "problem": {"key": "backend.net_closed", "message": "x"}}));
        let s = ServerStatus { online: false, players: None, region: "127.0.0.1".into(), network: true, progression: false };
        assert_eq!(
            serde_json::to_value(s).unwrap(),
            serde_json::json!({"online": false, "players": null, "region": "127.0.0.1", "network": true, "progression": false})
        );
    }
}
