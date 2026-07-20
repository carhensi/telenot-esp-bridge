//! Configuration of the Telenot bridge: sensor/area schema, validation, migration.
//!
//! Addresses are stored as **canonical 16-bit GMS addresses** (verified against real
//! panel captures) — not as the group-relative hex values from the old Node config. The PIN for the
//! command path is **deliberately not part of this schema** (it lives in encrypted NVS)
//! so it can never end up in a config export or backup.

use serde::{Deserialize, Serialize};

pub mod chunked;
mod legacy;
pub use legacy::LegacyImport;
pub mod table;
pub use table::{SensorMut, SensorRef, SensorTable};

/// Current schema version. Increment on structural breaks and add a migration step.
pub const CURRENT_SCHEMA_VERSION: u32 = 1;

/// Maximum number of sensors the device can persist and load reliably. Derived from the
/// chunked A/B storage on the 128 KB `cfg` partition: two postcard snapshots (A + B) of
/// ~48 KB each at 600 sensors fit the ~110 KB usable partition space; (de)serialization
/// streams through an 8 KB chunk buffer, so no full-size contiguous allocation exists.
/// RAM: steady state holds ONE compact sensor table (~≤85 KB incl. string arena), the
/// commit path briefly peaks at two tables. Beyond 600 (GMS plus full expansion) the
/// string arena would need lazy flash-backed strings first — see docs/hiplex.md (H3).
pub const MAX_SENSORS: usize = 600;

/// GMS output address range (verified against real panel captures): 0x0500 (master outputs) through
/// 0x077F inclusive (comlock410 addr. 15). Single source of truth for the parser, validation,
/// PATCH gate, and HA discovery — do not duplicate as a literal elsewhere.
pub const OUTPUT_ADDR_RANGE: core::ops::Range<u16> = 0x0500..0x0780;

/// System status block within the output address range: security area status (0x0530+),
/// detection area status (0x0570+) and "detection area blocked" (0x05F0–0x066F). These addresses
/// carry arm state/bypass — they are readable but NEVER switchable.
pub const STATUS_ADDR_RANGE: core::ops::Range<u16> = 0x0530..0x0670;

/// Is this address permitted as a switch output (`switchable`, `OUTPUT_ON/OFF`)?
/// Deliberately narrower than the output address range: the status block is excluded so the
/// (non-PIN-gated) output path can never send telegrams to arm/bypass addresses —
/// defence-in-depth regardless of how the panel handles unknown message types there.
pub fn is_switchable_addr(addr: u16) -> bool {
    OUTPUT_ADDR_RANGE.contains(&addr) && !STATUS_ADDR_RANGE.contains(&addr)
}

/// Complete device configuration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Config {
    pub schema_version: u32,
    #[serde(default)]
    pub sensors: SensorTable,
    /// Panel selection + variant. Additive: absent in older configs → complex 400H
    /// defaults, bit-identical to pre-profile behavior.
    #[serde(default)]
    pub panel: PanelSettings,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            schema_version: CURRENT_SCHEMA_VERSION,
            sensors: SensorTable::new(),
            panel: PanelSettings::default(),
        }
    }
}

/// Which panel the bridge talks to, and how.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PanelSettings {
    #[serde(default)]
    pub kind: PanelKind,
    /// GMS protocol level — only meaningful for the hiplex (the complex 400 has no
    /// GMS plus); ignored for `Complex400`.
    #[serde(default)]
    pub gms_variant: GmsVariant,
    /// Safety gate for the hiplex COMMAND path: the hiplex addresses are forum-sourced
    /// and unverified until the first real capture. `false` (default) = arm/bypass/
    /// Schaltaktion commands are rejected visibly (`DENIED`); reading/mirroring is
    /// never gated. Set only after the capture diff confirmed the address map.
    #[serde(default)]
    pub hiplex_cmds_verified: bool,
    /// Optional display names per Sicherungsbereich (hiplex; id 16 = Zentralen-
    /// Schutzbereich). Areas without an entry get a generic name.
    #[serde(default)]
    pub areas: Vec<AreaCfg>,
    /// Eager-Send: fire an arm/disarm/output command immediately when the serial line is
    /// idle, instead of waiting for the panel's ~3 s SEND_NORM poll window. A collision or
    /// missing ACK falls back to the reliable poll-window slot (existing retry) — so the
    /// worst case equals the poll-gated behavior, the best case is ~instant. Default ON.
    /// Set to `false` to fall back to strict poll-window-only sending.
    #[serde(default = "default_true")]
    pub eager_send: bool,
}

impl Default for PanelSettings {
    fn default() -> Self {
        Self {
            kind: PanelKind::default(),
            gms_variant: GmsVariant::default(),
            hiplex_cmds_verified: false,
            areas: Vec::new(),
            eager_send: true,
        }
    }
}

/// serde default for `eager_send` (bool's own default is `false`, but we want ON).
fn default_true() -> bool {
    true
}

/// Supported panels (serialized form; domain enum lives in `telenot-core::profile`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PanelKind {
    #[default]
    Complex400,
    Hiplex8400,
}

/// GMS protocol level of the hiplex (lite = Meldebereich-only, plus = Meldepunkte +
/// Klartexte @ 115200 Baud).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GmsVariant {
    #[default]
    Lite,
    Plus,
}

/// Display name for a Sicherungsbereich.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AreaCfg {
    /// 1-based; complex: 1, hiplex: 1–15 + 16 = Zentralen-Schutzbereich.
    pub id: u8,
    pub name: String,
}

/// Maximum area id across profiles (hiplex: 15 Sicherungsbereiche + Zentralen-Schutzbereich).
pub const AREA_ID_MAX: u8 = 16;

/// A configured detection point / status point.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Sensor {
    /// Canonical 16-bit GMS address, e.g. `0x0532` = external arm area 1.
    pub address: u16,
    /// Internal short name.
    pub name: String,
    /// Display name (Home Assistant).
    pub name_ha: String,
    pub kind: SensorKind,
    /// MQTT subtopic path relative to the device root.
    pub topic: String,
    #[serde(default)]
    pub polarity: Polarity,
    /// `false` for discovery-generated entries until confirmed in the setup web UI →
    /// treated as fail-safe (unknown) until then.
    #[serde(default)]
    pub confirmed: bool,
    /// Opt-in: output may be switched via the command path (`OUTPUT_ON/OFF`).
    /// Default `false` (fail-closed allowlist); only valid for output addresses (≥ 0x0500).
    #[serde(default)]
    pub switchable: bool,
    /// Opt-in: this detector is exposed in direct-HomeKit mode as its own accessory (contact/
    /// motion/smoke sensor depending on `kind`). Default `false` — only confirmed, flagged
    /// detectors are mirrored (Apple Home limit ~150 accessories).
    #[serde(default)]
    pub show_in_homekit: bool,
}

/// Bit polarity of a status point. GMS default: bit `'0'` = active/on/triggered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Polarity {
    /// Protocol default: bit `'0'` = active. Default.
    #[default]
    ActiveLow,
    /// Inverted: bit `'1'` = active (real hardware deviation, e.g. some security areas).
    ActiveHigh,
    /// Discovery-generated; polarity not yet confirmed → fail-safe (unknown).
    Unconfirmed,
}

impl Polarity {
    /// Logical "active/on" state derived from the raw status bit. `None` = unconfirmed/fail-safe.
    pub fn is_active(&self, raw_bit: bool) -> Option<bool> {
        match self {
            Polarity::ActiveLow => Some(!raw_bit),
            Polarity::ActiveHigh => Some(raw_bit),
            Polarity::Unconfirmed => None,
        }
    }
}

/// Sensor/point type. `device_class()` returns the Home Assistant equivalent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SensorKind {
    Bewegungsmelder,
    Magnetkontakt,
    Schliesskontakt,
    Rauchmelder,
    Wassermelder,
    Ueberfallmelder,
    Gehaeuse,
    Signalgeber,
    Sabotage,
    Systemstatus,
    Sicherung,
    // Diagnostic types (GMS message types): battery 0x33, mains 0x32, transmission 0x34,
    // fault 0x30, technical 0x40/0x41 → standard HA device_classes battery/power/connectivity/problem.
    Batterie,
    Netzstoerung,
    Uebertragung,
    Stoerung,
    Technik,
    // Bus participants (not room sensors): keypad (BT physical, BuildSec = Telenot app/remote
    // keypad) + building/access technology (comlock/CL line). Diagnostic; generically named
    // slots (BT-Addr./BuildSec-Addr.) are hidden by default in the setup UI.
    Bedienteil,
    Gebaeudetechnik,
    Unbekannt,
}

impl SensorKind {
    /// Home Assistant `device_class` (matches the legacy bridge's mapping).
    pub fn device_class(&self) -> &'static str {
        match self {
            SensorKind::Bewegungsmelder => "motion",
            SensorKind::Magnetkontakt => "window",
            SensorKind::Schliesskontakt => "door",
            SensorKind::Rauchmelder => "smoke",
            SensorKind::Wassermelder => "moisture",
            SensorKind::Ueberfallmelder => "safety",
            SensorKind::Gehaeuse => "tamper",
            SensorKind::Signalgeber => "sound",
            SensorKind::Sabotage => "tamper",
            SensorKind::Systemstatus => "problem",
            SensorKind::Sicherung => "safety",
            SensorKind::Batterie => "battery",
            SensorKind::Netzstoerung => "power",
            SensorKind::Uebertragung => "connectivity",
            SensorKind::Stoerung => "problem",
            SensorKind::Technik => "problem",
            SensorKind::Bedienteil => "connectivity",
            SensorKind::Gebaeudetechnik => "connectivity",
            SensorKind::Unbekannt => "problem",
        }
    }

    /// Maps the `type` string from the old Node config to a `SensorKind`.
    pub fn from_legacy_type(t: &str) -> SensorKind {
        match t {
            "bewegungsmelder" => SensorKind::Bewegungsmelder,
            "magnetkontakt" => SensorKind::Magnetkontakt,
            "schliesskontakt" => SensorKind::Schliesskontakt,
            "rauchmelder" => SensorKind::Rauchmelder,
            "wassermelder" => SensorKind::Wassermelder,
            "ueberfallmelder" => SensorKind::Ueberfallmelder,
            "gehaeuse" => SensorKind::Gehaeuse,
            "signalgeber" => SensorKind::Signalgeber,
            "sabotage" => SensorKind::Sabotage,
            "systemstatus" => SensorKind::Systemstatus,
            "sicherung" => SensorKind::Sicherung,
            "bedienteil" => SensorKind::Bedienteil,
            "gebaeudetechnik" => SensorKind::Gebaeudetechnik,
            _ => SensorKind::Unbekannt,
        }
    }

    /// Heuristic for auto-discovery: guesses the type from the panel's plain-text name.
    /// GMS provides no type — the result is a suggestion to be confirmed in the setup web UI.
    pub fn guess_from_name(name: &str) -> SensorKind {
        let n = name.trim().to_lowercase();
        let has = |needle: &str| n.contains(needle);

        let head = n.split([' ', '-', '.', ':', '_']).next().unwrap_or("");

        // 1. Override words win over EVERYTHING (including abbreviations): "ESG Gehäuse" → Gehaeuse,
        //    "Sabotage MB03 ISG" → Sabotage.
        if has("gehäuse") || has("gehaeuse") || has("deckel") {
            return SensorKind::Gehaeuse;
        }
        if has("sabotage") {
            return SensorKind::Sabotage;
        }

        // 2. Contact abbreviation IS the decision (real installer config: "MK Tür" → Magnetkontakt,
        //    "SK Tür" → Schliesskontakt — the prefix distinguishes opening from bolt contact, not the
        //    word "Tür"; both are on the same door). Content words only as fallback (step 4).
        match head {
            "mk" => return SensorKind::Magnetkontakt,
            "sk" | "rk" => return SensorKind::Schliesskontakt,
            _ => {}
        }

        // 3. Further abbreviations at the start of the name (token-exact, so "RM" doesn't match inside "Warmwasser").
        //    comlock access modules: "CL0-DK", "CL410-1 Garage" → head "cl0"/"cl410".
        if let Some(rest) = head.strip_prefix("cl") {
            if rest.starts_with(|c: char| c.is_ascii_digit()) {
                return SensorKind::Gebaeudetechnik;
            }
        }
        match head {
            "rm" => return SensorKind::Rauchmelder,
            "bm" | "im" | "ir" | "pir" => return SensorKind::Bewegungsmelder,
            "wm" => return SensorKind::Wassermelder,
            "üf" | "uf" => return SensorKind::Ueberfallmelder,
            // External/internal/acoustic/optical signal sounder.
            "asg" | "osg" | "esg" | "isg" | "sg" => return SensorKind::Signalgeber,
            "gk" => return SensorKind::Gehaeuse,
            // BT = physical keypad; BuildSec = Telenot app (remote keypad, appears as a BT slot in scan).
            // Both → Bedienteil. Gebaeudetechnik stays for the physical comlock line.
            "bt" | "buildsec" => return SensorKind::Bedienteil,
            "ue" | "üe" => return SensorKind::Uebertragung,
            _ => {}
        }

        // 4a. English type keywords (English-named installations / demo) — the type in the name wins
        //     before German content words apply (which wouldn't match English names anyway).
        if has("motion") {
            return SensorKind::Bewegungsmelder;
        }
        if has("smoke") || has("fire") {
            return SensorKind::Rauchmelder;
        }
        if has("leak") || has("water") || has("flood") {
            return SensorKind::Wassermelder;
        }
        if has("siren") || has("sounder") {
            return SensorKind::Signalgeber;
        }
        if has("panic") || has("hold-up") || has("holdup") {
            return SensorKind::Ueberfallmelder;
        }
        if has("glass") || has("contact") {
            return SensorKind::Magnetkontakt;
        }

        // 4. Content keywords when no decisive abbreviation was present (e.g. "Haustür", "Wassermelder").
        if has("fenster") {
            return SensorKind::Magnetkontakt;
        }
        if has("tür") || has("tuer") || has("türe") {
            return SensorKind::Schliesskontakt;
        }
        if has("wasser") {
            return SensorKind::Wassermelder;
        }
        if has("rauch") || has("brand") {
            return SensorKind::Rauchmelder;
        }
        if has("bewegung") || has("infrarot") {
            return SensorKind::Bewegungsmelder;
        }
        if has("sirene")
            || has("summer")
            || has("signalgeber")
            || has("akustisch")
            || has("optisch")
            || has("blitz")
        {
            return SensorKind::Signalgeber;
        }
        if has("überfall") || has("ueberfall") {
            return SensorKind::Ueberfallmelder;
        }
        if has("bedient") {
            return SensorKind::Bedienteil;
        }
        if has("comlock") {
            return SensorKind::Gebaeudetechnik;
        }
        if has("akku") || has("batterie") || has("bat.") {
            return SensorKind::Batterie;
        }
        if has("netz") {
            return SensorKind::Netzstoerung;
        }
        if has("übertrag") || has("uebertrag") {
            return SensorKind::Uebertragung;
        }
        if has("technik") {
            return SensorKind::Technik;
        }
        if has("stör") || has("stoer") {
            return SensorKind::Stoerung;
        }
        if has("scharf") || has("unscharf") || has("bereit") {
            return SensorKind::Systemstatus;
        }
        SensorKind::Unbekannt
    }
}

/// Generically named bus slot (panel auto-placeholder such as "BT-Adr. 03" / "BuildSec-Adr.
/// 12") — mostly unused. These are hidden by default in the setup UI (avoids HA clutter);
/// an *installer-named* keypad ("BT Haustür") remains visible.
pub fn is_generic_bus_label(name: &str) -> bool {
    let n = name.trim().to_lowercase();
    n.starts_with("bt-adr") || n.starts_with("bt adr") || n.starts_with("buildsec")
}

/// Error while loading the configuration.
#[derive(Debug)]
pub enum ConfigError {
    Json(serde_json::Error),
    /// Config schema is newer than the firmware (e.g. after OTA rollback) — do NOT overwrite.
    UnsupportedVersion(u32),
    /// The `schema_version` field is missing.
    MissingVersion,
    /// Serialization buffer could not be allocated — config too large for the device heap.
    TooLarge,
}

impl core::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            ConfigError::Json(e) => write!(f, "JSON-Fehler: {e}"),
            ConfigError::UnsupportedVersion(v) => {
                write!(
                    f,
                    "Schema-Version {v} neuer als Firmware ({CURRENT_SCHEMA_VERSION})"
                )
            }
            ConfigError::MissingVersion => write!(f, "schema_version fehlt"),
            ConfigError::TooLarge => {
                write!(
                    f,
                    "Config zu groß für den Gerätespeicher (max. {MAX_SENSORS} Sensoren)"
                )
            }
        }
    }
}

impl std::error::Error for ConfigError {}

impl Config {
    /// Loads config from JSON including forward-migration of older schemas.
    pub fn from_json(bytes: &[u8]) -> Result<Config, ConfigError> {
        // Peek only at the schema version first: on the current schema we deserialize
        // straight from the slice. The `serde_json::Value` detour (below, only for old
        // schemas) costs a multiple of the JSON size in transient heap — enough to OOM
        // the ESP32 at boot with a full sensor inventory.
        #[derive(serde::Deserialize)]
        struct SchemaProbe {
            schema_version: Option<u32>,
        }
        let probe: SchemaProbe = serde_json::from_slice(bytes).map_err(ConfigError::Json)?;
        let version = probe.schema_version.ok_or(ConfigError::MissingVersion)?;

        if version > CURRENT_SCHEMA_VERSION {
            // Downgrade after OTA rollback: do not destroy the config; caller handles fail-safe.
            return Err(ConfigError::UnsupportedVersion(version));
        }
        if version == CURRENT_SCHEMA_VERSION {
            return serde_json::from_slice(bytes).map_err(ConfigError::Json);
        }

        let value: serde_json::Value = serde_json::from_slice(bytes).map_err(ConfigError::Json)?;
        let migrated = migrate(value, version);
        serde_json::from_value(migrated).map_err(ConfigError::Json)
    }

    /// Compact JSON into a pre-sized buffer. `to_string_pretty` with growth-by-doubling
    /// needed ~2× the final size in contiguous heap — exactly what panicked the ESP32
    /// while persisting a full inventory (~147 sensors) on a fragmented heap.
    /// `try_reserve` turns "buffer does not fit" into a clean error instead of an
    /// allocation abort (which would panic-reboot the device).
    pub fn to_json(&self) -> Result<String, ConfigError> {
        let mut buf: Vec<u8> = Vec::new();
        buf.try_reserve_exact(2048 + self.sensors.len() * 400)
            .map_err(|_| ConfigError::TooLarge)?;
        serde_json::to_writer(&mut buf, self).map_err(ConfigError::Json)?;
        Ok(String::from_utf8(buf).expect("serde_json emits UTF-8"))
    }

    /// Validates the config; returns a list of findings (empty = clean).
    pub fn validate(&self) -> Vec<Issue> {
        let mut issues =
            validate_inventory(self.schema_version, self.sensors.len(), self.sensors.iter());
        issues.extend(validate_panel(&self.panel));
        issues
    }
}

/// Sensor-level validation over any inventory view — the review screen judges a
/// working edit session (excluded entries filtered out) with EXACTLY the same
/// rules as the commit gate, without materializing a second [`SensorTable`].
pub fn validate_inventory<'a>(
    schema_version: u32,
    count: usize,
    sensors: impl Iterator<Item = SensorRef<'a>>,
) -> Vec<Issue> {
    let mut issues = Vec::new();
    if count > MAX_SENSORS {
        issues.push(Issue::error(
            IssueCode::TooManySensors,
            format!(
                "{count} Sensoren — Gerätelimit ist {MAX_SENSORS} (Heap-/Flash-Budget). \
                 Nicht benötigte Melder ausschließen."
            ),
        ));
    }
    if schema_version != CURRENT_SCHEMA_VERSION {
        issues.push(Issue::error(
            IssueCode::SchemaVersionMismatch,
            format!("schema_version {schema_version} != aktuell {CURRENT_SCHEMA_VERSION}"),
        ));
    }
    // Duplicate addresses among confirmed sensors (Telenot permits OR-linking,
    // so this is only a warning). Collected in the same pass as the per-sensor checks.
    let mut seen = std::collections::BTreeMap::new();
    for s in sensors {
        if s.confirmed() {
            *seen.entry(s.address()).or_insert(0usize) += 1;
        }
        {
            if s.topic().trim().is_empty() {
                issues.push(Issue::error(
                    IssueCode::EmptyTopic,
                    format!("Sensor 0x{:04X}: leeres Topic", s.address()),
                ));
            }
            if s.name().trim().is_empty() {
                issues.push(Issue::error(
                    IssueCode::EmptyName,
                    format!("Sensor 0x{:04X}: leerer Name", s.address()),
                ));
            }
            if !s.confirmed() {
                issues.push(Issue::warning(
                    IssueCode::SensorUnconfirmed,
                    format!(
                        "Sensor 0x{:04X} ({}) unbestätigt (Discovery) → fail-safe",
                        s.address(),
                        s.name()
                    ),
                ));
            }
            if s.polarity() == Polarity::Unconfirmed {
                issues.push(Issue::warning(
                    IssueCode::PolarityUnconfirmed,
                    format!(
                        "Sensor 0x{:04X} ({}): Polarität unbestätigt → fail-safe",
                        s.address(),
                        s.name()
                    ),
                ));
            }
            if s.switchable() && !is_switchable_addr(s.address()) {
                issues.push(Issue::error(
                    IssueCode::SwitchableNotOutput,
                    format!(
                        "Sensor 0x{:04X} ({}): switchable, aber keine schaltbare \
                         Ausgangs-Adresse (Eingang oder System-Status-Block)",
                        s.address(),
                        s.name()
                    ),
                ));
            }
            if s.switchable() && s.kind() == SensorKind::Signalgeber {
                issues.push(Issue::warning(
                    IssueCode::SwitchableSignalgeber,
                    format!(
                        "Sensor 0x{:04X} ({}): Signalgeber schaltbar — Sirene remote \
                         stummschalten/auslösen möglich (bewusst?)",
                        s.address(),
                        s.name()
                    ),
                ));
            }
        }
    }
    for (addr, count) in seen.into_iter().filter(|(_, c)| *c > 1) {
        issues.push(Issue::warning(
            IssueCode::DuplicateAddress,
            format!("Adresse 0x{addr:04X} mehrfach belegt ({count}×) — ODER-Verknüpfung?"),
        ));
    }
    issues
}

/// Panel-settings validation: area ids in range and unique; the hiplex command
/// gate has no meaning on the complex (commands there are capture-verified).
pub fn validate_panel(panel: &PanelSettings) -> Vec<Issue> {
    let mut issues = Vec::new();
    let mut area_seen = std::collections::BTreeSet::new();
    for a in &panel.areas {
        if a.id == 0 || a.id > AREA_ID_MAX {
            issues.push(Issue::error(
                IssueCode::AreaIdInvalid,
                format!("Bereich-Id {} außerhalb 1–{AREA_ID_MAX}", a.id),
            ));
        } else if !area_seen.insert(a.id) {
            issues.push(Issue::error(
                IssueCode::AreaIdInvalid,
                format!("Bereich-Id {} mehrfach vergeben", a.id),
            ));
        }
    }
    if panel.hiplex_cmds_verified && panel.kind == PanelKind::Complex400 {
        issues.push(Issue::warning(
            IssueCode::PanelFlagIgnored,
            "hiplex_cmds_verified gesetzt, aber Panel ist complex 400H — Flag ohne \
             Wirkung"
                .into(),
        ));
    }
    issues
}

/// Validation finding. `code` is stable and machine-readable (for i18n/REST); `message`
/// is a localised explanation (log/fallback).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Issue {
    pub severity: Severity,
    pub code: IssueCode,
    pub message: String,
}

impl Issue {
    pub(crate) fn error(code: IssueCode, message: String) -> Self {
        Issue {
            severity: Severity::Error,
            code,
            message,
        }
    }
    pub(crate) fn warning(code: IssueCode, message: String) -> Self {
        Issue {
            severity: Severity::Warning,
            code,
            message,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Warning,
    Error,
}

impl Severity {
    pub fn as_str(&self) -> &'static str {
        match self {
            Severity::Warning => "warning",
            Severity::Error => "error",
        }
    }
}

impl IssueCode {
    /// Stable string (= serde representation) for REST/i18n.
    pub fn as_str(&self) -> &'static str {
        match self {
            IssueCode::SchemaVersionMismatch => "schema_version_mismatch",
            IssueCode::EmptyTopic => "empty_topic",
            IssueCode::EmptyName => "empty_name",
            IssueCode::SensorUnconfirmed => "sensor_unconfirmed",
            IssueCode::PolarityUnconfirmed => "polarity_unconfirmed",
            IssueCode::DuplicateAddress => "duplicate_address",
            IssueCode::SwitchableNotOutput => "switchable_not_output",
            IssueCode::SwitchableSignalgeber => "switchable_signalgeber",
            IssueCode::LegacyMalformed => "legacy_malformed",
            IssueCode::LegacySkipped => "legacy_skipped",
            IssueCode::LegacyEntry => "legacy_entry",
            IssueCode::AreaIdInvalid => "area_id_invalid",
            IssueCode::PanelFlagIgnored => "panel_flag_ignored",
            IssueCode::TooManySensors => "too_many_sensors",
        }
    }
}

/// Stable, machine-readable finding code. Maps 1:1 to the i18n keys of the setup web UI
/// (e.g. `sensor_unconfirmed` → `s8.w.unconf`, `polarity_unconfirmed` → `s8.w.pol`) so that
/// host and firmware emit identical, translatable codes — `message` remains log/fallback text only.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IssueCode {
    /// Config schema does not match the firmware version.
    SchemaVersionMismatch,
    /// Sensor has no MQTT topic.
    EmptyTopic,
    /// Sensor has no name.
    EmptyName,
    /// Discovery sensor not yet confirmed → fail-safe.
    SensorUnconfirmed,
    /// Polarity unconfirmed → fail-safe (no logical state derivable).
    PolarityUnconfirmed,
    /// Address used by multiple sensors (OR-linking possible).
    DuplicateAddress,
    /// `switchable` set on a non-output address (< 0x0500) — cannot be switched.
    SwitchableNotOutput,
    /// Signal sounder marked as switchable — siren could be silenced/triggered remotely.
    SwitchableSignalgeber,
    /// Legacy import: top-level JSON is not an object.
    LegacyMalformed,
    /// Legacy import: part of the structure was skipped.
    LegacySkipped,
    /// Legacy import: individual entry could not be migrated.
    LegacyEntry,
    /// Area id outside 1–16 or duplicated.
    AreaIdInvalid,
    /// A panel flag has no effect for the selected panel kind.
    PanelFlagIgnored,
    /// More sensors than the device can persist/load (heap + NVS budget).
    TooManySensors,
}

/// Migration chain: brings an older `schema_version` up to the current schema.
/// `#[serde(default)]` covers purely additive changes without a version bump; this function
/// handles only structural rewrites. Currently `1` is the first version → identity.
fn migrate(mut value: serde_json::Value, from: u32) -> serde_json::Value {
    // Placeholder for future migrations:
    //   if from < 2 { /* v1 -> v2 */ }
    let _ = from;
    if let Some(obj) = value.as_object_mut() {
        obj.insert(
            "schema_version".into(),
            serde_json::json!(CURRENT_SCHEMA_VERSION),
        );
    }
    value
}
