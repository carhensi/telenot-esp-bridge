//! Property tests for the FT1.2 layer: the decoder must never panic on any input,
//! and encode→decode must be lossless for arbitrary user data — including fragmented
//! delivery and data embedded in garbage. Complements the cargo-fuzz targets (fuzz/) on stable.

use proptest::prelude::*;
use telenot_protocol::{encode_frame, FrameDecoder, RecordIter, MAX_USER_DATA};

proptest! {
    /// Arbitrary bytes, arbitrarily fragmented: never panics, always terminates.
    #[test]
    fn decoder_never_panics(data in proptest::collection::vec(any::<u8>(), 0..2048), chunk in 1usize..32) {
        let mut dec = FrameDecoder::new();
        for part in data.chunks(chunk) {
            dec.feed(part);
            while dec.next_frame().is_some() {}
        }
    }

    /// encode→decode round-trip for arbitrary user data up to MAX_USER_DATA.
    #[test]
    fn encode_decode_roundtrip(user in proptest::collection::vec(any::<u8>(), 0..=MAX_USER_DATA)) {
        let mut out = [0u8; MAX_USER_DATA + 6];
        let n = encode_frame(&user, &mut out).expect("passt in den Puffer");
        let mut dec = FrameDecoder::new();
        dec.feed(&out[..n]);
        let frame = dec.next_frame().expect("komplett").expect("valide");
        prop_assert_eq!(frame.user_data(), &user[..]);
        prop_assert!(dec.next_frame().is_none());
    }

    /// Round-trip also survives leading/trailing garbage and fragmentation (resync).
    #[test]
    fn roundtrip_with_garbage_and_fragmentation(
        user in proptest::collection::vec(any::<u8>(), 0..=MAX_USER_DATA),
        pre in proptest::collection::vec(any::<u8>(), 0..64),
        post in proptest::collection::vec(any::<u8>(), 0..64),
        chunk in 1usize..16,
    ) {
        let mut out = [0u8; MAX_USER_DATA + 6];
        let n = encode_frame(&user, &mut out).expect("passt in den Puffer");
        let mut stream = pre.clone();
        stream.extend_from_slice(&out[..n]);
        stream.extend_from_slice(&post);

        let mut dec = FrameDecoder::new();
        let mut found = false;
        for part in stream.chunks(chunk) {
            dec.feed(part);
            while let Some(res) = dec.next_frame() {
                if let Ok(f) = res {
                    if f.user_data() == &user[..] {
                        found = true;
                    }
                }
            }
        }
        // Leading garbage can accidentally form a valid frame header that consumes
        // part of our frame — not finding it is then correct. Without leading garbage
        // the frame MUST be found.
        if pre.is_empty() {
            prop_assert!(found, "frame with leading garbage not recovered");
        }
    }

    /// Record layer: RecordIter always terminates and never panics.
    #[test]
    fn record_iter_never_panics(data in proptest::collection::vec(any::<u8>(), 0..1024)) {
        for rec in RecordIter::new(&data) {
            let Ok(r) = rec else { break };
            let _ = (r.as_meldung(), r.as_datetime(), r.as_ascii());
            let _ = (r.as_bereich_meldebereich(), r.as_fehler(), r.as_ident());
            if let Some(bs) = r.as_block_status() {
                let base = bs.base_address();
                for a in base..base.saturating_add(32) {
                    let _ = bs.is_active(a);
                }
            }
        }
    }
}
