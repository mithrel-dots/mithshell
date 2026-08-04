//! Lock screen backend: PAM authentication and the blurred backdrop.
//!
//! The UI half lives in [`crate::ui::lock`]. This module owns everything
//! that must not run on the GTK main thread.

pub mod backdrop;
mod pam;

use std::{
    ffi::CStr,
    path::Path,
    sync::OnceLock,
    thread::{self, JoinHandle},
};

use async_channel::Sender;
use log::{info, warn};

pub use pam::Outcome;

/// Written by an administrator who wants the lock screen to use a stack
/// other than `login` -- for example one that adds `pam_fprintd` but omits
/// the session modules `login` runs.
const MITHSHELL_SERVICE_FILE: &str = "/etc/pam.d/mithshell";

/// A password submitted from the lock screen.
///
/// `generation` lets the UI discard the answer to an attempt it has already
/// abandoned (for example because the session was torn down while a slow
/// PAM module was still deliberating).
pub struct AuthRequest {
    pub generation: u64,
    pub password: String,
}

/// A message from the authentication worker for a given attempt.
pub enum AuthEvent {
    /// A `PAM_TEXT_INFO`/`PAM_ERROR_MSG` message arriving mid-attempt, for
    /// example `pam_u2f`'s "Please touch the device". Zero or more of these
    /// precede the terminal `Result` for the same generation.
    Progress { generation: u64, message: String },
    /// PAM's final verdict on the attempt.
    Result { generation: u64, outcome: Outcome },
}

/// The PAM service the lock screen authenticates against.
///
/// Prefers `/etc/pam.d/mithshell` when the administrator has written one,
/// and otherwise inherits `login`, which every distribution ships and which
/// already has the right stack for "prove you are the console user" --
/// including `pam_faillock` rate limiting and whatever fingerprint or
/// smartcard modules are configured system-wide.
///
/// Resolved once and cached: swapping the file out from under a running
/// daemon should not silently change which stack a lock uses.
/// Resolved once per process and cached. The authentication stack a lock
/// screen uses is a security boundary; letting it change underneath a
/// running daemon (via `mithshell reload`, or by dropping a file into
/// `/etc/pam.d` while the shell is up) would be a way to weaken it without
/// restarting anything.
pub fn service_name(configured: Option<&str>) -> &'static str {
    static SERVICE: OnceLock<&'static str> = OnceLock::new();

    SERVICE.get_or_init(|| {
        if let Some(name) = configured {
            // An explicit override is honoured verbatim; validating it here
            // would only mask a typo until the user is already locked out.
            info!("lock screen will authenticate against the `{name}` PAM service (configured)");
            return Box::leak(name.to_owned().into_boxed_str());
        }
        if Path::new(MITHSHELL_SERVICE_FILE).exists() {
            info!("lock screen will authenticate against the `mithshell` PAM service");
            "mithshell"
        } else {
            info!("no {MITHSHELL_SERVICE_FILE}; lock screen inherits the `login` PAM service");
            "login"
        }
    })
}

/// The name of the user who owns this session.
///
/// Read from the passwd database rather than `$USER`, which is inherited
/// from the environment and therefore attacker-controllable in a way that
/// would let someone point the lock screen at a different account.
pub fn current_user() -> String {
    static USER: OnceLock<String> = OnceLock::new();

    USER.get_or_init(|| {
        // SAFETY: `getuid` is always successful, and `getpwuid` returns
        // either null or a pointer to a static entry valid until the next
        // passwd call. We copy the name out immediately.
        let name = unsafe {
            let entry = libc::getpwuid(libc::getuid());
            if entry.is_null() || (*entry).pw_name.is_null() {
                None
            } else {
                Some(
                    CStr::from_ptr((*entry).pw_name)
                        .to_string_lossy()
                        .into_owned(),
                )
            }
        };
        name.unwrap_or_else(|| {
            warn!("no passwd entry for the current uid; falling back to $USER");
            std::env::var("USER").unwrap_or_else(|_| "unknown".to_owned())
        })
    })
    .clone()
}

/// Spawns the authentication worker.
///
/// One long-lived thread rather than a thread per attempt: PAM modules keep
/// per-process state (`pam_faillock` tally files, PKCS#11 sessions) and are
/// happier being driven serially. Serialising also means a user mashing
/// Enter cannot spawn a pile of threads each sleeping out its own
/// `FAIL_DELAY`.
pub fn start_authenticator(
    service: &'static str,
    events: Sender<AuthEvent>,
) -> (Sender<AuthRequest>, JoinHandle<()>) {
    let (sender, receiver) = async_channel::unbounded::<AuthRequest>();
    let handle = thread::spawn(move || {
        let user = current_user();
        while let Ok(request) = receiver.recv_blocking() {
            let mut password = request.password;
            let generation = request.generation;
            let progress_events = events.clone();
            let outcome = pam::authenticate(service, &user, &password, &mut |message| {
                let _ = progress_events.send_blocking(AuthEvent::Progress {
                    generation,
                    message,
                });
            });
            password.clear();
            if events
                .send_blocking(AuthEvent::Result {
                    generation,
                    outcome,
                })
                .is_err()
            {
                break;
            }
        }
    });
    (sender, handle)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `service_name` caches in a process-wide `OnceLock`, so the override
    /// and detection paths cannot be asserted independently from the same
    /// test binary. Assert the invariant that holds either way.
    #[test]
    fn resolves_a_non_empty_pam_service() {
        let service = service_name(None);
        assert!(!service.is_empty());
        assert_eq!(service_name(Some("sudo")), service, "resolution is cached");
    }

    #[test]
    fn current_user_is_not_empty() {
        assert!(!current_user().is_empty());
    }
}
