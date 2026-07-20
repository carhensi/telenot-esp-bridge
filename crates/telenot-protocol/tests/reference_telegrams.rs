//! Tests against complete GMS reference telegrams captured from the panel — with
//! real checksums. Each frame is pushed through the FrameDecoder; decoding as `Ok`
//! validates both the checksum AND the byte-exact transcription at once.
//! Command frames are cross-verified against real NAS captures.

use telenot_protocol::{
    encode_belegt_query, encode_command_02, encode_conf_ack, encode_text_query, BlockStatus,
    FehlerCode, FrameDecoder, Function, MeldungsArt, Record, ERW_AUSGAENGE,
};

/// Hex string → bytes; whitespace is ignored (allows direct copy from capture logs).
fn hex(s: &str) -> Vec<u8> {
    let clean: Vec<u8> = s.bytes().filter(|b| !b.is_ascii_whitespace()).collect();
    clean
        .chunks(2)
        .map(|c| {
            let hi = (c[0] as char).to_digit(16).unwrap();
            let lo = (c[1] as char).to_digit(16).unwrap();
            (hi * 16 + lo) as u8
        })
        .collect()
}

/// Decodes exactly one frame from a complete telegram hex string (validates the checksum).
fn decode_one(ref_hex: &str) -> telenot_protocol::Frame {
    let bytes = hex(ref_hex);
    let mut d = FrameDecoder::new();
    d.feed(&bytes);
    let frame = d
        .next_frame()
        .expect("ein Frame")
        .expect("valid frame (checksum ok)");
    assert!(d.next_frame().is_none(), "kein Rest");
    frame
}

fn records(frame: &telenot_protocol::Frame) -> Vec<Record<'_>> {
    frame.records().map(|r| r.expect("Satz ok")).collect()
}

// --- Telegramme complex 400 → GMS -----------------------------------------

#[test]
fn send_norm_and_conf_ack_classify() {
    assert_eq!(
        decode_one("68 02 02 68 40 02 42 16").function(),
        Some(Function::SendNorm)
    );
    assert_eq!(
        decode_one("68 02 02 68 00 02 02 16").function(),
        Some(Function::ConfirmAck)
    );
    // SEND_NDAT frames carry C=0x73.
    let alarm = decode_one(ALARM);
    assert_eq!(alarm.function(), Some(Function::SendNdat));
}

const ALARM: &str = "68 2C 2C 68 73 02 \
    05 02 01 00 01 01 22 \
    07 50 05 14 03 08 0A 10 32 \
    10 54 4D 41 2D 4D 47 30 32 20 20 20 20 20 20 20 20 20 \
    06 56 00 80 51 FF FF FF C7 16";

#[test]
fn alarm_message_decodes_all_records() {
    let frame = decode_one(ALARM);
    let recs = records(&frame);
    assert_eq!(recs.len(), 4, "0x02 + 0x50 + 0x54 + 0x56");

    let m = recs[0].as_meldung().expect("0x02");
    assert_eq!(m.address(), 0x0001);
    assert_eq!(m.kind(), MeldungsArt::Einbruch);
    assert!(m.is_active(), "0x22 -> triggered");
    assert!(m.is_input());
    assert_eq!(m.geraet_bereich, 0x01, "Bereich 1");

    let dt = recs[1].as_datetime().expect("0x50");
    assert_eq!((dt.monat, dt.tag, dt.stunde), (0x03, 0x08, 0x0A));

    assert_eq!(recs[2].as_ascii().expect("0x54"), b"MA-MG02");
    assert!(recs[3].as_ident().is_some(), "0x56");
}

#[test]
fn arming_states_from_spontaneous_messages() {
    // extern scharf (0x0532, Meldungsart 0x61 = ein)
    let m = records(&decode_one(EXTERN_SCHARF))[0].as_meldung().unwrap();
    assert_eq!(m.address(), 0x0532);
    assert_eq!(m.kind(), MeldungsArt::Sicherungsbereich);
    assert!(m.is_active(), "0x61 -> scharf");

    // unscharf (0x0530, Meldungsart 0xE1 = aus)
    let m = records(&decode_one(UNSCHARF))[0].as_meldung().unwrap();
    assert_eq!(m.address(), 0x0530);
    assert_eq!(m.kind(), MeldungsArt::Sicherungsbereich);
    assert!(!m.is_active(), "0xE1 -> unscharf");

    // Neustart (Adresse 0xFFFF)
    let m = records(&decode_one(NEUSTART))[0].as_meldung().unwrap();
    assert_eq!(m.address(), 0xFFFF);
    assert_eq!(m.kind(), MeldungsArt::Neustart);
}

const EXTERN_SCHARF: &str = "68 2C 2C 68 73 02 \
    05 02 00 05 32 01 61 \
    07 50 05 14 03 0B 08 13 12 \
    10 54 42 65 72 65 63 68 74 69 67 75 6E 67 20 37 20 20 \
    06 56 00 80 51 FF FF FF BC 16";

const UNSCHARF: &str = "68 2C 2C 68 73 02 \
    05 02 00 05 30 01 E1 \
    07 50 05 14 03 0B 09 03 1B \
    10 54 42 65 72 65 63 68 74 69 67 75 6E 67 20 38 20 20 \
    06 56 00 80 51 FF FF FF 35 16";

const NEUSTART: &str = "68 1A 1A 68 73 02 \
    05 02 00 FF FF 01 53 \
    07 50 05 14 03 0A 0B 10 39 \
    06 56 00 80 51 FF FF FF C9 16";

#[test]
fn status_telegram_block_status() {
    // Input status, basic configuration, from address 0x0000.
    let frame = decode_one(STATUS_INPUTS);
    let recs = records(&frame);
    let bs: BlockStatus = recs[0].as_block_status().expect("0x24");
    assert_eq!(bs.base_address(), 0x0000);
    assert_eq!(bs.adresserweiterung, 0x01, "inputs");
    assert_eq!(bs.status.len(), 30, "Grundausbau: 30 Statusbytes");
    // FF at address 0x0000 -> bit '1' -> idle (not active).
    assert_eq!(bs.raw_bit(0x0000), Some(true));
    assert_eq!(bs.is_active(0x0000), Some(false));
    assert_eq!(bs.raw_bit(0x9999), None, "ausserhalb des Blocks");
}

const STATUS_INPUTS: &str = "68 2E 2E 68 73 02 \
    22 24 10 00 00 01 \
    FF FF FF FF FF \
    FF FF FF FF FF FF FF FF \
    FF FF FF FF FF FF FF FF \
    FF \
    FB FF FF FF FF FF FF FF \
    06 56 00 80 51 FF FF FF D4 16";

#[test]
fn error_telegram_embedded_in_conf_ack() {
    // CONFIRM_ACK with embedded error record 0x11, code 0x10.
    let frame = decode_one("68 06 06 68 00 02 02 11 00 10 25 16");
    assert_eq!(frame.function(), Some(Function::ConfirmAck));
    let f = records(&frame)[0].as_fehler().expect("0x11");
    assert_eq!(f.fehlercode, FehlerCode::AdresseAusserhalb);
}

// --- Befehle GMS → complex 400 (Encoder-Golden-Tests) ---------------------

fn encode<F: Fn(&mut [u8]) -> Result<usize, telenot_protocol::FrameError>>(f: F) -> Vec<u8> {
    let mut out = [0u8; 64];
    let n = f(&mut out).expect("encode ok");
    out[..n].to_vec()
}

#[test]
fn command_encoders_match_reference_and_real_corpus() {
    // extern scharf (0x0532, 0x61)
    assert_eq!(
        encode(|o| encode_command_02(0x0532, ERW_AUSGAENGE, 0x61, o)),
        hex("68 09 09 68 73 01 05 02 00 05 32 02 61 15 16")
    );
    // intern scharf == realer NAS-Mitschnitt
    assert_eq!(
        encode(|o| encode_command_02(0x0531, ERW_AUSGAENGE, 0x62, o)),
        hex("680909687301050200053102621516")
    );
    // unscharf == realer NAS-Mitschnitt
    assert_eq!(
        encode(|o| encode_command_02(0x0530, ERW_AUSGAENGE, 0xE1, o)),
        hex("680909687301050200053002E19316")
    );
    // Reset (0x0533, 0x52)
    assert_eq!(
        encode(|o| encode_command_02(0x0533, ERW_AUSGAENGE, 0x52, o)),
        hex("68 09 09 68 73 01 05 02 00 05 33 02 52 07 16")
    );
    // Ausgang ein/aus (0x0515)
    assert_eq!(
        encode(|o| encode_command_02(0x0515, ERW_AUSGAENGE, 0x00, o)),
        hex("68 09 09 68 73 01 05 02 00 05 15 02 00 97 16")
    );
    assert_eq!(
        encode(|o| encode_command_02(0x0515, ERW_AUSGAENGE, 0x80, o)),
        hex("68 09 09 68 73 01 05 02 00 05 15 02 80 17 16")
    );
}

#[test]
fn query_encoders_match_reference() {
    // Belegtstatus-Abfrage
    assert_eq!(
        encode(encode_belegt_query),
        hex("68 09 09 68 73 02 05 10 00 00 00 71 24 1F 16")
    );
    // Text-Abfrage MG8 (Adresse 0x0007)
    assert_eq!(
        encode(|o| encode_text_query(0x0007, o)),
        hex("68 09 09 68 73 02 05 10 00 00 07 73 0C 10 16")
    );
    // Text-Abfrage Sicherungsbereich 1 (Adresse 0x0530)
    assert_eq!(
        encode(|o| encode_text_query(0x0530, o)),
        hex("68 09 09 68 73 02 05 10 00 05 30 73 0C 3E 16")
    );
}

#[test]
fn text_query_response_yields_name() {
    // Response: 0x0C (area/detection area) + 0x54 (name) + 0x56 (ID).
    let frame = decode_one(
        "68 24 24 68 73 02 \
         06 0C 01 00 07 73 FE 08 \
         10 54 4D 41 2D 4D 47 30 38 20 20 20 20 20 20 20 20 20 \
         06 56 00 80 51 FF FF FF 6D 16",
    );
    let recs = records(&frame);
    let bm = recs[0].as_bereich_meldebereich().expect("0x0C");
    assert_eq!(bm.address(), 0x0007);
    assert_eq!(bm.meldebereich, 0x08);
    assert_eq!(recs[1].as_ascii().expect("0x54"), b"MA-MG08");
}

#[test]
fn conf_ack_encoder_is_fixed_frame() {
    assert_eq!(encode(encode_conf_ack), hex("68 02 02 68 00 02 02 16"));
}

#[test]
fn record_iter_is_truncation_safe() {
    // Artificial frame with a record whose length extends past the end:
    // user_data = [C=73, A=02, RecordLen=05, RecordType=02, only 1 payload byte].
    let frame = {
        let mut d = FrameDecoder::new();
        // wrap these (internally "broken") user data in a valid frame
        let mut out = [0u8; 32];
        let n = telenot_protocol::encode_frame(&[0x73, 0x02, 0x05, 0x02, 0xAA], &mut out).unwrap();
        d.feed(&out[..n]);
        d.next_frame().unwrap().unwrap()
    };
    let last = frame.records().last().unwrap();
    assert!(last.is_err(), "over-long record -> Truncated, no panic");
}
