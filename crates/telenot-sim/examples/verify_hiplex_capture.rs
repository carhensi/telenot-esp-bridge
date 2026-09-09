//! Offline read-only replay of raw RX or GMSDIAG1 captures through the production core.
//! No network, no commands, no sensor names in output.
use std::{error::Error, sync::Arc};
use telenot_config::{Config, GmsVariant, PanelKind};
use telenot_core::{Action, Core, CoreOptions};
use telenot_protocol::FrameDecoder;

fn show(now: u64, actions: Vec<Action>) {
    for action in actions {
        if let Action::Publish { topic, payload, .. } = action {
            if topic == "state" {
                println!("{now} ms: {payload}");
            }
        }
    }
}

fn main() -> Result<(), Box<dyn Error>> {
    let path = std::env::args().nth(1).ok_or("capture path required")?;
    let data = std::fs::read(path)?;
    let mut config = Config::default();
    config.panel.kind = PanelKind::Hiplex8400;
    config.panel.gms_variant = GmsVariant::Plus;
    let mut core = Core::new(Arc::new(config), CoreOptions::default());
    let mut decoder = FrameDecoder::new();
    let mut frames = 0;
    let mut errors = 0;
    let mut last = 0;
    let mut feed = |now, bytes: &[u8]| {
        while last + 100 < now {
            last += 100;
            show(last, core.on_tick(last));
        }
        last = now;
        decoder.feed(bytes);
        while let Some(frame) = decoder.next_frame() {
            match frame {
                Ok(frame) => {
                    frames += 1;
                    show(now, core.on_frame(now, &frame));
                }
                Err(_) => errors += 1,
            }
        }
    };
    if data.starts_with(b"GMSDIAG1") {
        let mut pos = 8;
        while pos < data.len() {
            if pos + 11 > data.len() {
                return Err("truncated trace header".into());
            }
            let direction = data[pos];
            let now = u64::from_le_bytes(data[pos + 1..pos + 9].try_into()?);
            let len = u16::from_le_bytes(data[pos + 9..pos + 11].try_into()?) as usize;
            pos += 11;
            if pos + len > data.len() {
                return Err("truncated trace payload".into());
            }
            if direction == 0 {
                feed(now, &data[pos..pos + len]);
            }
            pos += len;
        }
    } else {
        // Raw captures have no timestamps: only decoding and transitions are verified.
        for (i, chunk) in data.chunks(128).enumerate() {
            feed(i as u64, chunk);
        }
    }
    println!(
        "frames={frames} errors={errors} final={:?}",
        core.arm_state()
    );
    if errors != 0 {
        return Err("invalid frames".into());
    }
    Ok(())
}
