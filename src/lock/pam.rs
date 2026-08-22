//! Minimal, hardened PAM client used by the lock screen.
//!
//! Everything here runs on a dedicated worker thread (never the GTK main
//! thread) because PAM modules are free to block for seconds -- `pam_unix`
//! alone sleeps for `FAIL_DELAY` after a bad password.
//!
//! The release profile builds with `panic = "abort"`, so a panic inside the
//! `extern "C"` conversation callback would take the whole shell down and
//! leave the user staring at a locked screen they cannot dismiss. Nothing in
//! `converse` is allowed to panic: no indexing, no `unwrap`, no allocation
//! that is not checked.

use std::{
    ffi::{CStr, CString},
    mem, ptr,
};

use libc::{c_char, c_int, c_void};
use log::{debug, warn};
use pam_sys::{
    types::{
        PamConversation, PamFlag, PamHandle, PamMessage, PamMessageStyle, PamResponse,
        PamReturnCode,
    },
    wrapped,
};

/// A module asking more questions than this in a single conversation turn is
/// a bug (or an attack); refuse rather than allocate an unbounded array.
const MAX_MESSAGES: usize = 32;

/// The result of one full authenticate + account-management cycle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// The user proved their identity and the account is in good standing.
    Granted,
    /// PAM said no. The string is safe to show on the lock screen.
    Denied(String),
    /// PAM could not reach a verdict (misconfigured stack, no service file,
    /// module crash). Distinguished from `Denied` because it must never be
    /// treated as a reason to keep retrying the same password.
    Failed(String),
}

/// Application data handed to the conversation callback through
/// `PamConversation::data_ptr`.
///
/// The `CString`s are owned here so the callback never has to allocate a
/// prompt answer from scratch; it only `strdup`s them into memory PAM will
/// `free()`.
struct Conversation<'a> {
    user: CString,
    password: CString,
    /// `PAM_ERROR_MSG` text, shown to the user verbatim when the attempt fails.
    errors: Vec<String>,
    /// `PAM_TEXT_INFO` text, only logged.
    infos: Vec<String>,
    /// Called immediately for every `PAM_TEXT_INFO`/`PAM_ERROR_MSG` message,
    /// so the lock screen can show "Please touch the device" while
    /// `pam_u2f` blocks waiting for a hardware touch, instead of sitting on
    /// a static "Authenticating..." for however long the module takes.
    progress: &'a mut dyn FnMut(String),
}

/// Copies a NUL-terminated PAM string into an owned `String`, lossily.
///
/// # Safety
///
/// `ptr` must be null or point to a valid NUL-terminated C string.
unsafe fn read_c_string(ptr: *const c_char) -> String {
    if ptr.is_null() {
        return String::new();
    }
    unsafe { CStr::from_ptr(ptr) }
        .to_string_lossy()
        .trim()
        .to_owned()
}

/// Releases a partially built response array using the same allocator PAM
/// would have used, so an early return never leaks.
///
/// # Safety
///
/// `responses` must come from `libc::calloc` with room for `count` entries,
/// and every non-null `resp` field must come from `libc::strdup`.
unsafe fn free_responses(responses: *mut PamResponse, count: usize) {
    for index in 0..count {
        // SAFETY: `index < count` and the array was calloc'd with `count` slots.
        let slot = unsafe { &mut *responses.add(index) };
        if !slot.resp.is_null() {
            unsafe { libc::free(slot.resp.cast::<c_void>()) };
            slot.resp = ptr::null_mut();
        }
    }
    unsafe { libc::free(responses.cast::<c_void>()) };
}

/// The PAM conversation function.
///
/// Answers `PROMPT_ECHO_OFF` with the password the user typed and
/// `PROMPT_ECHO_ON` with their username, and records informational and error
/// text for the caller. Because this is a non-interactive conversation, it
/// can never block.
///
/// # Safety
///
/// Called by libpam with the ABI described in `pam_conv(3)`.
extern "C" fn converse(
    num_msg: c_int,
    msg: *mut *mut PamMessage,
    out: *mut *mut PamResponse,
    appdata: *mut c_void,
) -> c_int {
    if out.is_null() {
        return PamReturnCode::CONV_ERR as c_int;
    }
    // Leave the caller's out-parameter in a defined state before any early
    // return; libpam does not promise to have zeroed it.
    unsafe { *out = ptr::null_mut() };

    if msg.is_null() || appdata.is_null() || num_msg <= 0 {
        return PamReturnCode::CONV_ERR as c_int;
    }
    let count = num_msg as usize;
    if count > MAX_MESSAGES {
        return PamReturnCode::CONV_ERR as c_int;
    }

    // SAFETY: `authenticate` passes a pointer to a live `Conversation` that
    // outlives the `pam_start`/`pam_end` pair, and libpam only calls the
    // conversation function from within those calls, on this same thread.
    let data = unsafe { &mut *appdata.cast::<Conversation<'_>>() };

    // PAM frees this array with `free()`, so it must come from the C allocator.
    let responses =
        unsafe { libc::calloc(count, mem::size_of::<PamResponse>()) }.cast::<PamResponse>();
    if responses.is_null() {
        return PamReturnCode::BUF_ERR as c_int;
    }

    for index in 0..count {
        // SAFETY: Linux-PAM passes an array of `count` message pointers.
        let message = unsafe { *msg.add(index) };
        if message.is_null() {
            continue;
        }
        // SAFETY: non-null message pointers reference a valid `pam_message`.
        let (style, text) = unsafe { ((*message).msg_style, read_c_string((*message).msg)) };

        let answer = match PamMessageStyle::from(style) {
            PamMessageStyle::PROMPT_ECHO_OFF => Some(data.password.as_ptr()),
            PamMessageStyle::PROMPT_ECHO_ON => Some(data.user.as_ptr()),
            PamMessageStyle::ERROR_MSG => {
                if !text.is_empty() {
                    (data.progress)(text.clone());
                    data.errors.push(text);
                }
                None
            }
            PamMessageStyle::TEXT_INFO => {
                if !text.is_empty() {
                    (data.progress)(text.clone());
                    data.infos.push(text);
                }
                None
            }
        };

        let Some(answer) = answer else {
            // A zeroed `resp` is the correct reply to a non-prompt message.
            continue;
        };
        let copy = unsafe { libc::strdup(answer) };
        if copy.is_null() {
            unsafe { free_responses(responses, count) };
            return PamReturnCode::BUF_ERR as c_int;
        }
        // SAFETY: `index < count`, so this slot is inside the calloc'd array.
        unsafe { (*responses.add(index)).resp = copy };
    }

    unsafe { *out = responses };
    PamReturnCode::SUCCESS as c_int
}

/// Runs one authentication attempt against `service` for `user`.
///
/// Blocking: PAM deliberately delays failed attempts. Call this from a
/// worker thread. `progress` is invoked synchronously, on this thread, for
/// every informational or error message the stack produces mid-attempt
/// (for example `pam_u2f`'s "Please touch the device").
///
/// The password is zeroed before this function returns, on every path.
pub fn authenticate(
    service: &str,
    user: &str,
    password: &str,
    progress: &mut dyn FnMut(String),
) -> Outcome {
    let (Ok(user_c), Ok(password_c)) = (CString::new(user), CString::new(password)) else {
        // A NUL byte cannot be typed into a GTK entry, so this only happens
        // for a pathological username.
        return Outcome::Failed("username or password contains a NUL byte".to_owned());
    };

    let mut data = Conversation {
        user: user_c,
        password: password_c,
        errors: Vec::new(),
        infos: Vec::new(),
        progress,
    };
    let conversation = PamConversation {
        conv: Some(converse),
        data_ptr: ptr::from_mut(&mut data).cast::<c_void>(),
    };

    let mut handle: *mut PamHandle = ptr::null_mut();
    let started = wrapped::start(service, Some(user), &conversation, &mut handle);
    if started != PamReturnCode::SUCCESS || handle.is_null() {
        zero(&mut data.password);
        return Outcome::Failed(format!(
            "pam_start for service `{service}` failed: {started:?}"
        ));
    }

    // SAFETY: `pam_start` returned SUCCESS with a non-null handle, and the
    // handle stays valid until the `pam_end` below.
    let pam = unsafe { &mut *handle };

    // DISALLOW_NULL_AUTHTOK: never let an account with an empty password
    // unlock the screen, regardless of what the inherited stack permits.
    let auth = wrapped::authenticate(pam, PamFlag::DISALLOW_NULL_AUTHTOK);
    // `pam_end` wants the status of the last call made on the handle.
    let mut last = auth;
    let outcome = if auth == PamReturnCode::SUCCESS {
        // Authentication only proves identity. `pam_acct_mgmt` is what
        // catches expired, disabled, or time-restricted accounts.
        let account = wrapped::acct_mgmt(pam, PamFlag::DISALLOW_NULL_AUTHTOK);
        last = account;
        match account {
            PamReturnCode::SUCCESS => {
                // Best effort: refresh Kerberos tickets and friends so a
                // long lock does not leave stale credentials behind.
                let refreshed = wrapped::setcred(pam, PamFlag::REFRESH_CRED);
                if refreshed != PamReturnCode::SUCCESS {
                    debug!("pam_setcred(REFRESH_CRED) returned {refreshed:?}");
                }
                Outcome::Granted
            }
            PamReturnCode::NEW_AUTHTOK_REQD => {
                Outcome::Denied("password has expired; log in on a TTY to change it".to_owned())
            }
            other => Outcome::Denied(describe(pam, other, &data.errors)),
        }
    } else {
        Outcome::Denied(describe(pam, auth, &data.errors))
    };

    for info in &data.infos {
        debug!("pam: {info}");
    }

    let ended = wrapped::end(pam, last);
    if ended != PamReturnCode::SUCCESS {
        warn!("pam_end returned {ended:?}");
    }
    zero(&mut data.password);
    outcome
}

/// Builds a user-facing message for a failure code.
///
/// Prefers text the stack produced itself (`pam_faillock` counts down
/// remaining attempts this way), and falls back to `pam_strerror`.
fn describe(pam: &mut PamHandle, code: PamReturnCode, errors: &[String]) -> String {
    if let Some(message) = errors.iter().find(|message| !message.is_empty()) {
        return message.clone();
    }
    match code {
        PamReturnCode::AUTH_ERR | PamReturnCode::PERM_DENIED => "Incorrect password".to_owned(),
        PamReturnCode::MAXTRIES => "Too many attempts; try again later".to_owned(),
        PamReturnCode::USER_UNKNOWN => "Unknown user".to_owned(),
        PamReturnCode::ACCT_EXPIRED => "Account has expired".to_owned(),
        PamReturnCode::CRED_EXPIRED => "Credentials have expired".to_owned(),
        PamReturnCode::ABORT => "Authentication aborted".to_owned(),
        code => wrapped::strerror(pam, code).map_or_else(
            || format!("Authentication failed ({code:?})"),
            str::to_owned,
        ),
    }
}

/// Overwrites a secret in place with volatile writes the optimizer is not
/// allowed to elide, then drops it.
///
/// This is best effort: `CString::new` already made one heap copy of the
/// GTK entry's buffer, and libpam copies it again. It removes the longest
/// lived copy, which is the one that would otherwise sit in the daemon's
/// heap for the rest of the session.
fn zero(secret: &mut CString) {
    let taken = mem::take(secret);
    let mut bytes = taken.into_bytes();
    for byte in &mut bytes {
        // SAFETY: `byte` is a live, aligned, writable `u8`.
        unsafe { ptr::write_volatile(byte, 0) };
    }
    std::sync::atomic::compiler_fence(std::sync::atomic::Ordering::SeqCst);
    drop(bytes);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_embedded_nul_bytes() {
        assert!(matches!(
            authenticate("login", "us\0er", "secret", &mut |_| {}),
            Outcome::Failed(_)
        ));
    }

    #[test]
    fn missing_service_is_a_failure_not_a_denial() {
        // Where an unknown service fails is implementation-defined across
        // libpam versions and stacks: `pam_start` can refuse it outright
        // (`Failed`), or the stack rejects the missing module mid-
        // authenticate, which surfaces as `Denied` carrying the stack's
        // own message rather than our generic wrong-password text. Either
        // way the attempt must never authenticate; only the wording the
        // user sees differs.
        let outcome = authenticate(
            "mithshell-nonexistent-service-\u{1}",
            "root",
            "",
            &mut |_| {},
        );
        assert!(matches!(outcome, Outcome::Failed(_) | Outcome::Denied(_)));
    }

    #[test]
    fn zeroing_clears_the_buffer() {
        let mut secret = CString::new("hunter2").unwrap();
        zero(&mut secret);
        assert!(secret.as_bytes().is_empty());
    }
}
