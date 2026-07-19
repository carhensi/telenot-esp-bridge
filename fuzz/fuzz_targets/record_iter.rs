//! Fuzzes the VdS-2465 record layer: exhausts RecordIter over arbitrary bytes
//! and calls all typed accessors — none of them may panic or read out of bounds.
#![no_main]

use libfuzzer_sys::fuzz_target;
use telenot_protocol::{Frame, RecordIter};

fuzz_target!(|data: &[u8]| {
    for rec in RecordIter::new(data) {
        let Ok(r) = rec else { break };
        if let Some(m) = r.as_meldung() {
            let _ = (m.address(), m.is_active(), m.is_input(), m.is_output(), m.kind());
        }
        if let Some(bs) = r.as_block_status() {
            let base = bs.base_address();
            // Bit accesses around the block boundaries (including out-of-range).
            for a in base..base.saturating_add(64) {
                let _ = (bs.raw_bit(a), bs.is_active(a));
            }
            let _ = (bs.raw_bit(base.wrapping_sub(1)), bs.is_belegt_response());
        }
        let _ = r.as_datetime();
        let _ = r.as_ascii();
        let _ = r.as_bereich_meldebereich();
        if let Some(f) = r.as_fehler() {
            let _ = f.fehlercode.is_not_occupied();
        }
        let _ = r.as_ident();
    }

    // Zweiter Pfad: dieselben Bytes als Frame-Nutzdaten (C-/A-Feld + Satzfolge).
    if let Ok(frame) = Frame::new(&data[..data.len().min(255)]) {
        let _ = (frame.control(), frame.function(), frame.address_byte());
        for rec in frame.records() {
            if rec.is_err() {
                break;
            }
        }
    }
});
