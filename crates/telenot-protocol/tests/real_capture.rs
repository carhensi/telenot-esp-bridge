//! Regression tests against REAL frames from a pcap capture of the live panel
//! (2026-06-02, two internal-arm/disarm cycles + window toggle). Validates the
//! decoder against the actual complex 400 — including the security area status
//! addresses 0x0530 (disarmed) / 0x0531 (internally armed) confirmed via the diff.

use telenot_protocol::{BlockStatus, Frame, FrameDecoder, MeldungsArt};

fn block_status(f: &Frame) -> BlockStatus<'_> {
    f.records()
        .filter_map(|r| r.ok())
        .find_map(|r| r.as_block_status())
        .expect("0x24")
}

fn hex(s: &str) -> Vec<u8> {
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
        .collect()
}

fn decode(s: &str) -> telenot_protocol::Frame {
    let mut d = FrameDecoder::new();
    d.feed(&hex(s));
    d.next_frame().expect("frame").expect("valid (checksum ok)")
}

// Output status telegrams (0x24, base 0x0500) in the disarmed and internally armed states.
const OUTSTATUS_DISARMED: &str = "6846466873023a2400050002fffffffffffffe9e9e9e9e9e9e9efffcffffffffffffffffffffffffffff7fffffffffffffffffffffffffffffffffffffffffffffff0656999999ffffff9d16";
const OUTSTATUS_INTERN: &str = "6846466873023a2400050002ffffffffffffdd9e9e9e9e9e9e9efffdffffffffffffffffffffffffffff7fffffffffffffffffffffffffffffffefefefffffffffff0656999999ffffff4d16";
// Spontaneous messages on arm/disarm.
const SPONT_INTERN_ARM: &str =
    "682c2c6873020502000531016207501a1406020824051054474d53202020202020202020202020200656999999ffffffe216";
const SPONT_DISARM: &str =
    "682c2c687302050200053001e107501a14060208240b1054474d53202020202020202020202020200656999999ffffff6616";
// Input status telegram (0x24, base 0x0000).
const INSTATUS: &str = "683e3e687302322400000001fffffffffffffffffffffffffffffffffdfffffffffffbfffffffffffffffffffdffffffffffffffffffffffffff0656999999ffffffba16";

#[test]
fn all_real_frames_pass_checksum() {
    for f in [
        OUTSTATUS_DISARMED,
        OUTSTATUS_INTERN,
        SPONT_INTERN_ARM,
        SPONT_DISARM,
        INSTATUS,
    ] {
        decode(f); // panics if checksum/framing invalid
    }
}

#[test]
fn bereichsstatus_diff_confirms_arm_addresses() {
    let disarmed = decode(OUTSTATUS_DISARMED);
    let intern = decode(OUTSTATUS_INTERN);

    let d = block_status(&disarmed);
    let i = block_status(&intern);
    assert_eq!(d.base_address(), 0x0500);
    assert_eq!(d.adresserweiterung, 0x02);

    // Disarmed: 0x0530 active, 0x0531 inactive. Internally armed: exactly reversed.
    assert_eq!(
        d.is_active(0x0530),
        Some(true),
        "disarmed -> unscharf aktiv"
    );
    assert_eq!(d.is_active(0x0531), Some(false));
    assert_eq!(i.is_active(0x0530), Some(false));
    assert_eq!(
        i.is_active(0x0531),
        Some(true),
        "intern -> intern scharf aktiv"
    );
}

#[test]
fn spontaneous_arm_disarm_decode() {
    let arm = decode(SPONT_INTERN_ARM);
    let m = arm
        .records()
        .filter_map(|r| r.ok())
        .find_map(|r| r.as_meldung())
        .unwrap();
    assert_eq!(m.address(), 0x0531);
    assert_eq!(m.kind(), MeldungsArt::Internbereich);
    assert!(m.is_active());
    // Plain-text source "GMS".
    let text = arm
        .records()
        .filter_map(|r| r.ok())
        .find_map(|r| r.as_ascii())
        .unwrap();
    assert_eq!(text, b"GMS");

    let dis = decode(SPONT_DISARM);
    let m = dis
        .records()
        .filter_map(|r| r.ok())
        .find_map(|r| r.as_meldung())
        .unwrap();
    assert_eq!(m.address(), 0x0530);
    assert_eq!(m.kind(), MeldungsArt::Sicherungsbereich);
    assert!(!m.is_active(), "0xE1 -> unscharf");
}

#[test]
fn input_status_has_expected_block() {
    let f = decode(INSTATUS);
    let bs = f
        .records()
        .filter_map(|r| r.ok())
        .find_map(|r| r.as_block_status())
        .unwrap();
    assert_eq!(bs.base_address(), 0x0000);
    assert_eq!(bs.adresserweiterung, 0x01, "inputs");
    // Address 0x0075 (im_essen from the legacy config) lies within the block.
    assert!(bs.raw_bit(0x0075).is_some());
}
