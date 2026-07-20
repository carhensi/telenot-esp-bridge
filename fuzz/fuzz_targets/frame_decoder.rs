//! Fuzzes the streaming FT1.2 decoder: feeds arbitrary bytes in randomly-sized
//! chunks (TCP may fragment arbitrarily), collects all frames, and validates each
//! decoded frame via an encode→decode round-trip. Also covers the
//! `expect("l <= 255 by length field")` invariant in frame.rs.
#![no_main]

use libfuzzer_sys::fuzz_target;
use telenot_protocol::{encode_frame, FrameDecoder, MAX_USER_DATA};

fuzz_target!(|data: &[u8]| {
    let mut dec = FrameDecoder::new();
    // Derive chunk size from the input so the fuzzer also exercises
    // fragmentation and resync paths.
    let chunk = (data.first().copied().unwrap_or(0) as usize % 17) + 1;
    for part in data.chunks(chunk) {
        dec.feed(part);
        while let Some(res) = dec.next_frame() {
            let Ok(frame) = res else { continue };
            let mut out = [0u8; MAX_USER_DATA + 6];
            let n = encode_frame(frame.user_data(), &mut out)
                .expect("dekodierter Frame muss enkodierbar sein");
            let mut dec2 = FrameDecoder::new();
            dec2.feed(&out[..n]);
            let redecoded = dec2
                .next_frame()
                .expect("complete frame fed")
                .expect("enkodierter Frame muss valide dekodieren");
            assert_eq!(redecoded.user_data(), frame.user_data(), "Round-Trip-Drift");
        }
    }
});
