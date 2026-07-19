use crate::checksum::checksum;

/// Maximum user-data length. The FT1.2 length field is a single octet.
pub const MAX_USER_DATA: usize = 255;

const START: u8 = 0x68;
const END: u8 = 0x16;
/// Per frame: `START, L, L, START` (4) + `checksum, END` (2).
const OVERHEAD: usize = 6;

/// Internal accumulation buffer of the streaming decoder. Holds the largest
/// possible frame (`MAX_USER_DATA + OVERHEAD = 261`) plus slack for resync.
const DECODER_CAP: usize = 512;

/// A validated, framed telegram. Owns a copy of its user-data octets so it can
/// outlive the decoder's internal buffer (no borrows, `Copy`-friendly size).
#[derive(Clone, PartialEq, Eq)]
pub struct Frame {
    buf: [u8; MAX_USER_DATA],
    len: usize,
}

impl Frame {
    /// Build a frame body from user-data octets (no header/checksum).
    pub fn new(user_data: &[u8]) -> Result<Self, FrameError> {
        if user_data.len() > MAX_USER_DATA {
            return Err(FrameError::TooLong);
        }
        let mut buf = [0u8; MAX_USER_DATA];
        buf[..user_data.len()].copy_from_slice(user_data);
        Ok(Frame {
            buf,
            len: user_data.len(),
        })
    }

    /// The user-data octets (FT1.2 control field, address, payload…).
    pub fn user_data(&self) -> &[u8] {
        &self.buf[..self.len]
    }

    /// First user-data octet — the FT1.2 control field of a GMS telegram.
    pub fn control(&self) -> Option<u8> {
        if self.len > 0 {
            Some(self.buf[0])
        } else {
            None
        }
    }
}

impl core::fmt::Debug for Frame {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "Frame[{}]<", self.len)?;
        for b in self.user_data() {
            write!(f, "{b:02x}")?;
        }
        write!(f, ">")
    }
}

/// Why a frame could not be decoded. Surfaced to the caller for diagnostics;
/// the decoder always resynchronises afterwards rather than wedging the stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameError {
    /// Header malformed: length octets disagree or the second `0x68` is missing.
    BadHeader,
    /// Transmitted checksum did not match the computed one.
    BadChecksum { expected: u8, got: u8 },
    /// End delimiter `0x16` missing where the length field said it should be.
    BadEnd,
    /// User data exceeds the single-octet length field / output buffer.
    TooLong,
}

/// Encode user-data into a complete FT1.2 frame. Returns the number of bytes
/// written into `out`.
pub fn encode_frame(user_data: &[u8], out: &mut [u8]) -> Result<usize, FrameError> {
    if user_data.len() > MAX_USER_DATA {
        return Err(FrameError::TooLong);
    }
    let total = user_data.len() + OVERHEAD;
    if out.len() < total {
        return Err(FrameError::TooLong);
    }
    let l = user_data.len() as u8;
    out[0] = START;
    out[1] = l;
    out[2] = l;
    out[3] = START;
    out[4..4 + user_data.len()].copy_from_slice(user_data);
    out[4 + user_data.len()] = checksum(user_data);
    out[4 + user_data.len() + 1] = END;
    Ok(total)
}

/// Streaming, fragmentation-tolerant FT1.2 frame decoder.
///
/// TCP delivers the GMS byte stream in arbitrary chunks, so a frame may arrive
/// split across reads or several frames may arrive at once. Feed raw bytes with
/// [`feed`](Self::feed), then drain frames with [`next_frame`](Self::next_frame)
/// until it returns `None`. On garbage or a bad frame the decoder reports the
/// error once and resynchronises to the next `0x68` — it never gets stuck.
pub struct FrameDecoder {
    buf: [u8; DECODER_CAP],
    len: usize,
}

impl Default for FrameDecoder {
    fn default() -> Self {
        Self::new()
    }
}

impl FrameDecoder {
    pub const fn new() -> Self {
        FrameDecoder {
            buf: [0u8; DECODER_CAP],
            len: 0,
        }
    }

    /// Append received bytes. If the buffer is full, the **oldest** bytes are
    /// dropped first — a stuck partial frame must never wedge the stream.
    pub fn feed(&mut self, input: &[u8]) {
        for &b in input {
            if self.len == DECODER_CAP {
                self.buf.copy_within(1..DECODER_CAP, 0);
                self.len -= 1;
            }
            self.buf[self.len] = b;
            self.len += 1;
        }
    }

    fn drop_front(&mut self, n: usize) {
        let n = n.min(self.len);
        self.buf.copy_within(n..self.len, 0);
        self.len -= n;
    }

    /// Pull the next frame (or framing error) out of the buffer. Returns `None`
    /// when more bytes are needed to complete the frame at the front.
    pub fn next_frame(&mut self) -> Option<Result<Frame, FrameError>> {
        // Resync: discard anything before the first START byte.
        if self.len == 0 {
            return None;
        }
        if self.buf[0] != START {
            let mut i = 1;
            while i < self.len && self.buf[i] != START {
                i += 1;
            }
            self.drop_front(i);
            if self.len == 0 {
                return None;
            }
        }

        // Need the full 4-octet header before we can trust the length field.
        if self.len < 4 {
            return None;
        }
        let l = self.buf[1] as usize;
        if self.buf[2] != self.buf[1] || self.buf[3] != START {
            self.drop_front(1); // bad header → drop one START and resync
            return Some(Err(FrameError::BadHeader));
        }

        let total = l + OVERHEAD;
        if self.len < total {
            return None; // fragmented — wait for the rest
        }

        let user = &self.buf[4..4 + l];
        let expected = checksum(user);
        let got = self.buf[4 + l];
        if expected != got {
            self.drop_front(1);
            return Some(Err(FrameError::BadChecksum { expected, got }));
        }
        if self.buf[4 + l + 1] != END {
            self.drop_front(1);
            return Some(Err(FrameError::BadEnd));
        }

        let frame = Frame::new(&self.buf[4..4 + l]).expect("l <= 255 by length field");
        self.drop_front(total);
        Some(Ok(frame))
    }
}
