//! Encoder for outgoing telegrams (GMS → complex 400): acknowledgement, commands (0x02)
//! and queries (0x10). All build user data into a stack buffer and frame it via
//! [`encode_frame`](crate::encode_frame) — there is NO second checksum/framing path
//! (the legacy code had a `.slice(-2)` zero-pad bug there).
//!
//! Byte layouts and checksums are verified against real command telegrams captured
//! from the panel.

use crate::frame::{encode_frame, FrameError};

/// C-field (control field) function codes (IEC 60870-5-1 FT1.2).
pub const C_SEND_NORM: u8 = 0x40;
pub const C_SEND_NDAT: u8 = 0x73;
pub const C_CONFIRM_ACK: u8 = 0x00;

/// A-field: complex 400 inserts 0x02 outbound; captured GMS command telegrams use 0x01,
/// queries use 0x02. The complex 400 does not evaluate the A-field.
pub const A_COMMAND: u8 = 0x01;
pub const A_QUERY: u8 = 0x02;

/// Address extension values.
pub const ERW_EINGAENGE: u8 = 0x01;
pub const ERW_AUSGAENGE: u8 = 0x02;
pub const ERW_BELEGT_EINGAENGE: u8 = 0x71;
pub const ERW_BELEGT_AUSGAENGE: u8 = 0x72;
pub const ERW_TEXT: u8 = 0x73;

/// Query types for record type 0x10.
pub const ABFRAGE_BELEGT: u8 = 0x24;
pub const ABFRAGE_TEXT: u8 = 0x0C;

/// Message type codes for GMS commands — send codes for [`encode_command_02`].
/// Single source of truth; do not duplicate as literals in core/mock.
pub const ART_AUSGANG_EIN: u8 = 0x00;
pub const ART_MB_SPERREN: u8 = 0x51;
pub const ART_RUECKSETZEN: u8 = 0x52;
pub const ART_EXTERN_SCHARF: u8 = 0x61;
pub const ART_INTERN_SCHARF: u8 = 0x62;
pub const ART_AUSGANG_AUS: u8 = 0x80;
pub const ART_MB_ENTSPERREN: u8 = 0xD1;
pub const ART_UNSCHARF: u8 = 0xE1;

/// The fixed CONFIRM_ACK frame, byte-identical to captured panel traffic. 8 bytes.
pub const CONF_ACK_FRAME: [u8; 8] = [0x68, 0x02, 0x02, 0x68, 0x00, 0x02, 0x02, 0x16];

/// Writes the fixed CONFIRM_ACK frame into `out`.
pub fn encode_conf_ack(out: &mut [u8]) -> Result<usize, FrameError> {
    if out.len() < CONF_ACK_FRAME.len() {
        return Err(FrameError::TooLong);
    }
    out[..CONF_ACK_FRAME.len()].copy_from_slice(&CONF_ACK_FRAME);
    Ok(CONF_ACK_FRAME.len())
}

/// Builds a SEND_NDAT with exactly one 0x02 message/control record (arm/disarm/reset/
/// switch output). Verified: `encode_command_02(0x0532, 0x02, 0x61)` is byte-identical
/// to the captured external-arm telegram.
pub fn encode_command_02(
    address: u16,
    adresserweiterung: u8,
    meldungsart: u8,
    out: &mut [u8],
) -> Result<usize, FrameError> {
    // user_data = [C, A, RecordLen=05, RecordType=02, Device=00, Addr-Hi, Addr-Lo, AddrExt, MsgType]
    let user_data = [
        C_SEND_NDAT,
        A_COMMAND,
        0x05,
        0x02,
        0x00,
        (address >> 8) as u8,
        (address & 0xFF) as u8,
        adresserweiterung,
        meldungsart,
    ];
    encode_frame(&user_data, out)
}

/// Builds a SEND_NDAT with a 0x10 query record. Verified:
/// `encode_query(0x0000, 0x71, 0x24)` is byte-identical to the captured occupancy query.
pub fn encode_query(
    address: u16,
    adresserweiterung: u8,
    abfragetyp: u8,
    out: &mut [u8],
) -> Result<usize, FrameError> {
    // user_data = [C, A, RecordLen=05, RecordType=10, Device=00, Addr-Hi, Addr-Lo, AddrExt, QueryType]
    let user_data = [
        C_SEND_NDAT,
        A_QUERY,
        0x05,
        0x10,
        0x00,
        (address >> 8) as u8,
        (address & 0xFF) as u8,
        adresserweiterung,
        abfragetyp,
    ];
    encode_frame(&user_data, out)
}

/// Builds a SEND_NDAT with a 0x50 record to **set** date/time.
/// When setting the time, the **weekday** is transmitted instead of the century (Mon=0…Sun=6,
/// required for automatic DST switching). All fields are **binary**,
/// not BCD; `jahr` is two-digit (e.g. 26 for 2026).
pub fn encode_set_datetime(dt: &crate::DateTime, out: &mut [u8]) -> Result<usize, FrameError> {
    // user_data = [C, A, RecordLen=07, RecordType=50, Year, Weekday, Month, Day, Hour, Min, Sec]
    let user_data = [
        C_SEND_NDAT,
        A_COMMAND,
        0x07,
        0x50,
        dt.jahr,
        dt.jh_or_weekday,
        dt.monat,
        dt.tag,
        dt.stunde,
        dt.minute,
        dt.sekunde,
    ];
    encode_frame(&user_data, out)
}

/// Occupancy query (two response telegrams: 0x71 inputs, 0x72 outputs).
pub fn encode_belegt_query(out: &mut [u8]) -> Result<usize, FrameError> {
    encode_query(0x0000, ERW_BELEGT_EINGAENGE, ABFRAGE_BELEGT, out)
}

/// Text/area/detection-area query for a detection point (response: 0x0C + 0x54 + 0x56).
pub fn encode_text_query(address: u16, out: &mut [u8]) -> Result<usize, FrameError> {
    encode_query(address, ERW_TEXT, ABFRAGE_TEXT, out)
}

#[cfg(test)]
mod tests {
    use super::encode_set_datetime;
    use crate::DateTime;

    #[test]
    fn set_datetime_layout_matches_reference() {
        // Set: weekday (Thu=3) instead of century, all fields binary.
        let dt = DateTime {
            jahr: 26,
            jh_or_weekday: 3,
            monat: 6,
            tag: 11,
            stunde: 14,
            minute: 30,
            sekunde: 5,
        };
        let mut buf = [0u8; 32];
        let n = encode_set_datetime(&dt, &mut buf).unwrap();
        assert_eq!(
            &buf[..n],
            &[
                0x68, 0x0B, 0x0B, 0x68, // FT1.2 header, L=11
                0x73, 0x01, // SEND_NDAT, A
                0x07, 0x50, // record length 7, record type 0x50
                0x1A, 0x03, 0x06, 0x0B, 0x0E, 0x1E,
                0x05, // Year,Weekday,Month,Day,Hour,Min,Sec
                0x2A, 0x16, // checksum, end
            ]
        );
    }
}
