//! Telenot GMS protocol — pure, `no_std`, hardware-free.
//!
//! The Telenot GMS serial interface uses **IEC 60870-5-1 FT1.2** variable-length
//! frames:
//!
//! ```text
//!   0x68 | L | L | 0x68 | <L user-data octets> | checksum | 0x16
//! ```
//!
//! where `checksum = (Σ user-data octets) mod 256`.
//!
//! This crate intentionally knows **nothing** about sensor maps, areas or arm
//! states — it only turns a byte stream into validated [`Frame`]s and back.
//! The mapping from frame bytes to named sensors / arm states lives in the
//! `telenot-core` crate, so that both layers stay host-testable in isolation.
#![cfg_attr(not(test), no_std)]

mod checksum;
mod command;
mod frame;
mod record;

pub use checksum::checksum;
pub use command::{
    encode_belegt_query, encode_command_02, encode_conf_ack, encode_query, encode_set_datetime,
    encode_text_query, ABFRAGE_BELEGT, ABFRAGE_TEXT, ART_AUSGANG_AUS, ART_AUSGANG_EIN,
    ART_EXTERN_SCHARF, ART_INTERN_SCHARF, ART_MB_ENTSPERREN, ART_MB_SPERREN, ART_RUECKSETZEN,
    ART_UNSCHARF, A_COMMAND, A_QUERY, CONF_ACK_FRAME, C_CONFIRM_ACK, C_SEND_NDAT, C_SEND_NORM,
    ERW_AUSGAENGE, ERW_BELEGT_AUSGAENGE, ERW_BELEGT_EINGAENGE, ERW_EINGAENGE, ERW_TEXT,
};
pub use frame::{encode_frame, Frame, FrameDecoder, FrameError, MAX_USER_DATA};
pub use record::{
    satztyp, BereichMeldebereich, BlockStatus, DateTime, Fehler, FehlerCode, Function, Meldung,
    MeldungsArt, Record, RecordError, RecordIter,
};
