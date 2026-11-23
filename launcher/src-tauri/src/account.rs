//! The launcher's shadNet session: registration, sign-in with a recovery code, friends, server
//! status, the play grant before a launch.
//!
//! The password lives in the Windows Credential Manager (`vault.rs`), the launcher's own copy, and
//! in the instance's `users.json` (`users.rs`), where the emulator reads it. The recovery code is
//! never stored. Accounts are per server: `users.json` holds the one of the server in use, and
//! `accounts.json` (`accounts.rs`) remembers which account each other server was left with, so
//! `change_server` can take it up again. The launcher holds one connection while it is open and the game is not running: shadNet
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
use crate::{accounts, avatar, clock, recovery, store};
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
    /// The server's last answer to the launcher's sign-in was that the account is in use elsewhere.
    pub in_use_elsewhere: bool,
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
    in_use: bool,
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

fn edit_remembered(env: &EnvRef, f: impl FnOnce(&mut accounts::Accounts)) -> Res<()> {
    let path = env.layout().accounts_json();
    let before = accounts::load(&path);
    let mut after = accounts::load(&path);
    f(&mut after);
    if after == before {
        return Ok(());
    }
    accounts::save(&path, &after)
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

/// Why a launch or a warm-up is refused: the game's own sign-in would be refused the same way, and
/// the game does not survive that.
fn in_use_elsewhere() -> BackendError {
    BackendError::new(
        "already_signed_in",
        "backend.account_in_use",
        "This account is in use in another launcher, in a running game or on another PC. Close it there first, then try again.",
    )
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

    /// The `account` event tells the UI when this changes.
    fn set_in_use(&self, env: &EnvRef, in_use: bool) {
        let before = std::mem::replace(&mut self.inner.lock().unwrap().in_use, in_use);
        if before != in_use {
            env.emit("account", serde_json::to_value(self.get_account(env)).unwrap_or(Value::Null));
        }
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
        self.set_in_use(env, result.as_ref().is_err_and(shadnet::in_use_elsewhere));
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
        let in_use_elsewhere = self.inner.lock().unwrap().in_use;
        AccountState { session: stored(env).map(|c| session(env, &c)), last_online_id: prefs(env).last_online_id, in_use_elsewhere }
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

    fn clear_friends(&self, env: &EnvRef) {
        let mut i = self.inner.lock().unwrap();
        i.friends = FriendsState::default();
        i.problem.clear();
        i.in_use = false;
        emit_friends(env, &i.friends);
    }

    /// Closes the session, removes the credentials from users.json and forgets the account of this
    /// server on this PC. The caller stops the game first.
    pub fn sign_out(&self, env: &EnvRef) -> Res<()> {
        let _g = self.connect.lock().unwrap();
        if let Some(conn) = self.take_conn() {
            conn.close();
        }
        let host = host(env);
        if let Some(c) = stored(env) {
            env.vault().delete(&vault::target(&host, &c.npid));
        }
        edit_users(env, users::clear_account)?;
        let _ = edit_remembered(env, |a| a.forget(&host));
        self.clear_friends(env);
        env.log(Msg::new("log.event.signedOut", "Network: signed out"));
        Ok(())
    }

    /// The player moves from the server `from` to `to`; `save` writes the settings that say so.
    /// The session closes and the account in use stays remembered for `from`, its password in the
    /// Credential Manager. The account remembered for `to` becomes the one in `users.json` again
    /// when the Credential Manager still has its password, and the monitor signs it in; otherwise
    /// the launcher is signed out there.
    pub fn change_server(&self, env: &EnvRef, from: &str, to: &str, save: impl FnOnce() -> Res<()>) -> Res<()> {
        let _g = self.connect.lock().unwrap();
        self.refuse_while_playing()?;
        let left = stored(env);
        let kept = left.as_ref().is_some_and(|c| {
            let target = vault::target(from, &c.npid);
            env.vault().get(&target).is_some() || env.vault().set(&target, &c.npid, &c.password).is_ok()
        });
        edit_remembered(env, |a| match &left {
            Some(c) if kept => a.remember(from, &c.npid, &c.email),
            _ => a.forget(from),
        })?;
        save()?;
        if let Some(conn) = self.take_conn() {
            conn.close();
        }
        let back = accounts::load(&env.layout().accounts_json()).get(to).and_then(|r| {
            let password = env.vault().get(&vault::target(to, &r.online_id))?;
            let email = if r.email.is_empty() { registered_email(&r.online_id) } else { r.email.clone() };
            Some(Credentials { npid: r.online_id.clone(), password, email })
        });
        edit_users(env, |v| match &back {
            Some(c) => users::set_account(v, c),
            None => users::clear_account(v),
        })?;
        self.clear_friends(env);
        if left.is_some() {
            env.log(Msg::new("log.event.signedOut", "Network: signed out"));
        }
        if let Some(c) = &back {
            let server = crate::settings::server_name(to);
            env.log(
                Msg::new("log.event.accountRemembered", format!("Network: back on the {} with the account {}", server.message, c.npid))
                    .with("id", c.npid.clone())
                    .with("server", server),
            );
        }
        Ok(())
    }

    /// The community server moved from one of `former` to `current` with its accounts: what this
    /// PC keeps for the old address (the `accounts.json` entry, the Credential Manager's password)
    /// follows to the new one, and the account stays the one in `users.json` when the community
    /// server is in use. Nothing happens when the new address has an account of its own, or for a
    /// former address that is the private server of the settings. The password is written under
    /// the new name before the old entry goes, so a start that ends half-way is finished by the next.
    pub fn carry_over(&self, env: &EnvRef, current: &str, former: &[&str]) {
        let _g = self.connect.lock().unwrap();
        let settings = store::load_settings(&env.layout().settings_json());
        let in_use = settings.server() == current;
        let to = vault::server_key(current);
        for old in former {
            let from = vault::server_key(old);
            if from == to || (!settings.private_server.is_empty() && vault::server_key(&settings.private_server) == from) {
                continue;
            }
            let known = accounts::load(&env.layout().accounts_json());
            let active = if in_use { stored(env) } else { None };
            let candidates: Vec<(String, String)> = match &active {
                Some(c) => vec![(c.npid.clone(), c.email.clone())],
                None => [known.get(old), known.get(current)].into_iter().flatten().map(|r| (r.online_id.clone(), r.email.clone())).collect(),
            };
            let moving = candidates.into_iter().find_map(|(id, email)| Some((env.vault().get(&vault::target(old, &id))?, id, email)));
            let Some((password, id, email)) = moving else { continue };
            if known.get(current).is_some_and(|r| !r.online_id.eq_ignore_ascii_case(&id)) {
                continue;
            }
            let target = vault::target(current, &id);
            let written = match env.vault().get(&target) {
                Some(there) => there == password,
                None => env.vault().set(&target, &id, &password).is_ok() && env.vault().get(&target).as_ref() == Some(&password),
            };
            if !written {
                continue;
            }
            let remembered = edit_remembered(env, |a| {
                a.forget(old);
                a.remember(current, &id, &email);
            });
            let email = if email.is_empty() { registered_email(&id) } else { email };
            let c = Credentials { npid: id, password, email };
            if remembered.is_err() || (in_use && active.is_none() && edit_users(env, |v| users::set_account(v, &c)).is_err()) {
                continue;
            }
            env.vault().delete(&vault::target(old, &c.npid));
            env.log(
                Msg::new("log.event.accountCarriedOver", format!("Network: the community server has moved, the account {} moved with it", c.npid))
                    .with("id", c.npid),
            );
            return;
        }
    }

    /// The server answered that the stored account's Online ID or password is wrong: the account
    /// is gone there, or it was never this server's. Signed out and forgotten, so that the monitor
    /// does not try it again every round; the `account` event tells the UI.
    fn forget_refused(&self, env: &EnvRef, at: &str, npid: &str) {
        let _g = self.connect.lock().unwrap();
        if host(env) != at || stored(env).is_none_or(|c| c.npid != npid) || edit_users(env, users::clear_account).is_err() {
            return;
        }
        env.vault().delete(&vault::target(at, npid));
        let _ = edit_remembered(env, |a| a.forget(at));
        self.clear_friends(env);
        env.log(
            Msg::new("log.event.accountRefused", format!("Network: the server no longer accepts the account {npid}: signed out")).with("id", npid),
        );
        env.emit("account", serde_json::to_value(self.get_account(env)).unwrap_or(Value::Null));
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
    /// address. A failure does not block the launch, since a backend in `open` mode does not need
    /// the grant: it returns the warning for the player. Except when the launcher could not sign in
    /// because the account is in use elsewhere, which is an error: the game could not sign in either.
    pub fn play_grant(&self, env: &EnvRef) -> Res<Option<Msg>> {
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
                Ok(None)
            }
            Ok(Play::NotOffered) => {
                env.log(Msg::new("log.event.playGrantNotOffered", "Network: this server does not ask the launcher to vouch for the player"));
                Ok(None)
            }
            Err(e) if shadnet::in_use_elsewhere(&e) => Err(in_use_elsewhere()),
            Err(e) => {
                env.log(
                    Msg::new("log.event.playGrantFailed", format!("Network: the server did not confirm the account for this game: {}", e.message))
                        .with("reason", e.msg()),
                );
                Ok(Some(
                    Msg::new("backend.play_grant_failed", format!("The server did not confirm your account: {} The game starts anyway, but it may not sign in.", e.message))
                        .with("reason", e.msg()),
                ))
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
        if !status.network {
            self.set_in_use(env, false);
        }
        if !wanted || !status.network {
            return;
        }
        let tried = stored(env).map(|c| c.npid);
        if let Err(e) = self.ensure(env) {
            if e.key == "backend.login_wrong" {
                if let Some(npid) = tried {
                    self.forget_refused(env, &host, &npid);
                }
                return;
            }
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
    use crate::settings::Settings;
    use std::collections::{BTreeMap, HashMap};
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicBool, Ordering};

    /// The second field makes it refuse what it is asked to keep.
    #[derive(Default)]
    struct MemoryVault(Mutex<HashMap<String, String>>, AtomicBool);

    impl Vault for MemoryVault {
        fn get(&self, target: &str) -> Option<String> {
            self.0.lock().unwrap().get(target).cloned()
        }
        fn set(&self, target: &str, _: &str, secret: &str) -> Res<()> {
            if self.1.load(Ordering::Relaxed) {
                return Err(BackendError::internal("refused"));
            }
            self.0.lock().unwrap().insert(target.into(), secret.into());
            Ok(())
        }
        fn delete(&self, target: &str) {
            self.0.lock().unwrap().remove(target);
        }
    }

    struct TestEnv {
        layout: Layout,
        account: Account,
        vault: MemoryVault,
        events: Mutex<Vec<(String, Value)>>,
    }

    impl Env for TestEnv {
        fn layout(&self) -> &Layout {
            &self.layout
        }
        fn account(&self) -> &Account {
            &self.account
        }
        fn emit(&self, event: &str, payload: Value) {
            self.events.lock().unwrap().push((event.into(), payload));
        }
        fn log(&self, _: Msg) {}
        fn client_version(&self) -> String {
            "test".into()
        }
        fn vault(&self) -> &dyn Vault {
            &self.vault
        }
    }

    /// A data folder of its own, a Credential Manager in memory, no network.
    struct Pc {
        dir: PathBuf,
        test: Arc<TestEnv>,
        env: EnvRef,
    }

    impl Pc {
        fn new(name: &str) -> Pc {
            let dir = std::env::temp_dir().join(format!("fzh-account-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            let none = PathBuf::from("missing");
            let layout = Layout { data: dir.clone(), emulator: none.clone(), patches: none.clone(), licences: none.clone(), warmup_script: none };
            let test = Arc::new(TestEnv { layout, account: Account::default(), vault: MemoryVault::default(), events: Mutex::default() });
            Pc { dir, env: test.clone(), test }
        }

        /// As after a sign-in on `server`: `users.json` and the Credential Manager.
        fn signed_in(&self, server: &str, npid: &str, password: &str) {
            let c = Credentials { npid: npid.into(), password: password.into(), email: registered_email(npid) };
            edit_users(&self.env, |v| users::set_account(v, &c)).unwrap();
            self.test.vault.set(&vault::target(server, npid), npid, password).unwrap();
        }

        /// What `save_settings` does when the server changes.
        fn go(&self, from: &str, to: &str) {
            let settings = Settings { use_private_server: true, private_server: to.into(), ..Settings::default() };
            let path = self.test.layout.settings_json();
            self.test.account.change_server(&self.env, from, to, || store::save_settings(&path, &settings)).unwrap();
        }

        fn stored(&self) -> Option<(String, String)> {
            credentials(&self.env).map(|c| (c.npid, c.password))
        }

        /// The community server is now at `default_server()` and was at `former` before.
        fn moved(&self, former: &[&str]) {
            self.test.account.carry_over(&self.env, crate::settings::default_server(), former);
        }

        fn remembered(&self, server: &str) -> Option<String> {
            accounts::load(&self.test.layout.accounts_json()).get(server).map(|r| r.online_id.clone())
        }

        fn password(&self, server: &str, npid: &str) -> Option<String> {
            self.test.vault.get(&vault::target(server, npid))
        }

        /// Everything a move of the community server may touch.
        fn kept(&self) -> (BTreeMap<String, String>, String, Option<Credentials>) {
            let vault = self.test.vault.0.lock().unwrap().clone().into_iter().collect();
            (vault, std::fs::read_to_string(self.test.layout.accounts_json()).unwrap_or_default(), stored(&self.env))
        }
    }

    impl Drop for Pc {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    const A: &str = "a.example";
    const B: &str = "b.example:40001:40002:40003:40004:40005";

    fn account_of(id: &str, pw: &str) -> Option<(String, String)> {
        Some((id.into(), pw.into()))
    }

    #[test]
    fn each_server_keeps_its_account() {
        let pc = Pc::new("switch");
        pc.signed_in(A, "Synthetic_1", "pw-on-a");
        pc.go(A, B);
        assert_eq!(pc.stored(), None, "nothing remembered for B yet");
        assert_eq!(pc.test.vault.get(&vault::target(A, "Synthetic_1")).as_deref(), Some("pw-on-a"));
        pc.signed_in(B, "Other_2", "pw-on-b");
        pc.go(B, A);
        assert_eq!(pc.stored(), account_of("Synthetic_1", "pw-on-a"));
        assert_eq!(pc.test.account.get_account(&pc.env).session.unwrap().online_id, "Synthetic_1");
        pc.go(A, B);
        assert_eq!(pc.stored(), account_of("Other_2", "pw-on-b"));
        let text = std::fs::read_to_string(pc.test.layout.accounts_json()).unwrap();
        assert!(text.contains("Synthetic_1") && !text.contains("pw-on-"), "no password in accounts.json");
    }

    #[test]
    fn the_same_online_id_on_two_servers_has_two_passwords() {
        let pc = Pc::new("same-id");
        pc.signed_in(A, "Synthetic_1", "pw-on-a");
        pc.go(A, B);
        pc.signed_in(B, "Synthetic_1", "pw-on-b");
        pc.go(B, A);
        assert_eq!(pc.stored(), account_of("Synthetic_1", "pw-on-a"));
        pc.go(A, B);
        assert_eq!(pc.stored(), account_of("Synthetic_1", "pw-on-b"));
    }

    #[test]
    fn signing_out_forgets_that_server_only() {
        let pc = Pc::new("sign-out");
        pc.signed_in(A, "Synthetic_1", "pw-on-a");
        pc.go(A, B);
        pc.signed_in(B, "Other_2", "pw-on-b");
        pc.test.account.sign_out(&pc.env).unwrap();
        assert_eq!(pc.test.vault.get(&vault::target(B, "Other_2")), None);
        pc.go(B, A);
        assert_eq!(pc.stored(), account_of("Synthetic_1", "pw-on-a"));
        pc.test.account.sign_out(&pc.env).unwrap();
        pc.go(A, B);
        pc.go(B, A);
        assert_eq!(pc.stored(), None);
        assert!(pc.test.vault.0.lock().unwrap().is_empty());
    }

    #[test]
    fn an_account_from_before_the_credential_manager_is_kept_too() {
        let pc = Pc::new("legacy");
        let c = Credentials { npid: "Synthetic_1".into(), password: "pw-in-users-json".into(), email: "me@example.test".into() };
        edit_users(&pc.env, |v| users::set_account(v, &c)).unwrap();
        pc.go(A, B);
        pc.go(B, A);
        assert_eq!(stored(&pc.env), Some(c));
    }

    #[test]
    fn a_remembered_account_without_its_password_means_signed_out() {
        let pc = Pc::new("no-password");
        pc.signed_in(A, "Synthetic_1", "pw-on-a");
        pc.go(A, B);
        pc.test.vault.delete(&vault::target(A, "Synthetic_1"));
        pc.go(B, A);
        assert_eq!(pc.stored(), None);
        assert!(pc.test.account.get_account(&pc.env).session.is_none());
    }

    #[test]
    fn a_refused_account_is_signed_out_once() {
        let pc = Pc::new("refused");
        pc.go(A, B);
        pc.signed_in(B, "Synthetic_1", "pw-on-b");
        let account = &pc.test.account;
        account.forget_refused(&pc.env, B, "Other_2");
        account.forget_refused(&pc.env, A, "Synthetic_1");
        assert!(pc.stored().is_some(), "another account or another server: nothing happens");
        account.forget_refused(&pc.env, B, "Synthetic_1");
        assert_eq!(pc.stored(), None);
        assert_eq!(pc.test.vault.get(&vault::target(B, "Synthetic_1")), None);
        let events = pc.test.events.lock().unwrap();
        let (name, payload) = events.last().unwrap();
        assert_eq!((name.as_str(), &payload["session"]), ("account", &Value::Null));
        drop(events);
        pc.go(B, A);
        pc.go(A, B);
        assert_eq!(pc.stored(), None);
    }

    #[test]
    fn the_server_does_not_change_while_the_game_runs() {
        let pc = Pc::new("playing");
        pc.signed_in(A, "Synthetic_1", "pw-on-a");
        pc.test.account.suspend(&pc.env);
        let e = pc.test.account.change_server(&pc.env, A, B, || panic!("settings must not be written")).unwrap_err();
        assert_eq!(e.code, "game_running");
        assert_eq!(pc.stored(), account_of("Synthetic_1", "pw-on-a"));
    }

    const OLD: &str = "old.example:40001:40002:40003:40004:40005";

    #[test]
    fn the_account_follows_the_community_server_to_its_new_address() {
        let pc = Pc::new("moved");
        let new = crate::settings::default_server();
        pc.signed_in(OLD, "Synthetic_1", "pw");
        pc.moved(&[]);
        assert_eq!((pc.password(OLD, "Synthetic_1").as_deref(), pc.remembered(new)), (Some("pw"), None), "no former address: nothing moves");
        pc.moved(&["older.example", OLD]);
        assert_eq!(pc.stored(), account_of("Synthetic_1", "pw"));
        assert_eq!((pc.password(new, "Synthetic_1").as_deref(), pc.password(OLD, "Synthetic_1")), (Some("pw"), None));
        assert_eq!((pc.remembered(new).as_deref(), pc.remembered(OLD)), (Some("Synthetic_1"), None));
        let after = pc.kept();
        pc.moved(&["older.example", OLD]);
        assert_eq!(pc.kept(), after, "the second start changes nothing");
        pc.go(new, B);
        pc.go(B, new);
        assert_eq!(pc.stored(), account_of("Synthetic_1", "pw"));
    }

    #[test]
    fn an_account_of_the_new_address_stays_as_it_is() {
        let pc = Pc::new("moved-occupied");
        let new = crate::settings::default_server();
        pc.signed_in(OLD, "Synthetic_1", "pw-old");
        pc.signed_in(new, "Synthetic_1", "pw-new");
        let before = pc.kept();
        pc.moved(&[OLD]);
        assert_eq!(pc.kept(), before, "the same Online ID with another password");
        pc.go(OLD, B);
        pc.go(B, new);
        pc.signed_in(new, "Other_2", "pw-2");
        pc.go(new, B);
        let before = pc.kept();
        pc.moved(&[OLD]);
        assert_eq!(pc.kept(), before, "another account remembered for the new address");
        pc.go(B, new);
        pc.moved(&[OLD]);
        assert_eq!(pc.stored(), account_of("Other_2", "pw-2"));
        assert_eq!(pc.password(OLD, "Synthetic_1").as_deref(), Some("pw-old"));
    }

    #[test]
    fn a_move_that_stopped_half_way_is_finished_and_never_loses_the_password() {
        let pc = Pc::new("moved-half");
        let new = crate::settings::default_server();
        pc.signed_in(OLD, "Synthetic_1", "pw");
        pc.test.vault.1.store(true, Ordering::Relaxed);
        let before = pc.kept();
        pc.moved(&[OLD]);
        assert_eq!(pc.kept(), before, "the Credential Manager refuses the new entry: the old one stays");
        pc.test.vault.1.store(false, Ordering::Relaxed);
        pc.test.vault.set(&vault::target(new, "Synthetic_1"), "Synthetic_1", "pw").unwrap();
        pc.moved(&[OLD]);
        assert_eq!((pc.password(new, "Synthetic_1").as_deref(), pc.password(OLD, "Synthetic_1")), (Some("pw"), None));
        assert_eq!((pc.remembered(new).as_deref(), pc.stored()), (Some("Synthetic_1"), account_of("Synthetic_1", "pw")));

        let pc = Pc::new("moved-half-signed-out");
        pc.test.vault.set(&vault::target(OLD, "Synthetic_1"), "Synthetic_1", "pw").unwrap();
        pc.test.vault.set(&vault::target(new, "Synthetic_1"), "Synthetic_1", "pw").unwrap();
        edit_remembered(&pc.env, |a| a.remember(new, "Synthetic_1", "")).unwrap();
        pc.moved(&[OLD]);
        assert_eq!(pc.stored(), account_of("Synthetic_1", "pw"), "stopped before users.json");
        assert_eq!(pc.password(OLD, "Synthetic_1"), None);
    }

    #[test]
    fn on_a_private_server_the_community_account_moves_for_later() {
        let pc = Pc::new("moved-private");
        let new = crate::settings::default_server();
        pc.signed_in(OLD, "Synthetic_1", "pw-community");
        pc.go(OLD, B);
        pc.signed_in(B, "Synthetic_1", "pw-private");
        pc.moved(&[OLD]);
        assert_eq!(pc.stored(), account_of("Synthetic_1", "pw-private"), "the private server keeps its own account");
        assert_eq!((pc.remembered(new).as_deref(), pc.remembered(OLD)), (Some("Synthetic_1"), None));
        assert_eq!((pc.password(new, "Synthetic_1").as_deref(), pc.password(OLD, "Synthetic_1")), (Some("pw-community"), None));
        let after = pc.kept();
        pc.moved(&[OLD]);
        assert_eq!(pc.kept(), after);
        pc.go(B, new);
        assert_eq!(pc.stored(), account_of("Synthetic_1", "pw-community"));
    }

    #[test]
    fn a_private_server_at_a_former_address_is_not_the_community_server() {
        let pc = Pc::new("moved-not-private");
        let new = crate::settings::default_server();
        pc.go(A, OLD);
        pc.signed_in(OLD, "Synthetic_1", "pw");
        let before = pc.kept();
        pc.moved(&[OLD]);
        assert_eq!(pc.kept(), before, "in use as the private server");
        let off = Settings { private_server: OLD.into(), ..Settings::default() };
        let path = pc.test.layout.settings_json();
        pc.test.account.change_server(&pc.env, OLD, new, || store::save_settings(&path, &off)).unwrap();
        let before = pc.kept();
        pc.moved(&[OLD]);
        assert_eq!(pc.kept(), before, "remembered for the private server that is turned off");
        assert_eq!((pc.remembered(OLD).as_deref(), pc.stored()), (Some("Synthetic_1"), None));
    }

    /// A shadNet server on this PC that answers the next `Login` with `error`; its address.
    fn refusing_server(error: u8) -> String {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            let mut info = vec![packet::SERVER_INFO, 0, 0, 19, 0, 0, 0];
            info.extend([0u8; 8]);
            info.extend(packet::PROTOCOL_VERSION.to_le_bytes());
            s.write_all(&info).unwrap();
            let mut h = [0u8; packet::HEADER_SIZE];
            s.read_exact(&mut h).unwrap();
            let header = packet::Header::parse(&h);
            assert_eq!(header.command, packet::LOGIN);
            s.read_exact(&mut vec![0u8; header.payload_len().unwrap()]).unwrap();
            let mut reply = packet::request(packet::LOGIN, header.id, &[error]);
            reply[0] = packet::REPLY;
            s.write_all(&reply).unwrap();
        });
        format!("127.0.0.1:{port}:40002:40003:40004:40005")
    }

    #[test]
    fn an_account_in_use_elsewhere_stops_the_launch() {
        let pc = Pc::new("in-use");
        let server = refusing_server(6);
        pc.go(A, &server);
        pc.signed_in(&server, "Synthetic_1", "pw");
        let e = pc.test.account.play_grant(&pc.env).unwrap_err();
        assert_eq!((e.code, e.key), ("already_signed_in", "backend.account_in_use"));
        let events = pc.test.events.lock().unwrap();
        let (name, payload) = events.last().unwrap();
        assert_eq!((name.as_str(), &payload["inUseElsewhere"], &payload["session"]["onlineId"]), ("account", &Value::Bool(true), &Value::from("Synthetic_1")));
        drop(events);
        pc.test.account.sign_out(&pc.env).unwrap();
        assert!(!pc.test.account.get_account(&pc.env).in_use_elsewhere);
    }

    #[test]
    fn any_other_failure_of_the_play_grant_is_only_a_warning() {
        let pc = Pc::new("grant-warning");
        let banned = refusing_server(5);
        pc.go(A, &banned);
        pc.signed_in(&banned, "Synthetic_1", "pw");
        let warning = pc.test.account.play_grant(&pc.env).unwrap().unwrap();
        assert_eq!((warning.key, warning.params["reason"]["key"].as_str()), ("backend.play_grant_failed", Some("backend.login_banned")));
        let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        let unreachable = format!("127.0.0.1:{port}:40002:40003:40004:40005");
        pc.go(&banned, &unreachable);
        pc.signed_in(&unreachable, "Synthetic_1", "pw");
        let warning = pc.test.account.play_grant(&pc.env).unwrap().unwrap();
        assert_eq!(warning.params["reason"]["key"], "backend.net_private_unreachable");
        assert!(!pc.test.account.get_account(&pc.env).in_use_elsewhere);
    }

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
