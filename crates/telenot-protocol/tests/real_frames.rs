//! Integration tests against **real** telegrams captured from a live
//! Telenot Complex 400H (via the running v0.1.3 NAS instance). These are the
//! ground-truth fixtures: if the codec round-trips them and survives realistic
//! TCP fragmentation / corruption, the framing layer is trustworthy.

use telenot_protocol::{checksum, encode_frame, FrameDecoder, FrameError};

/// Real outgoing command telegrams (full frames, hex).
const INTARM: &str = "680909687301050200053102621516";
const DISARM: &str = "680909687301050200053002E19316";
/// The fixed confirmation-ACK frame used by the bridge.
const ACK: &str = "6802026800020216";

/// User-data slices (between the 4-octet header and the checksum).
const INTARM_USERDATA: &str = "730105020005310262";
const DISARM_USERDATA: &str = "7301050200053002E1";

fn hex(s: &str) -> Vec<u8> {
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
        .collect()
}

#[test]
fn real_checksums_match() {
    assert_eq!(checksum(&hex(INTARM_USERDATA)), 0x15);
    // DISARM user-data 73 01 05 02 00 05 30 02 E1 -> 0x93
    assert_eq!(checksum(&hex(DISARM_USERDATA)), 0x93);
}

#[test]
fn parse_and_reencode_is_identical() {
    for s in [INTARM, DISARM, ACK] {
        let bytes = hex(s);
        let mut d = FrameDecoder::new();
        d.feed(&bytes);
        let frame = d.next_frame().expect("a frame").expect("a valid frame");

        let mut out = [0u8; 512];
        let n = encode_frame(frame.user_data(), &mut out).unwrap();
        assert_eq!(&out[..n], &bytes[..], "re-encode must reproduce {s}");
        assert!(d.next_frame().is_none(), "no trailing frame for {s}");
    }
}

#[test]
fn survives_tcp_fragmentation() {
    let bytes = hex(INTARM);
    let mut d = FrameDecoder::new();
    d.feed(&bytes[..3]);
    assert!(d.next_frame().is_none(), "header incomplete");
    d.feed(&bytes[3..8]);
    assert!(d.next_frame().is_none(), "body incomplete");
    d.feed(&bytes[8..]);
    let frame = d.next_frame().unwrap().unwrap();
    assert_eq!(frame.user_data(), &hex(INTARM_USERDATA)[..]);
}

#[test]
fn decodes_two_frames_in_one_read() {
    let mut combined = hex(INTARM);
    combined.extend(hex(DISARM));
    let mut d = FrameDecoder::new();
    d.feed(&combined);
    assert!(d.next_frame().unwrap().is_ok());
    assert!(d.next_frame().unwrap().is_ok());
    assert!(d.next_frame().is_none());
}

#[test]
fn reports_bad_checksum_then_resyncs_to_next_valid_frame() {
    let mut corrupt = hex(INTARM);
    let cksum_idx = corrupt.len() - 2;
    corrupt[cksum_idx] ^= 0xFF; // wreck the checksum

    let mut d = FrameDecoder::new();
    d.feed(&corrupt);
    d.feed(&hex(DISARM)); // a clean frame follows

    matches!(d.next_frame(), Some(Err(FrameError::BadChecksum { .. })))
        .then_some(())
        .expect("first pull should flag the bad checksum");

    let mut recovered = false;
    while let Some(ev) = d.next_frame() {
        if ev.is_ok() {
            recovered = true;
            break;
        }
    }
    assert!(recovered, "decoder must resync to the trailing valid frame");
}

#[test]
fn resyncs_past_leading_garbage() {
    let mut d = FrameDecoder::new();
    d.feed(&[0x00, 0xFF, 0xE5, 0x12]); // line noise + stray single-char ack
    d.feed(&hex(INTARM));

    let mut found = false;
    while let Some(ev) = d.next_frame() {
        if ev.is_ok() {
            found = true;
        }
    }
    assert!(found, "valid frame after garbage must be recovered");
}

#[test]
fn rejects_oversized_userdata() {
    let big = [0u8; 300];
    let mut out = [0u8; 512];
    assert_eq!(encode_frame(&big, &mut out), Err(FrameError::TooLong));
}
