//! Passive polarity observation window (S4 "trigger sensor").
//!
//! No telegram is sent to the armed panel (the GMS protocol has no active trigger). Instead,
//! the setup opens a time window; the user physically triggers the detector; the bridge reads
//! the raw bit change in the block status and infers the consistent polarity:
//! - GMS-V6 default: bit `'0'` = active. At rest = contact closed, triggered = active.
//! - If the bit flips to `'0'` (false) on trigger → **ActiveLow**; to `'1'` (true) → **ActiveHigh**.

use telenot_config::Polarity;

#[derive(Debug, Clone)]
pub struct PolarityObservation {
    pub address: u16,
    pub started_ms: u64,
    pub window_ms: u64,
    initial_raw: Option<bool>,
    pub result: Option<Polarity>,
}

impl PolarityObservation {
    pub fn new(address: u16, now_ms: u64, window_ms: u64) -> Self {
        PolarityObservation {
            address,
            started_ms: now_ms,
            window_ms,
            initial_raw: None,
            result: None,
        }
    }

    /// Feed in an observed raw bit for the target address.
    pub fn observe(&mut self, raw_now: bool) {
        match self.initial_raw {
            None => self.initial_raw = Some(raw_now),
            Some(init) => {
                if self.result.is_none() && raw_now != init {
                    // Triggered = now active. Active raw bit '1' → ActiveHigh, '0' → ActiveLow.
                    self.result = Some(if raw_now {
                        Polarity::ActiveHigh
                    } else {
                        Polarity::ActiveLow
                    });
                }
            }
        }
    }

    pub fn expired(&self, now_ms: u64) -> bool {
        now_ms.saturating_sub(self.started_ms) > self.window_ms
    }

    pub fn done(&self) -> bool {
        self.result.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flip_to_zero_is_active_low() {
        let mut o = PolarityObservation::new(0x0042, 0, 30_000);
        o.observe(true); // at rest: contact closed, bit '1'
        assert_eq!(o.result, None);
        o.observe(false); // triggered: bit '0' → active
        assert_eq!(o.result, Some(Polarity::ActiveLow));
    }

    #[test]
    fn flip_to_one_is_active_high() {
        let mut o = PolarityObservation::new(0x0042, 0, 30_000);
        o.observe(false);
        o.observe(true);
        assert_eq!(o.result, Some(Polarity::ActiveHigh));
    }

    #[test]
    fn window_expires() {
        let o = PolarityObservation::new(0x0042, 1000, 30_000);
        assert!(!o.expired(20_000));
        assert!(o.expired(40_000));
    }
}
