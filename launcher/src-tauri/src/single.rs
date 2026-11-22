//! One launcher per data folder. Two launchers on the same folder would share one account, which
//! shadNet lets only one of them sign in with, and write the same files; launchers on different
//! folders (`FZH_DATA_DIR`) run side by side.
//!
//! Both kernel objects are named after the canonical path of the data folder, in the session's
//! namespace: a mutex the running launcher owns until its process ends, and an auto-reset event a
//! later start sets to bring that launcher's window to the front. Nothing is left on disk, and a
//! launcher that crashed or was killed leaves the mutex abandoned, which the next start takes.

use sha2::{Digest, Sha256};
use std::path::Path;
use std::time::Duration;

/// How long a start waits for the launcher that holds the folder before it gives way: that one may
/// be ending, as when the launcher restarts itself. The start has no window yet, so the wait shows
/// nowhere.
pub const HANDOVER: Duration = Duration::from_secs(3);

/// Names the data folder however it was spelled: letter case, `..`, a trailing separator, a junction.
pub fn key(data: &Path) -> String {
    let _ = std::fs::create_dir_all(data);
    let path = std::fs::canonicalize(data).or_else(|_| std::path::absolute(data)).unwrap_or_else(|_| data.to_path_buf());
    let hash = Sha256::digest(path.to_string_lossy().to_lowercase().as_bytes());
    hash[..12].iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(windows)]
mod windows {
    use super::*;
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0, WAIT_TIMEOUT};
    use windows_sys::Win32::System::Threading::{CreateEventW, CreateMutexW, ReleaseMutex, SetEvent, WaitForSingleObject, INFINITE};
    use windows_sys::Win32::UI::WindowsAndMessaging::{AllowSetForegroundWindow, ASFW_ANY};

    struct Handle(HANDLE);

    // SAFETY: a kernel handle is usable from any thread.
    unsafe impl Send for Handle {}

    impl Handle {
        fn wait(&self, ms: u32) -> u32 {
            // SAFETY: the handle is open for as long as `self` lives; a null one makes the call fail.
            unsafe { WaitForSingleObject(self.0, ms) }
        }
    }

    impl Drop for Handle {
        fn drop(&mut self) {
            // SAFETY: the handle is ours and is closed once.
            unsafe { CloseHandle(self.0) };
        }
    }

    fn name(kind: &str, key: &str) -> Vec<u16> {
        format!("Local\\FZH-Resurrection-{kind}-{key}").encode_utf16().chain(Some(0)).collect()
    }

    fn event(name: &[u16]) -> Handle {
        // SAFETY: `name` is NUL-terminated; an existing event of that name is opened.
        Handle(unsafe { CreateEventW(std::ptr::null(), 0, 0, name.as_ptr()) })
    }

    /// The data folder is this launcher's while this lives. Windows ties the mutex to the thread
    /// that claimed it: keep this on that thread, for as long as the launcher runs.
    pub struct Instance {
        lock: Handle,
        wake: Vec<u16>,
        _event: Handle,
    }

    impl Drop for Instance {
        fn drop(&mut self) {
            // SAFETY: the handle is valid; the call fails harmlessly on another thread.
            unsafe { ReleaseMutex(self.lock.0) };
        }
    }

    /// The other half of an `Instance`, free to move to where the window is known.
    pub struct Wake(Handle);

    impl Instance {
        pub fn wake(&self) -> Wake {
            Wake(event(&self.wake))
        }
    }

    impl Wake {
        /// Calls `f` every time another start asked for this launcher, also for a request that came
        /// before this call.
        pub fn listen(self, f: impl Fn() + Send + 'static) {
            let event = self.0;
            std::thread::spawn(move || {
                while event.wait(INFINITE) == WAIT_OBJECT_0 {
                    f();
                }
            });
        }
    }

    /// Takes the data folder for this process, or asks the launcher that has it to show itself and
    /// returns None once `wait` passed without that launcher ending. When Windows refuses the
    /// objects the launcher starts without the guarantee rather than not at all.
    pub fn claim(data: &Path, wait: Duration) -> Option<Instance> {
        let key = key(data);
        let wake = name("wake", &key);
        let event = event(&wake);
        // SAFETY: the name is NUL-terminated.
        let lock = Handle(unsafe { CreateMutexW(std::ptr::null(), 0, name("lock", &key).as_ptr()) });
        if lock.wait(0) == WAIT_TIMEOUT {
            // SAFETY: the event handle is open; the other launcher may then take the foreground.
            unsafe {
                AllowSetForegroundWindow(ASFW_ANY);
                SetEvent(event.0);
            }
            if lock.wait(wait.as_millis() as u32) == WAIT_TIMEOUT {
                return None;
            }
        }
        Some(Instance { lock, wake, _event: event })
    }
}

#[cfg(windows)]
pub use windows::claim;

#[cfg(not(windows))]
mod other {
    use super::*;

    pub struct Instance;
    pub struct Wake;

    impl Instance {
        pub fn wake(&self) -> Wake {
            Wake
        }
    }

    impl Wake {
        pub fn listen(self, _: impl Fn() + Send + 'static) {}
    }

    pub fn claim(_: &Path, _: Duration) -> Option<Instance> {
        Some(Instance)
    }
}

#[cfg(not(windows))]
pub use other::claim;

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::mpsc;

    const SOON: Duration = Duration::from_millis(50);
    const LONG: Duration = Duration::from_secs(10);

    fn folder(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("fzh-single-{name}-{}", std::process::id()))
    }

    /// A start in a process of its own, as far as the mutex goes: another thread.
    fn start(data: &Path, wait: Duration) -> bool {
        let data = data.to_path_buf();
        std::thread::spawn(move || claim(&data, wait).is_some()).join().unwrap()
    }

    #[test]
    fn the_key_is_the_folder_however_it_is_spelled() {
        let dir = folder("key");
        let spelled = dir.join("sub").join("..").to_string_lossy().to_uppercase() + "\\";
        assert_eq!(key(&dir), key(Path::new(&spelled)));
        assert_ne!(key(&dir), key(&dir.join("sub")));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_second_start_wakes_the_first_and_gives_way() {
        let dir = folder("second");
        let first = claim(&dir, SOON).expect("the folder is free");
        assert!(!start(&dir, SOON));
        let (tx, rx) = mpsc::channel();
        first.wake().listen(move || tx.send(()).unwrap());
        rx.recv_timeout(LONG).expect("the request made before listening is kept");
        assert!(!start(&dir, SOON));
        rx.recv_timeout(LONG).expect("woken again");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn other_folders_are_free() {
        let (a, b) = (folder("a"), folder("b"));
        let _first = claim(&a, SOON).unwrap();
        assert!(start(&b, SOON));
        let _ = (std::fs::remove_dir_all(&a), std::fs::remove_dir_all(&b));
    }

    #[test]
    fn a_launcher_that_died_blocks_nothing() {
        let dir = folder("died");
        let held = dir.clone();
        // The thread ends owning the mutex, as a killed process does.
        std::thread::spawn(move || std::mem::forget(claim(&held, SOON).unwrap())).join().unwrap();
        assert!(start(&dir, SOON));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_start_waits_for_a_launcher_that_is_ending() {
        let dir = folder("ending");
        let held = dir.clone();
        let (tx, rx) = mpsc::channel();
        let old = std::thread::spawn(move || {
            let instance = claim(&held, SOON).unwrap();
            tx.send(()).unwrap();
            std::thread::sleep(Duration::from_millis(300));
            drop(instance);
        });
        rx.recv().unwrap();
        assert!(start(&dir, LONG));
        old.join().unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }
}
