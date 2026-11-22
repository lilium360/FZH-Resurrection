//! The launcher's own copy of the account password: a generic credential in the Windows Credential
//! Manager, one per server and account. The emulator reads its copy from `users.json` (`users.rs`).

use crate::error::{BackendError, Res};
use crate::instance::Server;

pub trait Vault: Send + Sync {
    fn get(&self, target: &str) -> Option<String>;
    fn set(&self, target: &str, user: &str, secret: &str) -> Res<()>;
    fn delete(&self, target: &str);
}

/// The credential's name, as shown in the Credential Manager. Names are unique ignoring case on
/// shadNet, so the Online ID is lower-cased.
pub fn target(server: &str, online_id: &str) -> String {
    let at = Server::of(server);
    format!("FZH Resurrection/{}:{}/{}", at.host.to_ascii_lowercase(), at.game, online_id.to_ascii_lowercase())
}

pub struct System;

#[cfg(windows)]
mod windows {
    use super::*;
    use std::ffi::c_void;
    use windows_sys::Win32::Foundation::GetLastError;
    use windows_sys::Win32::Security::Credentials::{
        CredDeleteW, CredFree, CredReadW, CredWriteW, CREDENTIALW, CRED_PERSIST_LOCAL_MACHINE, CRED_TYPE_GENERIC,
    };

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(Some(0)).collect()
    }

    impl Vault for System {
        fn get(&self, target: &str) -> Option<String> {
            let name = wide(target);
            let mut cred: *mut CREDENTIALW = std::ptr::null_mut();
            // SAFETY: `name` is NUL-terminated; on success `cred` points to a block owned by the
            // system until CredFree.
            unsafe {
                if CredReadW(name.as_ptr(), CRED_TYPE_GENERIC, 0, &mut cred) == 0 {
                    return None;
                }
                let c = &*cred;
                let blob = if c.CredentialBlob.is_null() {
                    Vec::new()
                } else {
                    std::slice::from_raw_parts(c.CredentialBlob, c.CredentialBlobSize as usize).to_vec()
                };
                CredFree(cred as *const c_void);
                String::from_utf8(blob).ok().filter(|s| !s.is_empty())
            }
        }

        fn set(&self, target: &str, user: &str, secret: &str) -> Res<()> {
            let mut name = wide(target);
            let mut user = wide(user);
            let mut blob = secret.as_bytes().to_vec();
            // SAFETY: every pointer stays valid for the call; CredWriteW copies what it keeps.
            let ok = unsafe {
                let mut c: CREDENTIALW = std::mem::zeroed();
                c.Type = CRED_TYPE_GENERIC;
                c.TargetName = name.as_mut_ptr();
                c.UserName = user.as_mut_ptr();
                c.CredentialBlob = blob.as_mut_ptr();
                c.CredentialBlobSize = blob.len() as u32;
                c.Persist = CRED_PERSIST_LOCAL_MACHINE;
                CredWriteW(&c, 0) != 0
            };
            if ok {
                return Ok(());
            }
            let code = unsafe { GetLastError() };
            Err(BackendError::new("io", "backend.vault_failed", format!("Windows could not keep the account in the Credential Manager (error {code}).")).with("code", code))
        }

        fn delete(&self, target: &str) {
            let name = wide(target);
            // SAFETY: `name` is NUL-terminated.
            unsafe { CredDeleteW(name.as_ptr(), CRED_TYPE_GENERIC, 0) };
        }
    }
}

#[cfg(not(windows))]
impl Vault for System {
    fn get(&self, _: &str) -> Option<String> {
        None
    }
    fn set(&self, _: &str, _: &str, _: &str) -> Res<()> {
        Err(BackendError::unsupported("backend.vault_failed", "There is no Credential Manager on this system."))
    }
    fn delete(&self, _: &str) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_credential_per_server_and_account() {
        assert_eq!(target("127.0.0.1", "Player_One"), "FZH Resurrection/127.0.0.1:31313/player_one");
        assert_eq!(target("Example.Test", "a-b"), target("example.test", "A-B"));
        assert_ne!(target("10.0.0.1", "x_y"), target("10.0.0.2", "x_y"));
    }
}
