//! Generic parser for the v1 `command` contract (`{cmd, pin}`) — **HA-agnostic**.
//! Shared by REST (`/command`) and the MQTT command subscriber; HA is just one of many
//! possible senders (Node-RED, openHAB, scripts, …).

use telenot_core::ArmCommand;

use crate::dto::CommandReq;
use crate::security::{disarm_authorized, PinCheck, Services};

/// Result of command authorization. `Deny` carries a human-readable reason (for the log).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandAuth {
    Allow,
    Deny(String),
}

/// An incoming control command per the v1 contract: Arm/Disarm/Reset or zone bypass.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BridgeCommand {
    Arm(ArmCommand),
    /// Arm command for a specific Sicherungsbereich (multi-area panels, 1-based).
    ArmArea {
        cmd: ArmCommand,
        area: u8,
    },
    /// Bypass (`sperren=true`) or unbypass detection area `mb` (complex: 1–128,
    /// hiplex: 1–512; the core enforces the profile's exact limit).
    Bypass {
        mb: u16,
        sperren: bool,
    },
    /// Toggle switch output (GMS address ≥ 0x0500). Fail-closed: the core only switches
    /// addresses that appear in the `switchable` allowlist of the config.
    Output {
        addr: u16,
        on: bool,
    },
}

impl BridgeCommand {
    /// Security-reducing commands (disarm, bypass detection area) are subject to the
    /// fail-closed policy: opt-in flag + PIN required. Arm/Reset/unbypass increase or
    /// preserve security and are always allowed.
    pub fn is_security_reducing(&self) -> bool {
        matches!(
            self,
            BridgeCommand::Arm(ArmCommand::Disarm)
                | BridgeCommand::ArmArea {
                    cmd: ArmCommand::Disarm,
                    ..
                }
                | BridgeCommand::Bypass { sperren: true, .. }
        )
    }
}

/// Authorizes a **HomeKit command**. Unlike the REST/MQTT path, HomeKit does not carry a
/// per-action PIN — authorization is the **E2E pairing itself** (only a cryptographically
/// verified Apple Home accessory can write). This is therefore a **deliberately separate**
/// decision point alongside [`authorize_command`] (which covers the PIN/remote-disarm path),
/// not a bypass of it.
///
/// Fail-closed: security-reducing commands (disarm) are only allowed when explicitly enabled
/// in setup (`allow_disarm`, default OFF → HomeKit can only arm). Arm/Reset are always
/// allowed; the core additionally applies pre-arm gating (open contacts → REJECTED).
///
/// XOR control channel: HAP direct control is **only** honoured in HomeKit mode
/// (`homekit_mode`). In MQTT mode the HomeKit channel is hard-blocked here — the counterpart
/// to [`authorize_command`], which blocks the MQTT/REST channel in HomeKit mode. This
/// guarantees that exactly ONE channel can ever switch the panel, enforced at the decision
/// point (not merely by "the other subsystem isn't started").
pub fn homekit_authorized(cmd: BridgeCommand, homekit_mode: bool, allow_disarm: bool) -> bool {
    if !homekit_mode {
        return false;
    }
    if cmd.is_security_reducing() {
        allow_disarm
    } else {
        true
    }
}

/// Authorizes an incoming command against the active policy — the **single** decision point
/// shared by REST, MQTT, and live tests. Security-reducing commands (disarm, bypass) are
/// fail-closed: allowed only when the remote switch is active AND the PIN matches
/// (constant-time + lockout via [`Services`]). Arm/Reset/unbypass are always allowed;
/// the core is pre-arm-gated (open contacts → REJECTED).
pub fn authorize_command(
    cmd: BridgeCommand,
    homekit_mode: bool,
    remote_disarm: bool,
    given_pin: Option<&str>,
    services: &mut dyn Services,
) -> CommandAuth {
    // XOR control channel: in HomeKit mode the MQTT/REST channel is fully blocked — the panel
    // then accepts commands ONLY via direct HomeKit (HAP). Fail-closed and hard-enforced at
    // the single decision point (belt-and-suspenders alongside the MQTT client not being
    // started in HomeKit mode). Counterpart: `homekit_authorized` blocks HAP in MQTT mode.
    if homekit_mode {
        return CommandAuth::Deny("HomeKit-Modus aktiv — MQTT/REST-Steuerung gesperrt".into());
    }
    if !cmd.is_security_reducing() {
        return CommandAuth::Allow;
    }
    let check = match given_pin {
        Some(p) => services.verify_pin(p),
        None => PinCheck::NoPin,
    };
    if disarm_authorized(remote_disarm, check) {
        return CommandAuth::Allow;
    }
    let reason = if !remote_disarm {
        "Remote-Disarm nicht aktiviert (fail-closed)".to_string()
    } else {
        match check {
            PinCheck::Wrong => "PIN falsch".into(),
            PinCheck::NoPin => "keine PIN gesetzt".into(),
            PinCheck::Locked { retry_after_s } => format!("gesperrt (noch {retry_after_s}s)"),
            PinCheck::Ok => "abgelehnt".into(),
        }
    };
    CommandAuth::Deny(reason)
}

/// Command string → `ArmCommand` (case-insensitive; covers the MQTT contract `ARM_HOME…` + REST).
pub fn parse_arm_command(s: &str) -> Option<ArmCommand> {
    Some(match s.trim().to_ascii_lowercase().as_str() {
        "arm_away" | "away" => ArmCommand::ArmAway,
        "arm_home" | "home" => ArmCommand::ArmHome,
        "arm_night" | "night" => ArmCommand::ArmNight,
        "disarm" => ArmCommand::Disarm,
        "reset" => ArmCommand::Reset,
        _ => return None,
    })
}

/// `CommandReq` → `BridgeCommand` (shared by REST and MQTT). Bypass requires `mb`.
pub fn parse_bridge_command(req: &CommandReq) -> Result<BridgeCommand, &'static str> {
    let s = req.cmd.trim().to_ascii_lowercase();
    if let Some(arm) = parse_arm_command(&s) {
        return match req.area {
            None | Some(1) => Ok(BridgeCommand::Arm(arm)),
            Some(area) if (2..=telenot_config::AREA_ID_MAX).contains(&area) => {
                Ok(BridgeCommand::ArmArea { cmd: arm, area })
            }
            Some(_) => Err("area außerhalb 1–16"),
        };
    }
    if let Some(on) = match s.as_str() {
        "output_on" => Some(true),
        "output_off" => Some(false),
        _ => None,
    } {
        return match req.addr {
            // Only real switch outputs (excluding the system-status block — arm/bypass
            // addresses are off-limits for the non-PIN-gated output path).
            Some(addr) if telenot_config::is_switchable_addr(addr) => {
                Ok(BridgeCommand::Output { addr, on })
            }
            Some(_) => Err("addr ist kein schaltbarer Ausgang"),
            None => Err("output braucht addr"),
        };
    }
    let sperren = match s.as_str() {
        "bypass_on" => true,
        "bypass_off" => false,
        _ => return Err("unbekannter Befehl"),
    };
    match req.mb {
        // Static ceiling = hiplex maximum; the core enforces the exact profile limit
        // (complex: 128) and rejects beyond it.
        Some(mb) if (1..=512).contains(&mb) => Ok(BridgeCommand::Bypass { mb, sperren }),
        Some(_) => Err("mb außerhalb 1–512"),
        None => Err("bypass braucht mb"),
    }
}

/// Parse an incoming command message (e.g. MQTT payload).
/// **`retained` messages are discarded** (contract: a retained command is stale or abusive).
pub fn parse_command_message(
    payload: &[u8],
    retained: bool,
) -> Result<(BridgeCommand, Option<String>), &'static str> {
    if retained {
        return Err("retained command verworfen");
    }
    let req: CommandReq = serde_json::from_slice(payload).map_err(|_| "ungültiges JSON")?;
    let cmd = parse_bridge_command(&req)?;
    Ok((cmd, req.pin))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_case_insensitive() {
        assert_eq!(parse_arm_command("ARM_AWAY"), Some(ArmCommand::ArmAway));
        assert_eq!(parse_arm_command("arm_home"), Some(ArmCommand::ArmHome));
        assert_eq!(parse_arm_command(" Disarm "), Some(ArmCommand::Disarm));
        assert_eq!(parse_arm_command("RESET"), Some(ArmCommand::Reset));
        assert_eq!(parse_arm_command("explode"), None);
    }

    #[test]
    fn message_parsing_and_retained_discard() {
        let (cmd, pin) = parse_command_message(br#"{"cmd":"ARM_AWAY"}"#, false).unwrap();
        assert_eq!(cmd, BridgeCommand::Arm(ArmCommand::ArmAway));
        assert_eq!(pin, None);

        let (cmd, pin) = parse_command_message(br#"{"cmd":"disarm","pin":"4729"}"#, false).unwrap();
        assert_eq!(cmd, BridgeCommand::Arm(ArmCommand::Disarm));
        assert_eq!(pin.as_deref(), Some("4729"));

        // retained → discarded
        assert!(parse_command_message(br#"{"cmd":"DISARM"}"#, true).is_err());
        // garbage / unknown
        assert!(parse_command_message(b"not json", false).is_err());
        assert!(parse_command_message(br#"{"cmd":"nope"}"#, false).is_err());
    }

    use crate::security::InMemoryServices;

    fn svc_with_pin(pin: &str) -> InMemoryServices {
        let mut s = InMemoryServices::new("pw");
        s.set_pin(pin);
        s
    }

    #[test]
    fn arm_and_reset_always_allowed() {
        let mut s = InMemoryServices::new("pw"); // no PIN, remote off
        for cmd in [
            BridgeCommand::Arm(ArmCommand::ArmAway),
            BridgeCommand::Arm(ArmCommand::ArmHome),
            BridgeCommand::Arm(ArmCommand::ArmNight),
            BridgeCommand::Arm(ArmCommand::Reset),
            // Unbypass increases security → always allowed, same as arm.
            BridgeCommand::Bypass {
                mb: 3,
                sperren: false,
            },
        ] {
            assert_eq!(
                authorize_command(cmd, false, false, None, &mut s),
                CommandAuth::Allow
            );
        }
    }

    #[test]
    fn bypass_sperren_fail_closed_like_disarm() {
        // Bypass reduces security → same policy as remote-disarm.
        let mut s = svc_with_pin("4729");
        let on = BridgeCommand::Bypass {
            mb: 7,
            sperren: true,
        };
        assert!(matches!(
            authorize_command(on, false, false, Some("4729"), &mut s),
            CommandAuth::Deny(_)
        ));
        assert!(matches!(
            authorize_command(on, false, true, Some("0000"), &mut s),
            CommandAuth::Deny(_)
        ));
        assert_eq!(
            authorize_command(on, false, true, Some("4729"), &mut s),
            CommandAuth::Allow
        );
    }

    #[test]
    fn bypass_parsing_validates_mb() {
        let (cmd, _) =
            parse_command_message(br#"{"cmd":"BYPASS_ON","mb":3,"pin":"1"}"#, false).unwrap();
        assert_eq!(
            cmd,
            BridgeCommand::Bypass {
                mb: 3,
                sperren: true
            }
        );
        let (cmd, _) = parse_command_message(br#"{"cmd":"bypass_off","mb":128}"#, false).unwrap();
        assert_eq!(
            cmd,
            BridgeCommand::Bypass {
                mb: 128,
                sperren: false
            }
        );
        assert!(parse_command_message(br#"{"cmd":"BYPASS_ON"}"#, false).is_err());
        assert!(parse_command_message(br#"{"cmd":"BYPASS_ON","mb":0}"#, false).is_err());
        // Parser ceiling = hiplex maximum (512); the exact per-panel limit (complex: 128)
        // is enforced by the core against the active profile.
        assert!(parse_command_message(br#"{"cmd":"BYPASS_ON","mb":512}"#, false).is_ok());
        assert!(parse_command_message(br#"{"cmd":"BYPASS_ON","mb":513}"#, false).is_err());
    }

    #[test]
    fn arm_area_parsing() {
        // area absent or 1 → legacy Arm; 2..=16 → ArmArea; beyond → error.
        let (cmd, _) = parse_command_message(br#"{"cmd":"ARM_AWAY","area":1}"#, false).unwrap();
        assert_eq!(cmd, BridgeCommand::Arm(ArmCommand::ArmAway));
        let (cmd, _) = parse_command_message(br#"{"cmd":"ARM_HOME","area":3}"#, false).unwrap();
        assert_eq!(
            cmd,
            BridgeCommand::ArmArea {
                cmd: ArmCommand::ArmHome,
                area: 3
            }
        );
        assert!(parse_command_message(br#"{"cmd":"ARM_HOME","area":17}"#, false).is_err());
        // Area-targeted disarm stays security-reducing (PIN + opt-in path).
        let (cmd, _) =
            parse_command_message(br#"{"cmd":"disarm","area":2,"pin":"1"}"#, false).unwrap();
        assert!(cmd.is_security_reducing());
    }

    #[test]
    fn output_parsing_validates_addr() {
        let (cmd, _) = parse_command_message(br#"{"cmd":"OUTPUT_ON","addr":1301}"#, false).unwrap();
        assert_eq!(
            cmd,
            BridgeCommand::Output {
                addr: 0x0515,
                on: true
            }
        );
        let (cmd, _) =
            parse_command_message(br#"{"cmd":"output_off","addr":1301}"#, false).unwrap();
        assert_eq!(
            cmd,
            BridgeCommand::Output {
                addr: 0x0515,
                on: false
            }
        );
        // Missing addr / outside switch-output address space → error.
        assert!(parse_command_message(br#"{"cmd":"OUTPUT_ON"}"#, false).is_err());
        assert!(parse_command_message(br#"{"cmd":"OUTPUT_ON","addr":117}"#, false).is_err());
        assert!(parse_command_message(br#"{"cmd":"OUTPUT_ON","addr":1920}"#, false).is_err());
        // System-status block (arm/bypass addresses) is off-limits for output:
        // 0x0530 = disarm B1 (1328), 0x05F2 = MB3 bypassed (1522).
        assert!(parse_command_message(br#"{"cmd":"OUTPUT_ON","addr":1328}"#, false).is_err());
        assert!(parse_command_message(br#"{"cmd":"OUTPUT_OFF","addr":1522}"#, false).is_err());
    }

    #[test]
    fn output_commands_not_pin_gated() {
        // Switching is authorized via the config allowlist (physical setup), not by PIN —
        // the core layer fail-closes anything outside the allowlist.
        let mut s = InMemoryServices::new("pw");
        assert_eq!(
            authorize_command(
                BridgeCommand::Output {
                    addr: 0x0515,
                    on: true
                },
                false,
                false,
                None,
                &mut s
            ),
            CommandAuth::Allow
        );
    }

    #[test]
    fn disarm_fail_closed_without_remote_or_pin() {
        // Remote-disarm off → always Deny, even with the correct PIN.
        let mut s = svc_with_pin("4729");
        assert!(matches!(
            authorize_command(
                BridgeCommand::Arm(ArmCommand::Disarm),
                false,
                false,
                Some("4729"),
                &mut s
            ),
            CommandAuth::Deny(_)
        ));
        // Remote on but no PIN configured → Deny (NoPin).
        let mut s2 = InMemoryServices::new("pw");
        assert!(matches!(
            authorize_command(
                BridgeCommand::Arm(ArmCommand::Disarm),
                false,
                true,
                Some("4729"),
                &mut s2
            ),
            CommandAuth::Deny(_)
        ));
    }

    #[test]
    fn disarm_allowed_only_with_correct_pin() {
        let mut s = svc_with_pin("4729");
        assert!(matches!(
            authorize_command(
                BridgeCommand::Arm(ArmCommand::Disarm),
                false,
                true,
                Some("0000"),
                &mut s
            ),
            CommandAuth::Deny(_)
        ));
        assert_eq!(
            authorize_command(
                BridgeCommand::Arm(ArmCommand::Disarm),
                false,
                true,
                Some("4729"),
                &mut s
            ),
            CommandAuth::Allow
        );
        // No PIN in payload → Deny.
        assert!(matches!(
            authorize_command(
                BridgeCommand::Arm(ArmCommand::Disarm),
                false,
                true,
                None,
                &mut s
            ),
            CommandAuth::Deny(_)
        ));
    }

    #[test]
    fn disarm_locks_out_after_repeated_wrong_pin() {
        let mut s = svc_with_pin("4729");
        for _ in 0..5 {
            assert!(matches!(
                authorize_command(
                    BridgeCommand::Arm(ArmCommand::Disarm),
                    false,
                    true,
                    Some("0000"),
                    &mut s
                ),
                CommandAuth::Deny(_)
            ));
        }
        // Now locked out — even the CORRECT PIN is rejected (fail-closed).
        match authorize_command(
            BridgeCommand::Arm(ArmCommand::Disarm),
            false,
            true,
            Some("4729"),
            &mut s,
        ) {
            CommandAuth::Deny(reason) => assert!(reason.contains("gesperrt"), "Grund: {reason}"),
            CommandAuth::Allow => panic!("must not allow during lockout"),
        }
    }

    #[test]
    fn homekit_arm_immer_erlaubt() {
        // Arming increases security → always allowed regardless of the disarm opt-in
        // (HomeKit mode active).
        for allow in [false, true] {
            assert!(homekit_authorized(
                BridgeCommand::Arm(ArmCommand::ArmAway),
                true,
                allow
            ));
            assert!(homekit_authorized(
                BridgeCommand::Arm(ArmCommand::ArmHome),
                true,
                allow
            ));
        }
    }

    #[test]
    fn homekit_disarm_nur_mit_optin() {
        // Fail-closed: disarm only when explicitly enabled in setup (HomeKit mode active).
        assert!(!homekit_authorized(
            BridgeCommand::Arm(ArmCommand::Disarm),
            true,
            false
        ));
        assert!(homekit_authorized(
            BridgeCommand::Arm(ArmCommand::Disarm),
            true,
            true
        ));
    }

    // ---- XOR control channel: exactly ONE of MQTT/REST or direct HomeKit may switch ----

    #[test]
    fn homekit_channel_blocked_in_mqtt_mode() {
        // homekit_mode=false → the HAP channel is hard-blocked, even for arm and even if
        // disarm is opted in. HomeKit control only lives in HomeKit mode.
        for cmd in [
            BridgeCommand::Arm(ArmCommand::ArmAway),
            BridgeCommand::Arm(ArmCommand::ArmHome),
            BridgeCommand::Arm(ArmCommand::Disarm),
        ] {
            assert!(!homekit_authorized(cmd, false, false));
            assert!(!homekit_authorized(cmd, false, true));
        }
    }

    #[test]
    fn mqtt_channel_blocked_in_homekit_mode() {
        // homekit_mode=true → the MQTT/REST channel is hard-blocked regardless of PIN or
        // remote-disarm. Even a security-preserving arm is denied on this channel; control
        // belongs exclusively to direct HomeKit in this mode.
        let mut s = svc_with_pin("4729");
        for (cmd, remote, pin) in [
            (BridgeCommand::Arm(ArmCommand::ArmAway), false, None),
            (BridgeCommand::Arm(ArmCommand::ArmHome), true, Some("4729")),
            (BridgeCommand::Arm(ArmCommand::Disarm), true, Some("4729")),
        ] {
            assert!(matches!(
                authorize_command(cmd, true, remote, pin, &mut s),
                CommandAuth::Deny(_)
            ));
        }
    }
}
