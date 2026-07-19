//! VdS-2465 record layer: splits the user data of an FT1.2 frame (after the C and
//! A fields) into a sequence of records `[RecordLen][RecordType][Payload…]` and provides
//! alloc-free, typed views of the record types used in GMS V6.
//!
//! **RecordLen** = number of payload octets AFTER the record type byte. A record therefore
//! occupies `RecordLen + 2` bytes (length byte + type byte + payload). Verified against
//! the frame length fields of real telegrams captured from the panel.
//!
//! This layer does NOT aggregate (no lists, no strings) — collecting across multiple
//! telegrams (discovery) is the responsibility of `telenot-core`.

use crate::frame::Frame;

/// VdS 2465 record types used by GMS-V6.
pub mod satztyp {
    pub const MELDUNG: u8 = 0x02;
    pub const BEREICH_MELDEBEREICH: u8 = 0x0C;
    pub const ABFRAGE: u8 = 0x10;
    pub const FEHLER: u8 = 0x11;
    pub const BLOCKSTATUS: u8 = 0x24;
    pub const DATUM_UHRZEIT: u8 = 0x50;
    pub const ASCII: u8 = 0x54;
    pub const IDENT: u8 = 0x56;
}

/// A single user-data record: type + payload slice (borrowed from the frame buffer).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Record<'a> {
    pub satztyp: u8,
    pub payload: &'a [u8],
}

/// Error while parsing a record sequence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordError {
    /// Record extends past the end of the user data (zero length field / overflow).
    Truncated,
}

/// Length-driven iterator over a record sequence. Defensive: a record that extends
/// past the end terminates iteration with `Err(Truncated)` — never panics,
/// never advances into unknown territory.
#[derive(Debug, Clone)]
pub struct RecordIter<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> RecordIter<'a> {
    pub fn new(buf: &'a [u8]) -> Self {
        RecordIter { buf, pos: 0 }
    }
}

impl<'a> Iterator for RecordIter<'a> {
    type Item = Result<Record<'a>, RecordError>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.pos >= self.buf.len() {
            return None;
        }
        // Need at least the length and type bytes.
        if self.buf.len() - self.pos < 2 {
            self.pos = self.buf.len();
            return Some(Err(RecordError::Truncated));
        }
        let len = self.buf[self.pos] as usize;
        let total = 2 + len;
        if self.pos + total > self.buf.len() {
            self.pos = self.buf.len();
            return Some(Err(RecordError::Truncated));
        }
        let satztyp = self.buf[self.pos + 1];
        let payload = &self.buf[self.pos + 2..self.pos + total];
        self.pos += total;
        Some(Ok(Record { satztyp, payload }))
    }
}

impl Frame {
    /// Function code byte (C-field) of the user data.
    pub fn function_byte(&self) -> Option<u8> {
        self.user_data().first().copied()
    }

    /// Address field byte (A-field). Not evaluated by the complex 400.
    pub fn address_byte(&self) -> Option<u8> {
        self.user_data().get(1).copied()
    }

    /// Classifies the C-field into an IEC 60870-5-1 FT1.2 function.
    pub fn function(&self) -> Option<Function> {
        self.function_byte().map(Function::from_control)
    }

    /// Iterates over the user-data records (after C and A fields).
    pub fn records(&self) -> RecordIter<'_> {
        RecordIter::new(self.user_data().get(2..).unwrap_or(&[]))
    }
}

/// IEC 60870-5-1 FT1.2 function code of the C-field. Inbound only F0–F3 is evaluated; SEND_NORM
/// (master) and CONFIRM_ACK (slave) share F=0 and are distinguished via bit 6.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Function {
    SendNorm,
    SendNdat,
    ConfirmAck,
    ConfirmNak,
    Other(u8),
}

impl Function {
    pub fn from_control(c: u8) -> Self {
        match c & 0x0F {
            3 => Function::SendNdat,
            1 => Function::ConfirmNak,
            0 if c & 0x40 != 0 => Function::SendNorm,
            0 => Function::ConfirmAck,
            _ => Function::Other(c),
        }
    }
}

// ---------------------------------------------------------------------------
// Typed views for individual record types (alloc-free).
// ---------------------------------------------------------------------------

/// VdS 2465 record type 0x02 — message/state change/control.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Meldung {
    pub geraet_bereich: u8,
    pub adresse: u8,
    pub adressenzusatz: u8,
    pub adresserweiterung: u8,
    pub meldungsart: u8,
}

impl Meldung {
    /// 16-bit detection-point address: adresse = high byte, adressenzusatz = low byte
    /// (verified: external arm area 1 = 0x0532 from adresse 0x05 / zusatz 0x32).
    pub fn address(&self) -> u16 {
        ((self.adresse as u16) << 8) | self.adressenzusatz as u16
    }
    /// Bit 7 of the message type: 0 = triggered/on/armed, 1 = withdrawn/off.
    pub fn is_active(&self) -> bool {
        self.meldungsart & 0x80 == 0
    }
    pub fn is_input(&self) -> bool {
        self.adresserweiterung == 0x01
    }
    pub fn is_output(&self) -> bool {
        self.adresserweiterung == 0x02
    }
    pub fn kind(&self) -> MeldungsArt {
        MeldungsArt::from_code(self.meldungsart)
    }
}

/// Message type classification from the lower 7 bits of the message type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MeldungsArt {
    Meldung,
    Brand,
    Ueberfall,
    Einbruch,
    Sabotage,
    Stoerung,
    StoerungNetz,
    StoerungAkku,
    StoerungUebertragungsweg,
    TechnischeMeldung,
    Technikalarm,
    Abschaltung,
    Ruecksetzen,
    Neustart,
    Sicherungsbereich,
    Internbereich,
    Unbekannt(u8),
}

impl MeldungsArt {
    pub fn from_code(meldungsart: u8) -> Self {
        match meldungsart & 0x7F {
            0x00 => MeldungsArt::Meldung,
            0x10 => MeldungsArt::Brand,
            0x21 => MeldungsArt::Ueberfall,
            0x22 => MeldungsArt::Einbruch,
            0x23 => MeldungsArt::Sabotage,
            0x30 => MeldungsArt::Stoerung,
            0x32 => MeldungsArt::StoerungNetz,
            0x33 => MeldungsArt::StoerungAkku,
            0x34 => MeldungsArt::StoerungUebertragungsweg,
            0x40 => MeldungsArt::TechnischeMeldung,
            0x41 => MeldungsArt::Technikalarm,
            0x51 => MeldungsArt::Abschaltung,
            0x52 => MeldungsArt::Ruecksetzen,
            0x53 => MeldungsArt::Neustart,
            0x61 => MeldungsArt::Sicherungsbereich,
            0x62 => MeldungsArt::Internbereich,
            other => MeldungsArt::Unbekannt(other),
        }
    }
}

/// VdS 2465 record type 0x24 — block status. Bits LSB-first from `base_address()`;
/// `'1' = idle/off/withdrawn`, `'0' = active/triggered/on`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlockStatus<'a> {
    pub geraet_bereich: u8,
    pub adresse: u8,
    pub adressenzusatz: u8,
    pub adresserweiterung: u8,
    pub status: &'a [u8],
}

impl<'a> BlockStatus<'a> {
    pub fn base_address(&self) -> u16 {
        ((self.adresse as u16) << 8) | self.adressenzusatz as u16
    }
    /// Raw bit at the given absolute address (`true` = set/'1'), `None` if outside the block.
    pub fn raw_bit(&self, addr: u16) -> Option<bool> {
        let base = self.base_address();
        if addr < base {
            return None;
        }
        let off = (addr - base) as usize;
        self.status.get(off / 8).map(|b| (b >> (off % 8)) & 1 == 1)
    }
    /// GMS-V6 polarity convention: active/triggered/on when the bit is `'0'`.
    pub fn is_active(&self, addr: u16) -> Option<bool> {
        self.raw_bit(addr).map(|b| !b)
    }
    /// Occupancy query response (address extension 0x71 inputs / 0x72 outputs).
    pub fn is_belegt_response(&self) -> bool {
        matches!(self.adresserweiterung, 0x71 | 0x72)
    }
}

/// VdS 2465 record type 0x50 — date/time, binary. `jh_or_weekday` is century
/// (when receiving) OR weekday (when setting) — ambiguous on receipt, do not guess.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DateTime {
    pub jahr: u8,
    pub jh_or_weekday: u8,
    pub monat: u8,
    pub tag: u8,
    pub stunde: u8,
    pub minute: u8,
    pub sekunde: u8,
}

/// Record type 0x0C — area/detection area for a detection point.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BereichMeldebereich {
    pub geraet_bereich: u8,
    pub adresse: u8,
    pub adressenzusatz: u8,
    pub adresserweiterung: u8,
    pub bereiche: u8,
    pub meldebereich: u8,
}

impl BereichMeldebereich {
    pub fn address(&self) -> u16 {
        ((self.adresse as u16) << 8) | self.adressenzusatz as u16
    }
}

/// Record type 0x11 — error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Fehler {
    pub geraet: u8,
    pub fehlercode: FehlerCode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FehlerCode {
    Rahmenfehler,
    SteuerfeldUnbekannt,
    MeldungsartUnbekannt,
    AdresseAusserhalb,
    FunktionNichtMoeglich,
    NichtBelegt,
    Pruefsumme,
    SatztypUnbekannt,
    Unbekannt(u8),
}

impl FehlerCode {
    pub fn from_code(c: u8) -> Self {
        match c {
            0x00 => FehlerCode::Rahmenfehler,
            0x01 => FehlerCode::SteuerfeldUnbekannt,
            0x02 => FehlerCode::MeldungsartUnbekannt,
            0x10 => FehlerCode::AdresseAusserhalb,
            0x18 => FehlerCode::FunktionNichtMoeglich,
            0x19 => FehlerCode::NichtBelegt,
            0x80 => FehlerCode::Pruefsumme,
            0xFF => FehlerCode::SatztypUnbekannt,
            other => FehlerCode::Unbekannt(other),
        }
    }
    /// Address not occupied / component missing — during discovery scan means "no detection
    /// point here", not an abort condition.
    pub fn is_not_occupied(&self) -> bool {
        matches!(
            self,
            FehlerCode::FunktionNichtMoeglich | FehlerCode::NichtBelegt
        )
    }
}

// ---------------------------------------------------------------------------
// Typed accessors on Record (alloc-free, length-validated).
// ---------------------------------------------------------------------------

impl<'a> Record<'a> {
    pub fn as_meldung(&self) -> Option<Meldung> {
        if self.satztyp != satztyp::MELDUNG || self.payload.len() < 5 {
            return None;
        }
        let p = self.payload;
        Some(Meldung {
            geraet_bereich: p[0],
            adresse: p[1],
            adressenzusatz: p[2],
            adresserweiterung: p[3],
            meldungsart: p[4],
        })
    }

    pub fn as_block_status(&self) -> Option<BlockStatus<'a>> {
        if self.satztyp != satztyp::BLOCKSTATUS || self.payload.len() < 4 {
            return None;
        }
        let p = self.payload;
        Some(BlockStatus {
            geraet_bereich: p[0],
            adresse: p[1],
            adressenzusatz: p[2],
            adresserweiterung: p[3],
            status: &p[4..],
        })
    }

    pub fn as_datetime(&self) -> Option<DateTime> {
        if self.satztyp != satztyp::DATUM_UHRZEIT || self.payload.len() < 7 {
            return None;
        }
        let p = self.payload;
        Some(DateTime {
            jahr: p[0],
            jh_or_weekday: p[1],
            monat: p[2],
            tag: p[3],
            stunde: p[4],
            minute: p[5],
            sekunde: p[6],
        })
    }

    /// ASCII text (0x54), trailing spaces (0x20) stripped.
    pub fn as_ascii(&self) -> Option<&'a [u8]> {
        if self.satztyp != satztyp::ASCII {
            return None;
        }
        let mut end = self.payload.len();
        while end > 0 && self.payload[end - 1] == 0x20 {
            end -= 1;
        }
        Some(&self.payload[..end])
    }

    pub fn as_bereich_meldebereich(&self) -> Option<BereichMeldebereich> {
        if self.satztyp != satztyp::BEREICH_MELDEBEREICH || self.payload.len() < 6 {
            return None;
        }
        let p = self.payload;
        Some(BereichMeldebereich {
            geraet_bereich: p[0],
            adresse: p[1],
            adressenzusatz: p[2],
            adresserweiterung: p[3],
            bereiche: p[4],
            meldebereich: p[5],
        })
    }

    pub fn as_fehler(&self) -> Option<Fehler> {
        if self.satztyp != satztyp::FEHLER || self.payload.len() < 2 {
            return None;
        }
        Some(Fehler {
            geraet: self.payload[0],
            fehlercode: FehlerCode::from_code(self.payload[1]),
        })
    }

    /// Raw bytes of the identification number (0x56).
    pub fn as_ident(&self) -> Option<&'a [u8]> {
        if self.satztyp != satztyp::IDENT {
            return None;
        }
        Some(self.payload)
    }
}
