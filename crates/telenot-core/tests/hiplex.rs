//! Host tests for the PROVISIONAL hiplex 8400H profile (multi-area core + command gate).
//!
//! All bit positions are COMPUTED from `profile::HIPLEX8400` — when a forum address is
//! corrected after the first capture (constants-only change), these tests keep testing
//! the right thing instead of a stale literal.

use telenot_config::{Config, PanelKind};
use telenot_core::{profile::HIPLEX8400, Action, ArmCommand, ArmState, Core, CoreOptions};
use telenot_protocol::{
    encode_command_02, encode_frame, satztyp, Frame, FrameDecoder, ART_UNSCHARF, A_QUERY,
    C_SEND_NDAT, C_SEND_NORM, ERW_AUSGAENGE,
};

fn hiplex_core(cmds_verified: bool, disarm: bool) -> Core {
    let mut cfg = Config::default();
    cfg.panel.kind = PanelKind::Hiplex8400;
    cfg.panel.hiplex_cmds_verified = cmds_verified;
    Core::new(
        std::sync::Arc::new(cfg),
        CoreOptions {
            disarm_enabled: disarm,
        },
    )
}

fn decode(bytes: &[u8]) -> Frame {
    let mut d = FrameDecoder::new();
    d.feed(bytes);
    d.next_frame().unwrap().unwrap()
}

/// VdS-2465 record: `[record_len=payload.len()][record_type][payload…]`.
fn record(satztyp: u8, payload: &[u8]) -> Vec<u8> {
    let mut v = vec![payload.len() as u8, satztyp];
    v.extend_from_slice(payload);
    v
}

fn ndat(records: &[u8]) -> Frame {
    let mut ud = vec![C_SEND_NDAT, 0x02];
    ud.extend_from_slice(records);
    let mut out = vec![0u8; ud.len() + 8];
    let n = encode_frame(&ud, &mut out).unwrap();
    decode(&out[..n])
}

fn send_norm() -> Frame {
    let ud = [C_SEND_NORM, A_QUERY];
    let mut out = [0u8; 16];
    let n = encode_frame(&ud, &mut out).unwrap();
    decode(&out[..n])
}

/// 0x24 block status frame starting at `base`, `len_bytes` of status, with the given
/// absolute addresses ACTIVE (protocol polarity: bit `'0'` = active).
fn block_frame(base: u16, len_bytes: usize, active_addrs: &[u16]) -> Frame {
    let mut status = vec![0xFFu8; len_bytes];
    for &addr in active_addrs {
        assert!(addr >= base);
        let off = (addr - base) as usize;
        assert!(off < len_bytes * 8, "address outside synthetic block");
        status[off / 8] &= !(1 << (off % 8));
    }
    let mut payload = vec![0x00, (base >> 8) as u8, (base & 0xFF) as u8, ERW_AUSGAENGE];
    payload.extend_from_slice(&status);
    ndat(&record(satztyp::BLOCKSTATUS, &payload))
}

fn publishes(actions: &[Action]) -> Vec<(String, String)> {
    actions
        .iter()
        .filter_map(|a| match a {
            Action::Publish { topic, payload, .. } => Some((topic.clone(), payload.clone())),
            _ => None,
        })
        .collect()
}

#[test]
fn multi_area_states_derive_and_aggregate() {
    let p = &HIPLEX8400;
    let mut c = hiplex_core(false, false);
    // Block over the whole area window: area 1 disarmed, area 3 armed-away,
    // area 16 (Zentralen-Schutzbereich) triggered; the rest idle/unknown.
    let area_window = (p.areas.count as usize * p.areas.stride as usize).div_ceil(8);
    let f = block_frame(
        p.areas.base,
        area_window,
        &[
            p.addr_unscharf(1),
            p.addr_extern_scharf(3),
            p.addr_alarm(16),
        ],
    );
    let actions = c.on_frame(1_000, &f);
    let pubs = publishes(&actions);

    assert!(pubs.contains(&("area/1/state".into(), "DISARMED".into())));
    assert!(pubs.contains(&("area/3/state".into(), "ARMED_AWAY".into())));
    assert!(pubs.contains(&("area/16/state".into(), "TRIGGERED".into())));
    // Severity aggregate on the legacy top-level topic: triggered dominates.
    assert!(pubs.contains(&("state".into(), "TRIGGERED".into())));
    assert_eq!(c.arm_state(), ArmState::Triggered);
    // Per-area readiness topics exist (multi-area layout), legacy `ready/*` does not.
    assert!(pubs.iter().any(|(t, _)| t == "area/1/ready/intern"));
    assert!(!pubs.iter().any(|(t, _)| t == "ready/intern"));
    assert_eq!(c.area_states().get(&3).unwrap().arm, ArmState::ArmedAway);
}

#[test]
fn complex_profile_publishes_no_area_topics() {
    let c400 = telenot_core::COMPLEX400;
    let mut c = Core::new(
        std::sync::Arc::new(Config::default()),
        CoreOptions::default(),
    );
    let f = block_frame(c400.areas.base, 8, &[c400.addr_unscharf(1)]);
    let pubs = publishes(&c.on_frame(1_000, &f));
    assert!(pubs.contains(&("state".into(), "DISARMED".into())));
    assert!(
        !pubs.iter().any(|(t, _)| t.starts_with("area/")),
        "complex must keep the historical topic set: {pubs:?}"
    );
}

#[test]
fn mb_bypass_readback_covers_512_areas() {
    let p = &HIPLEX8400;
    let mut c = hiplex_core(false, false);
    // Last 64-byte window of the bypass range: MB 512 bypassed.
    let f = block_frame(p.mb_gesperrt_base, 64, &[p.addr_mb_gesperrt(512)]);
    let pubs = publishes(&c.on_frame(1_000, &f));
    assert!(pubs.contains(&("mb/512/bypassed".into(), "ON".into())));
    assert_eq!(c.mb_bypassed().get(&512), Some(&true));
    assert!(c.mb_bypassed().get(&513).is_none());
}

#[test]
fn unverified_hiplex_rejects_commands_visibly() {
    let mut c = hiplex_core(false, true);
    let actions = c.on_command(0, ArmCommand::Disarm);
    let pubs = publishes(&actions);
    assert!(
        pubs.iter().any(|(t, pl)| t == "command_result"
            && pl.starts_with("DENIED Disarm: hiplex-Befehlsadressen unverifiziert")),
        "gate must reject visibly: {pubs:?}"
    );
    // And nothing may reach the wire in the next send window.
    let actions = c.on_frame(100, &send_norm());
    assert_eq!(
        actions
            .iter()
            .filter(|a| matches!(a, Action::SendFrame(f) if f.len() > 8))
            .count(),
        0,
        "no command frame may be sent while unverified"
    );
    // Bypass and output paths run through the same gate.
    let pubs = publishes(&c.on_bypass(0, 5, false));
    assert!(pubs
        .iter()
        .any(|(t, pl)| t == "command_result" && pl.starts_with("DENIED BypassOff MB5")));
}

#[test]
fn verified_hiplex_encodes_commands_from_profile_addresses() {
    let p = &HIPLEX8400;
    let mut c = hiplex_core(true, true);
    let actions = c.on_command(0, ArmCommand::Disarm);
    assert!(
        publishes(&actions).is_empty(),
        "verified: no DENIED expected"
    );
    // Send window → exactly the telegram encoded from the (provisional) profile address.
    let actions = c.on_frame(100, &send_norm());
    let sent: Vec<_> = actions
        .iter()
        .filter_map(|a| match a {
            Action::SendFrame(f) => Some(f.clone()),
            _ => None,
        })
        .collect();
    let mut buf = [0u8; 32];
    let n = encode_command_02(p.addr_unscharf(1), ERW_AUSGAENGE, ART_UNSCHARF, &mut buf).unwrap();
    assert_eq!(sent, vec![buf[..n].to_vec()]);
}

#[test]
fn night_mode_is_area1_only() {
    let mut c = hiplex_core(true, false);
    let pubs = publishes(&c.on_command_area(0, ArmCommand::ArmNight, 2));
    assert!(pubs
        .iter()
        .any(|(t, pl)| t == "command_result" && pl.starts_with("DENIED ArmNight B2")));
}

#[test]
fn area_id_bounds_are_enforced() {
    let mut c = hiplex_core(true, false);
    let pubs = publishes(&c.on_command_area(0, ArmCommand::ArmAway, 17));
    assert!(pubs
        .iter()
        .any(|(t, pl)| t == "command_result" && pl.contains("Bereich 17 außerhalb 1–16")));
}

#[test]
fn lite_discovery_synthesizes_mb_sensors() {
    use telenot_core::Discovery;
    let p = &HIPLEX8400;
    let mut d = Discovery::new();
    // Occupancy: MB 1 and MB 400 occupied inside the MB-status window, plus one
    // named regular input at 0x0010.
    let mb1 = p.mb_status_base;
    let mb400 = p.mb_status_base + 399;
    let mut status = vec![0xFFu8; 64];
    for addr in [mb1, mb400] {
        let off = (addr - p.mb_status_base) as usize;
        status[off / 8] &= !(1 << (off % 8));
    }
    d.ingest_belegt(&telenot_protocol::BlockStatus {
        geraet_bereich: 0,
        adresse: (p.mb_status_base >> 8) as u8,
        adressenzusatz: (p.mb_status_base & 0xFF) as u8,
        adresserweiterung: 0x72,
        status: &status,
    });
    d.ingest_belegt(&telenot_protocol::BlockStatus {
        geraet_bereich: 0,
        adresse: 0,
        adressenzusatz: 0,
        adresserweiterung: 0x71,
        status: &[0xFE], // 0x0000 occupied
    });
    d.ingest_text(0x0000, "MK Haustür");

    let panel = telenot_config::PanelSettings {
        kind: PanelKind::Hiplex8400,
        ..Default::default()
    };
    let cfg = d.into_config_for(p, &panel);

    // Named input survives as usual; unnamed MBs get generic entries (lite: no Klartexte).
    assert!(cfg.sensors.iter().any(|s| s.address() == 0x0000));
    let mb = cfg.sensors.iter().find(|s| s.address() == mb400).unwrap();
    assert_eq!(mb.name(), "MB 400");
    assert_eq!(mb.topic(), "mb_400");
    assert!(!mb.confirmed(), "MB sensors start unconfirmed");
    assert!(cfg
        .sensors
        .iter()
        .any(|s| s.address() == mb1 && s.name() == "MB 1"));
    // Panel selection is carried, a discovery commit must not reset it.
    assert_eq!(cfg.panel.kind, PanelKind::Hiplex8400);
}

#[test]
fn schaltaktion_requires_known_base() {
    // Provisional hiplex profile has no Schaltaktion base yet → visible DENIED.
    let mut c = hiplex_core(true, true);
    let pubs = publishes(&c.on_schaltaktion(0, 3, true));
    assert!(pubs
        .iter()
        .any(|(t, pl)| t == "command_result" && pl.contains("Basisadresse unbekannt")));
}
