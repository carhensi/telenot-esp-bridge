//! Bounded hiplex read probes, independent of inventory discovery and configuration.

use telenot_core::{Action, Tick};
use telenot_protocol::{encode_query, Frame, Function};

const RESPONSE_WINDOW_MS: u64 = 4_000;
const DEADLINE_MS: u64 = 30_000;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Stage {
    Inputs,
    Outputs,
    Text,
    Done,
}

pub(crate) struct DiagnosticProbe {
    started: Tick,
    sent_at: Option<Tick>,
    stage: Stage,
    input_address: Option<u16>,
}

impl DiagnosticProbe {
    pub(crate) fn new(now: Tick) -> Self {
        Self {
            started: now,
            sent_at: None,
            stage: Stage::Inputs,
            input_address: None,
        }
    }

    pub(crate) fn tick(&mut self, now: Tick, actions: &mut Vec<Action>) {
        if self.stage != Stage::Done && now.saturating_sub(self.started) >= DEADLINE_MS {
            self.stage = Stage::Done;
            actions.push(Action::Log(
                "hiplex-Lesediagnose: Zeitlimit erreicht".into(),
            ));
        }
    }

    /// Returns true only when a query occupies this SEND_NORM window instead of its ACK.
    pub(crate) fn on_frame(&mut self, now: Tick, frame: &Frame, actions: &mut Vec<Action>) -> bool {
        self.tick(now, actions);
        if self.stage == Stage::Done {
            return false;
        }
        // Only occupancy received after our input query can supply a text-query address.
        // The existing encoder can address device 0 only. Do not flatten other devices.
        if self.stage == Stage::Inputs && self.sent_at.is_some() {
            for record in frame.records().filter_map(Result::ok) {
                if let Some(block) = record.as_block_status() {
                    if block.adresserweiterung != 0x71 || block.geraet_bereich != 0 {
                        continue;
                    }
                    for (i, byte) in block.status.iter().enumerate() {
                        for bit in 0..8 {
                            if byte & (1 << bit) == 0 {
                                if let Some(address) =
                                    block.base_address().checked_add((i * 8 + bit) as u16)
                                {
                                    self.input_address = Some(
                                        self.input_address.map_or(address, |a| a.min(address)),
                                    );
                                }
                            }
                        }
                    }
                }
            }
        }
        if frame.function() != Some(Function::SendNorm) {
            return false;
        }
        if let Some(sent) = self.sent_at {
            // Keep the full response window, including after ACK/NAK/error or partial blocks.
            if now.saturating_sub(sent) < RESPONSE_WINDOW_MS {
                return false;
            }
            self.stage = match self.stage {
                Stage::Inputs => Stage::Outputs,
                Stage::Outputs if self.input_address.is_some() => Stage::Text,
                _ => Stage::Done,
            };
            self.sent_at = None;
        }
        let (address, extension, kind, label) = match self.stage {
            Stage::Inputs => (0, 0x71, 0x24, "Eingangsbelegung"),
            Stage::Outputs => (0, 0x72, 0x24, "separate Ausgangsbelegung"),
            Stage::Text => (
                self.input_address
                    .expect("text stage requires observed address"),
                0x73,
                0x0C,
                "Text einer belegten Eingangsadresse",
            ),
            Stage::Done => {
                actions.push(Action::Log(if self.input_address.is_some() {
                    "hiplex-Lesediagnose beendet: drei Leseabfragen; Antworten im RX/TX-Mitschnitt".into()
                } else {
                    "hiplex-Lesediagnose beendet: keine belegte Eingangsadresse erkannt, Textabfrage ausgelassen".into()
                }));
                return false;
            }
        };
        let mut buffer = [0; 32];
        if let Ok(length) = encode_query(address, extension, kind, &mut buffer) {
            self.sent_at = Some(now);
            actions.push(Action::SendFrame(buffer[..length].to_vec()));
            actions.push(Action::Log(format!("hiplex-Lesediagnose: {label}")));
            return true;
        }
        self.stage = Stage::Done;
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn poll() -> Frame {
        Frame::new(&[0x40, 2]).unwrap()
    }

    fn occupied(device: u8, extension: u8) -> Frame {
        Frame::new(&[0x73, 2, 5, 0x24, device, 1, 0, extension, 0xFE]).unwrap()
    }

    fn query(probe: &mut DiagnosticProbe, now: Tick) -> Option<Vec<u8>> {
        let mut actions = Vec::new();
        let sent = probe.on_frame(now, &poll(), &mut actions);
        let frames: Vec<_> = actions
            .into_iter()
            .filter_map(|a| match a {
                Action::SendFrame(bytes) => Some(bytes),
                _ => None,
            })
            .collect();
        assert_eq!(frames.len(), usize::from(sent));
        frames.into_iter().next().map(|bytes| {
            let mut decoder = telenot_protocol::FrameDecoder::new();
            decoder.feed(&bytes);
            let frame = decoder.next_frame().unwrap().unwrap();
            let records: Vec<_> = frame.records().collect::<Result<_, _>>().unwrap();
            assert_eq!(records.len(), 1);
            assert_eq!(records[0].satztyp, 0x10, "only read query records");
            records[0].payload.to_vec()
        })
    }

    #[test]
    fn missing_outputs_does_not_block_single_text_probe() {
        let mut probe = DiagnosticProbe::new(0);
        assert_eq!(query(&mut probe, 0), Some(vec![0, 0, 0, 0x71, 0x24]));
        assert!(!probe.on_frame(1_000, &occupied(0, 0x71), &mut Vec::new()));
        assert!(query(&mut probe, 3_999).is_none());
        assert_eq!(query(&mut probe, 4_000), Some(vec![0, 0, 0, 0x72, 0x24]));
        for frame in [
            Frame::new(&[0, 2]).unwrap(),
            Frame::new(&[1, 2]).unwrap(),
            Frame::new(&[0x73, 2, 2, 0x11, 0, 0x18]).unwrap(),
        ] {
            assert!(!probe.on_frame(4_100, &frame, &mut Vec::new()));
        }
        assert!(query(&mut probe, 7_999).is_none());
        assert_eq!(query(&mut probe, 8_000), Some(vec![0, 1, 0, 0x73, 0x0C]));
        assert!(query(&mut probe, 12_000).is_none());
        assert!(query(&mut probe, 20_000).is_none());
    }

    #[test]
    fn no_input_occupancy_means_no_guessed_text_address() {
        let mut probe = DiagnosticProbe::new(0);
        // Pre-query occupancy, live status and other device namespaces must not seed a probe.
        probe.on_frame(0, &occupied(0, 0x71), &mut Vec::new());
        assert!(query(&mut probe, 1).is_some());
        probe.on_frame(10, &occupied(0, 0x01), &mut Vec::new());
        probe.on_frame(20, &occupied(1, 0x71), &mut Vec::new());
        assert_eq!(query(&mut probe, 4_001), Some(vec![0, 0, 0, 0x72, 0x24]));
        probe.on_frame(5_000, &occupied(0, 0x71), &mut Vec::new());
        assert!(query(&mut probe, 8_001).is_none());
    }

    #[test]
    fn no_polls_or_expired_deadline_never_send() {
        let mut probe = DiagnosticProbe::new(0);
        let mut actions = Vec::new();
        probe.on_frame(1_000, &occupied(0, 0x71), &mut actions);
        probe.tick(30_000, &mut actions);
        assert!(!actions.iter().any(|a| matches!(a, Action::SendFrame(_))));
        assert!(query(&mut probe, 30_001).is_none());
    }
}
