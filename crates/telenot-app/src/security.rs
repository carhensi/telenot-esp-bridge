//! Security ports: secrets (PIN/MQTT password) go **write-only** in, never come back out.
//! Token generation + clock. The host provides [`InMemoryServices`]; the firmware implements
//! the same trait against encrypted NVS + Argon2 + hardware RNG.
//!
//! This module also contains the **consuming** security logic that applies to both worlds:
//! constant-time comparison, PIN verification + lockout, login brute-force throttle, and
//! the fail-closed disarm authorisation ([`disarm_authorized`]). The firmware replaces only
//! the storage (NVS/Argon2/HW-RNG, persistent lockout), not this decision logic.

use std::time::Instant;

/// Login: locked for [`LOGIN_LOCK_MS`] after this many failed attempts.
const LOGIN_MAX_FAILS: u32 = 5;
const LOGIN_LOCK_MS: u64 = 60_000;
/// Disarm PIN: stricter because it disarms the panel.
const PIN_MAX_FAILS: u32 = 5;
const PIN_LOCK_MS: u64 = 300_000;

/// I/O ports of the service layer provided by the daemon/firmware. Pure API handlers
/// communicate with the "outside world" exclusively through this trait.
pub trait Services: Send {
    /// Monotonic time in ms (for session/scan/diagnostics).
    fn now_ms(&self) -> u64;
    /// Generate a fresh session/CSRF token.
    fn new_token(&mut self) -> String;

    /// Verify the login password (initial from the label, then the one set by the user).
    /// Pure comparison without lockout — callers wrap this with [`Self::login_locked`]/
    /// [`Self::note_login_fail`]/[`Self::note_login_ok`].
    fn verify_login(&self, password: &str) -> bool;
    /// Set a new login password (fulfils the first-boot requirement).
    fn set_password(&mut self, new: &str);
    /// Is a custom password still required to be set on first start?
    fn password_change_required(&self) -> bool;

    /// Seconds until login is allowed again if currently locked (brute-force throttle).
    fn login_locked(&self) -> Option<u64>;
    /// Record a failed login attempt (locks after the threshold).
    fn note_login_fail(&mut self);
    /// Successful login → reset the counter/lockout.
    fn note_login_ok(&mut self);

    /// Set the disarm PIN (stored hashed; never readable back).
    fn set_pin(&mut self, pin: &str);
    fn pin_set(&self) -> bool;
    /// Verify the disarm PIN **in constant time** against the stored value and maintain a
    /// separate lockout counter. This is the single PIN check — the disarm gate calls ONLY
    /// this, never a plaintext comparison.
    fn verify_pin(&mut self, pin: &str) -> PinCheck;

    /// Set the MQTT broker password (write-only towards the client).
    fn set_mqtt_password(&mut self, pw: &str);
    fn mqtt_password_set(&self) -> bool;
    /// Internal read of the MQTT password ONLY for connecting/testing — never included in an
    /// HTTP response (the device must know its own broker password to connect).
    fn mqtt_password(&self) -> Option<String>;
}

/// Result of a PIN check. `Locked` carries the remaining lockout time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PinCheck {
    Ok,
    Wrong,
    /// No PIN is (yet) set → fail-closed.
    NoPin,
    Locked {
        retry_after_s: u64,
    },
}

/// Fail-closed disarm authorisation: only disarm if remote disarm is explicitly enabled
/// **and** the PIN was correct. Single source of truth for the host gate and firmware.
pub fn disarm_authorized(remote_disarm: bool, pin: PinCheck) -> bool {
    remote_disarm && matches!(pin, PinCheck::Ok)
}

/// Remote disarm is only effective when explicitly enabled AND a PIN is set.
pub fn disarm_enabled(remote_disarm: bool, pin_set: bool) -> bool {
    remote_disarm && pin_set
}

/// Constant-time byte comparison (prevents timing oracles for PIN/password/token).
/// Length may leak (standard); content must not.
pub fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

/// Weak PIN (e.g. `0000`, `1234`, all same digits, ascending/descending)? Allowed but warned.
pub fn is_weak_pin(pin: &str) -> bool {
    let digits: Vec<u8> = pin.bytes().filter(u8::is_ascii_digit).collect();
    if digits.len() < 4 {
        return true;
    }
    let all_same = digits.iter().all(|&d| d == digits[0]);
    let ascending = digits.windows(2).all(|w| w[1] == w[0] + 1);
    let descending = digits.windows(2).all(|w| w[0] == w[1] + 1);
    all_same || ascending || descending
}

/// Is the PIN format valid (numeric, ≥4 digits)?
pub fn pin_format_ok(pin: &str) -> bool {
    pin.len() >= 4 && pin.bytes().all(|b| b.is_ascii_digit())
}

/// Failed-attempt throttle: counts failures and locks above the threshold for a time window.
/// On the host this is in-memory; the firmware mirrors the same state in NVS (survives reboot).
#[derive(Debug, Default, Clone, Copy)]
struct Lockout {
    fails: u32,
    locked_until_ms: u64,
}

impl Lockout {
    /// Remaining lockout time in seconds, if currently locked.
    fn locked_for(&self, now_ms: u64) -> Option<u64> {
        (self.locked_until_ms > now_ms).then(|| (self.locked_until_ms - now_ms).div_ceil(1000))
    }
    /// Record a failure; lock for `window_ms` once `max` is reached and reset the counter
    /// for the next window.
    fn record_fail(&mut self, now_ms: u64, max: u32, window_ms: u64) {
        self.fails += 1;
        if self.fails >= max {
            self.locked_until_ms = now_ms + window_ms;
            self.fails = 0;
        }
    }
    fn reset(&mut self) {
        self.fails = 0;
        self.locked_until_ms = 0;
    }
}

/// Host implementation of [`Services`]: in-memory, plaintext in process memory (loopback dev).
/// The security INVARIANT (write-only, never in a response, fail-closed, constant-time,
/// lockout) is enforced in the service layer; the firmware replaces the storage with
/// NVS+Argon2 (and persists the lockout).
pub struct InMemoryServices {
    start: Instant,
    token_counter: u64,
    login_password: String,
    password_changed: bool,
    pin: Option<String>,
    mqtt_password: Option<String>,
    login_lock: Lockout,
    pin_lock: Lockout,
}

impl InMemoryServices {
    /// `initial_password` = the device initial password (label / dev default).
    pub fn new(initial_password: impl Into<String>) -> Self {
        InMemoryServices {
            start: Instant::now(),
            token_counter: 0,
            login_password: initial_password.into(),
            password_changed: false,
            pin: None,
            mqtt_password: None,
            login_lock: Lockout::default(),
            pin_lock: Lockout::default(),
        }
    }
}

impl Services for InMemoryServices {
    fn now_ms(&self) -> u64 {
        self.start.elapsed().as_millis() as u64
    }
    fn new_token(&mut self) -> String {
        self.token_counter += 1;
        // Counter + uptime ms → sufficient for loopback dev; firmware uses HW-RNG.
        format!("s{}_{:x}", self.token_counter, self.now_ms())
    }
    fn verify_login(&self, password: &str) -> bool {
        !password.is_empty() && ct_eq(password.as_bytes(), self.login_password.as_bytes())
    }
    fn set_password(&mut self, new: &str) {
        self.login_password = new.to_string();
        self.password_changed = true;
    }
    fn password_change_required(&self) -> bool {
        !self.password_changed
    }
    fn login_locked(&self) -> Option<u64> {
        self.login_lock.locked_for(self.now_ms())
    }
    fn note_login_fail(&mut self) {
        let now = self.now_ms();
        self.login_lock
            .record_fail(now, LOGIN_MAX_FAILS, LOGIN_LOCK_MS);
    }
    fn note_login_ok(&mut self) {
        self.login_lock.reset();
    }
    fn set_pin(&mut self, pin: &str) {
        self.pin = Some(pin.to_string());
        self.pin_lock.reset();
    }
    fn pin_set(&self) -> bool {
        self.pin.is_some()
    }
    fn verify_pin(&mut self, pin: &str) -> PinCheck {
        let now = self.now_ms();
        if let Some(retry_after_s) = self.pin_lock.locked_for(now) {
            return PinCheck::Locked { retry_after_s };
        }
        let Some(stored) = &self.pin else {
            return PinCheck::NoPin;
        };
        if ct_eq(stored.as_bytes(), pin.as_bytes()) {
            self.pin_lock.reset();
            PinCheck::Ok
        } else {
            self.pin_lock.record_fail(now, PIN_MAX_FAILS, PIN_LOCK_MS);
            PinCheck::Wrong
        }
    }
    fn set_mqtt_password(&mut self, pw: &str) {
        self.mqtt_password = Some(pw.to_string());
    }
    fn mqtt_password_set(&self) -> bool {
        self.mqtt_password.is_some()
    }
    fn mqtt_password(&self) -> Option<String> {
        self.mqtt_password.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn weak_pins_flagged() {
        assert!(is_weak_pin("0000"));
        assert!(is_weak_pin("1111"));
        assert!(is_weak_pin("1234"));
        assert!(is_weak_pin("4321"));
        assert!(is_weak_pin("12")); // too short
        assert!(!is_weak_pin("4729"));
        assert!(!is_weak_pin("8302"));
    }

    #[test]
    fn disarm_is_fail_closed() {
        assert!(!disarm_enabled(false, false));
        assert!(!disarm_enabled(true, false), "never without PIN");
        assert!(!disarm_enabled(false, true));
        assert!(disarm_enabled(true, true));
    }

    #[test]
    fn disarm_authorized_requires_remote_and_correct_pin() {
        assert!(disarm_authorized(true, PinCheck::Ok));
        assert!(
            !disarm_authorized(false, PinCheck::Ok),
            "remote_disarm off → never"
        );
        assert!(!disarm_authorized(true, PinCheck::Wrong));
        assert!(!disarm_authorized(true, PinCheck::NoPin));
        assert!(!disarm_authorized(
            true,
            PinCheck::Locked { retry_after_s: 9 }
        ));
    }

    #[test]
    fn ct_eq_matches_eq_semantics() {
        assert!(ct_eq(b"4729", b"4729"));
        assert!(!ct_eq(b"4729", b"4720"));
        assert!(!ct_eq(b"4729", b"472")); // different lengths
        assert!(ct_eq(b"", b""));
    }

    #[test]
    fn secrets_are_write_only_and_password_flow() {
        let mut s = InMemoryServices::new("sticker-pw");
        assert!(s.password_change_required());
        assert!(s.verify_login("sticker-pw"));
        assert!(!s.verify_login("falsch"));
        s.set_password("neues-geheim");
        assert!(!s.password_change_required());
        assert!(s.verify_login("neues-geheim"));
        assert!(!s.pin_set());
        s.set_pin("4729");
        assert!(s.pin_set());
    }

    #[test]
    fn verify_pin_fail_closed_and_constant_path() {
        let mut s = InMemoryServices::new("pw");
        assert_eq!(
            s.verify_pin("4729"),
            PinCheck::NoPin,
            "without a set PIN: NoPin"
        );
        s.set_pin("4729");
        assert_eq!(s.verify_pin("0000"), PinCheck::Wrong);
        assert_eq!(s.verify_pin("4729"), PinCheck::Ok);
    }

    #[test]
    fn pin_lockout_after_repeated_wrong() {
        let mut s = InMemoryServices::new("pw");
        s.set_pin("4729");
        for _ in 0..PIN_MAX_FAILS {
            assert_eq!(s.verify_pin("0000"), PinCheck::Wrong);
        }
        // Now locked — even the CORRECT PIN is rejected (fail-closed).
        match s.verify_pin("4729") {
            PinCheck::Locked { retry_after_s } => {
                assert!(retry_after_s > 0 && retry_after_s <= 300)
            }
            other => panic!("expected Locked, got {other:?}"),
        }
    }

    #[test]
    fn correct_pin_resets_fail_counter() {
        let mut s = InMemoryServices::new("pw");
        s.set_pin("4729");
        // Just below the threshold, then correct → counter reset, no lockout.
        for _ in 0..(PIN_MAX_FAILS - 1) {
            assert_eq!(s.verify_pin("0000"), PinCheck::Wrong);
        }
        assert_eq!(s.verify_pin("4729"), PinCheck::Ok);
        for _ in 0..(PIN_MAX_FAILS - 1) {
            assert_eq!(s.verify_pin("0000"), PinCheck::Wrong, "counter was reset");
        }
    }

    #[test]
    fn login_lockout_then_unlock_on_success_reset() {
        let mut s = InMemoryServices::new("pw");
        assert!(s.login_locked().is_none());
        for _ in 0..LOGIN_MAX_FAILS {
            s.note_login_fail();
        }
        assert!(s.login_locked().is_some(), "locked after too many failures");
        // A successful reset lifts the lockout.
        s.note_login_ok();
        assert!(s.login_locked().is_none());
    }
}
