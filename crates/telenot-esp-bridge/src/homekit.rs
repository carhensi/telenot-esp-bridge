//! Direct HomeKit (HAP) — phase-0 spike. Exposes the EMA as a HomeKit `SecuritySystem`
//! (read status + arm/disarm) via Espressif's esp-homekit-sdk (FFI,
//! `esp_idf_svc::sys::homekit`). Runs in its own task; in HomeKit mode MQTT is off
//! (RAM freed for HAP — HAP brings its own E2E encryption).
//!
//! Spike scope: SecuritySystem accessory + pairing + live status + arming. Disarm is enqueued
//! but gated by the fail-closed loop path (authorisation is separate). Individual detectors
//! (`show_in_homekit`) are also included.

use core::ffi::{c_char, c_int, c_void};
use core::ptr;
use core::sync::atomic::{AtomicBool, AtomicPtr, Ordering};
use core::time::Duration;
use std::ffi::{CStr, CString};
use std::sync::{Arc, Mutex, OnceLock};

use esp_idf_svc::sys::homekit as hap;
use telenot_app::{App, BridgeCommand, Intent};
use telenot_config::{Config, SensorKind};
use telenot_core::ArmCommand;

/// Maximum number of detector services on the (single) accessory. HAP allows ~100 services
/// per accessory; we leave headroom for SecuritySystem + mandatory services.
const MAX_SENSOR_SERVS: usize = 90;

/// HomeKit-Sensortyp, auf den ein Telenot-Melder gespiegelt wird.
#[derive(Clone, Copy)]
enum SensorClass {
    Contact,
    Motion,
    Smoke,
    Leak,
}

impl SensorClass {
    /// Detector `kind` → matching HomeKit sensor type. `None` = not mirrored (diagnostic/bus
    /// types with no meaningful HomeKit equivalent).
    fn from_kind(k: SensorKind) -> Option<SensorClass> {
        match k {
            SensorKind::Bewegungsmelder => Some(SensorClass::Motion),
            SensorKind::Magnetkontakt
            | SensorKind::Schliesskontakt
            | SensorKind::Gehaeuse
            | SensorKind::Sabotage
            | SensorKind::Ueberfallmelder => Some(SensorClass::Contact),
            SensorKind::Rauchmelder => Some(SensorClass::Smoke),
            SensorKind::Wassermelder => Some(SensorClass::Leak),
            _ => None,
        }
    }
    /// UUID of the state characteristic (for updating from the panel status).
    fn state_uuid(self) -> &'static [u8] {
        match self {
            SensorClass::Contact => hap::HAP_CHAR_UUID_CONTACT_SENSOR_STATE,
            SensorClass::Motion => hap::HAP_CHAR_UUID_MOTION_DETECTED,
            SensorClass::Smoke => hap::HAP_CHAR_UUID_SMOKE_DETECTED,
            SensorClass::Leak => hap::HAP_CHAR_UUID_LEAK_DETECTED,
        }
    }
    unsafe fn create_serv(self) -> *mut hap::hap_serv_t {
        match self {
            SensorClass::Contact => hap::hap_serv_contact_sensor_create(0),
            SensorClass::Motion => hap::hap_serv_motion_sensor_create(false),
            SensorClass::Smoke => hap::hap_serv_smoke_sensor_create(0),
            SensorClass::Leak => hap::hap_serv_leak_sensor_create(0),
        }
    }
    /// `active` (open/motion/triggered) → HomeKit value. Motion uses bool, the others uint8 1/0.
    fn val(self, active: bool) -> hap::hap_val_t {
        match self {
            SensorClass::Motion => hap::hap_val_t { b: active },
            _ => hap::hap_val_t { u: active as u32 },
        }
    }
}

/// Generates a random, valid HomeKit pairing code (`xxx-xx-xxx`). Avoids Apple's forbidden codes
/// (all digits equal, strictly ascending/descending) — NOT the public Espressif sample code.
fn gen_setup_code() -> String {
    // Without active WiFi/BT, esp_random() is only a weak PRNG. For the one-time code
    // generation, enable the HW entropy source (SAR-ADC) → true TRNG, then disable again
    // (must happen before any ADC/RF use — harmless here, Ethernet uses no ADC).
    unsafe { esp_idf_svc::sys::bootloader_random_enable() };
    let code = loop {
        let mut d = [0u8; 8];
        for x in d.iter_mut() {
            *x = (unsafe { esp_idf_svc::sys::esp_random() } % 10) as u8;
        }
        let all_same = d.iter().all(|&x| x == d[0]);
        let asc = d.windows(2).all(|w| w[1] == w[0] + 1);
        let desc = d.windows(2).all(|w| w[0] == w[1] + 1);
        if all_same || asc || desc {
            continue;
        }
        break format!(
            "{}{}{}-{}{}-{}{}{}",
            d[0], d[1], d[2], d[3], d[4], d[5], d[6], d[7]
        );
    };
    unsafe { esp_idf_svc::sys::bootloader_random_disable() };
    code
}

/// Global app reference for C callbacks (the bridge is a single device).
static APP: OnceLock<Arc<Mutex<App>>> = OnceLock::new();
/// Handle of the SecuritySystem CurrentState characteristic (updated by the state-sync task).
static CURR_CHAR: AtomicPtr<hap::hap_char_t> = AtomicPtr::new(ptr::null_mut());
/// HAP runs only once (hap_deinit is a stub). Live start is therefore idempotent.
static STARTED: AtomicBool = AtomicBool::new(false);

// HomeKit SecuritySystem — CurrentState: 0=Stay 1=Away 2=Night 3=Disarmed 4=Triggered.
fn arm_state_to_hk(s: &str) -> u8 {
    match s {
        "ARMED_HOME" => 0,
        "ARMED_AWAY" => 1,
        "ARMED_NIGHT" => 2,
        "TRIGGERED" => 4,
        _ => 3, // DISARMED / unknown
    }
}
// TargetState: 0=Stay 1=Away 2=Night 3=Disarm.
fn hk_target_to_cmd(t: u32) -> ArmCommand {
    match t {
        0 => ArmCommand::ArmHome,
        1 => ArmCommand::ArmAway,
        2 => ArmCommand::ArmNight,
        _ => ArmCommand::Disarm,
    }
}

/// `&str` → permanently-live `*mut c_char` (passed to C; intentionally leaked, lives until exit).
/// NUL-tolerant: an embedded NUL (e.g. from corrupt NVS) would otherwise panic the HAP thread.
fn leak_c(s: &str) -> *mut c_char {
    match CString::new(s) {
        Ok(c) => c.into_raw(),
        Err(_) => CString::new(s.replace('\0', ""))
            .expect("NUL entfernt")
            .into_raw(),
    }
}

/// Mandatory identify routine (in production: blink LED; here a no-op).
unsafe extern "C" fn identify(_ha: *mut hap::hap_acc_t) -> c_int {
    hap::HAP_SUCCESS as c_int
}

/// Write callback: HomeKit → EMA. Only the TargetState characteristic is processed → enqueued
/// as `Intent::Command`; the existing fail-closed loop path authorises and executes.
unsafe extern "C" fn sec_write(
    write_data: *mut hap::hap_write_data_t,
    count: c_int,
    _serv_priv: *mut c_void,
    _write_priv: *mut c_void,
) -> c_int {
    for i in 0..count {
        let w = &mut *write_data.offset(i as isize);
        let uuid = hap::hap_char_get_type_uuid(w.hc);
        let is_target = !uuid.is_null()
            && CStr::from_ptr(uuid).to_bytes_with_nul()
                == hap::HAP_CHAR_UUID_SECURITY_SYSTEM_TARGET_STATE;
        if is_target {
            let cmd = hk_target_to_cmd(w.val.u); // uint8 in union → .u
            if let Some(app) = APP.get() {
                // Dedicated HomeKit path (not the PIN/remote-disarm path): the loop authorises via
                // homekit_authorized (disarm only if enabled in setup). If disarm is denied,
                // the state-sync loop mirrors the real (still armed) state back.
                app.lock().unwrap().intents.push(Intent::HomekitCommand {
                    cmd: BridgeCommand::Arm(cmd),
                });
            }
            // Mirror TargetState immediately (HomeKit expects the set value echoed back).
            hap::hap_char_update_val(w.hc, &mut w.val);
        }
        if !w.status.is_null() {
            *w.status = hap::hap_status_t_HAP_STATUS_SUCCESS;
        }
    }
    hap::HAP_SUCCESS as c_int
}

/// Starts the HAP task (bridge + pairing + state sync + live reconcile). `setup_code` format:
/// `xxx-xx-xxx`. `config` provides the detectors visible AT START; afterwards the HAP thread
/// can add/remove detectors LIVE (`App::homekit_reconcile` → `Intent::ApplyHomekit`) without
/// a reboot — each detector is its own bridged accessory; the SDK bumps c# automatically.
pub fn start(app: Arc<Mutex<App>>, config: Config) {
    // Idempotent: HAP cannot be cleanly stopped (hap_deinit is a stub), so only one start.
    if STARTED.swap(true, Ordering::SeqCst) {
        log::info!("HAP läuft bereits — Start ignoriert");
        return;
    }
    let _ = APP.set(app.clone());
    // Take the device-specific code from settings; generate randomly on first use. The assignment
    // in App triggers persistence in app_loop (save_mqtt).
    let code = {
        let mut a = app.lock().unwrap();
        // A NUL in the persisted code (corrupt/imported NVS) would panic the HAP thread at
        // CString construction — on EVERY boot. Better regenerate (will be re-persisted).
        if a.setup.mqtt.homekit_code.is_empty() || a.setup.mqtt.homekit_code.contains('\0') {
            a.setup.mqtt.homekit_code = gen_setup_code();
        }
        a.setup.mqtt.homekit_code.clone()
    };
    let _ = std::thread::Builder::new()
        .name("hap".into())
        .stack_size(12 * 1024)
        .spawn(move || unsafe { run(app, config, code) });
}

/// A live-published detector accessory (bridged) + its state characteristic. Only the HAP
/// thread holds/mutates this list, so the raw pointers are thread-locally safe.
struct SensorAcc {
    addr: u16,
    class: SensorClass,
    acc: *mut hap::hap_acc_t,
    ch: *mut hap::hap_char_t,
    last: i8,
    /// Current HomeKit name — used for reconcile comparison (rename → remove+re-add, because
    /// HAP cannot change the name characteristic after creation).
    label: String,
}

/// Creates a SEPARATE bridged accessory for a detector and attaches it to the bridge. One
/// accessory per detector (not a service on a shared accessory) → detectors can be added/removed
/// LIVE after `hap_start`; the SDK bumps c# automatically (no re-pairing needed).
unsafe fn add_sensor_acc(addr: u16, class: SensorClass, label: &str) -> Option<SensorAcc> {
    // LOCAL CStrings (no leak_c): the SDK copies all accessory/char strings internally via
    // strdup (esp_hap_char.c `val.s = strdup(s)`), so they may drop after creation.
    // This way repeated live reconciles accumulate NO heap over years (previously: 8 leaks/add).
    let uid = CString::new(format!("tnmelder-{addr:04x}")).unwrap();
    let c_name = CString::new(label).unwrap_or_else(|_| CString::new("Melder").unwrap());
    let c_model = CString::new("EMA-Melder").unwrap();
    let c_manuf = CString::new("Telenot").unwrap();
    let c_fw = CString::new(env!("CARGO_PKG_VERSION")).unwrap();
    let c_pv = CString::new("1.1.0").unwrap();
    let mut cfg = hap::hap_acc_cfg_t {
        name: c_name.as_ptr() as *mut c_char,
        model: c_model.as_ptr() as *mut c_char,
        manufacturer: c_manuf.as_ptr() as *mut c_char,
        serial_num: uid.as_ptr() as *mut c_char,
        fw_rev: c_fw.as_ptr() as *mut c_char,
        hw_rev: ptr::null_mut(),
        pv: c_pv.as_ptr() as *mut c_char,
        cid: hap::hap_cid_t_HAP_CID_SENSOR,
        identify_routine: Some(identify),
    };
    let acc = hap::hap_acc_create(&mut cfg);
    if acc.is_null() {
        return None;
    }
    let ss = class.create_serv();
    if ss.is_null() {
        return None;
    }
    hap::hap_serv_add_char(
        ss,
        hap::hap_char_name_create(c_name.as_ptr() as *mut c_char),
    );
    hap::hap_acc_add_serv(acc, ss);
    let ch = hap::hap_serv_get_char_by_uuid(ss, class.state_uuid().as_ptr() as *const c_char);
    hap::hap_add_bridged_accessory(acc, hap::hap_get_unique_aid(uid.as_ptr()));
    Some(SensorAcc {
        addr,
        class,
        acc,
        ch,
        last: -1,
        label: label.to_string(),
    })
}

/// HomeKit display name for a detector: the human-readable **display name** (`name`); falls back
/// to `name_ha` if empty, then the hex address. `name_ha` is the MQTT/HA name (often a slug/
/// generic) and would result in many identically named detectors in Apple Home.
fn hk_label(s: telenot_config::SensorRef<'_>) -> String {
    if !s.name().trim().is_empty() {
        s.name().to_string()
    } else if !s.name_ha().trim().is_empty() {
        s.name_ha().to_string()
    } else {
        format!("{:#06x}", s.address())
    }
}

/// Currently desired detectors for HomeKit from the working state (canonical via
/// `App::working_config`, then confirmed + `show_in_homekit` + valid HomeKit type).
/// Source of truth for the live reconcile.
/// `Err` = the inventory does not fit the sensor-table budget (reconcile is skipped).
fn desired_sensors(
    app: &Arc<Mutex<App>>,
) -> Result<Vec<(u16, SensorClass, String)>, telenot_config::ConfigError> {
    let cfg = app.lock().unwrap().working_config()?;
    Ok(cfg
        .sensors
        .iter()
        .filter(|s| s.confirmed() && s.show_in_homekit())
        .filter_map(|s| {
            let class = SensorClass::from_kind(s.kind())?;
            Some((s.address(), class, hk_label(s)))
        })
        .collect())
}

unsafe fn run(app: Arc<Mutex<App>>, config: Config, setup_code: String) {
    hap::hap_init(hap::hap_transport_t_HAP_TRANSPORT_ETHERNET);

    let serial = app.lock().unwrap().device.serial.replace(':', "");
    // PRIMARY: Bridge accessory. SecuritySystem + individual detectors hang as BRIDGED accessories
    // beneath it — only this model allows live add/remove of detectors after `hap_start`.
    let mut bridge_cfg = hap::hap_acc_cfg_t {
        name: leak_c("Telenot Bridge"),
        model: leak_c("EMA-Bridge"),
        manufacturer: leak_c("Telenot"),
        serial_num: leak_c(&serial),
        fw_rev: leak_c(env!("CARGO_PKG_VERSION")),
        hw_rev: ptr::null_mut(),
        pv: leak_c("1.1.0"),
        cid: hap::hap_cid_t_HAP_CID_BRIDGE,
        identify_routine: Some(identify),
    };
    let bridge = hap::hap_acc_create(&mut bridge_cfg);
    hap::hap_add_accessory(bridge);

    // SecuritySystem as a bridged accessory (initial Disarmed(3)/Disarm(3)).
    let sec_uid = format!("{serial}-sec");
    let mut sec_cfg = hap::hap_acc_cfg_t {
        name: leak_c("Alarmanlage"),
        model: leak_c("EMA"),
        manufacturer: leak_c("Telenot"),
        serial_num: leak_c(&sec_uid),
        fw_rev: leak_c(env!("CARGO_PKG_VERSION")),
        hw_rev: ptr::null_mut(),
        pv: leak_c("1.1.0"),
        cid: hap::hap_cid_t_HAP_CID_SECURITY_SYSTEM,
        identify_routine: Some(identify),
    };
    let sec_acc = hap::hap_acc_create(&mut sec_cfg);
    let serv = hap::hap_serv_security_system_create(3, 3);
    hap::hap_serv_add_char(serv, hap::hap_char_name_create(leak_c("Alarmanlage")));
    hap::hap_serv_set_write_cb(serv, Some(sec_write));
    hap::hap_acc_add_serv(sec_acc, serv);
    let curr = hap::hap_serv_get_char_by_uuid(
        serv,
        hap::HAP_CHAR_UUID_SECURITY_SYSTEM_CURRENT_STATE.as_ptr() as *const c_char,
    );
    CURR_CHAR.store(curr, Ordering::SeqCst);
    let sec_uid_c = CString::new(sec_uid.replace('\0', "")).expect("NUL entfernt");
    hap::hap_add_bridged_accessory(sec_acc, hap::hap_get_unique_aid(sec_uid_c.as_ptr()));

    // Initial detectors (from the committed boot config) as bridged accessories.
    let mut sensors: Vec<SensorAcc> = Vec::new();
    for s in config.sensors.iter() {
        if !s.confirmed() || !s.show_in_homekit() {
            continue;
        }
        let Some(class) = SensorClass::from_kind(s.kind()) else {
            continue;
        };
        if sensors.len() >= MAX_SENSOR_SERVS {
            log::warn!(
                "HomeKit: Melder-Limit ({MAX_SENSOR_SERVS}) erreicht — weitere übersprungen"
            );
            break;
        }
        if let Some(sa) = add_sensor_acc(s.address(), class, &hk_label(s)) {
            sensors.push(sa);
        }
    }

    // Defense-in-depth against the filter in start(): NEVER panic on the setup code, regenerate if needed.
    let sc = CString::new(setup_code)
        .unwrap_or_else(|_| CString::new(gen_setup_code()).expect("gen_setup_code ist NUL-frei"));
    hap::hap_set_setup_code(sc.as_ptr());
    let sid = CString::new("TnBr").unwrap();
    hap::hap_set_setup_id(sid.as_ptr());

    // Compute pairing payload (X-HM://…) for the QR code in the setup web + store in App.
    let payload_ptr = hap::esp_hap_get_setup_payload(
        sc.as_ptr() as *mut c_char,
        sid.as_ptr() as *mut c_char,
        false,
        hap::hap_cid_t_HAP_CID_BRIDGE,
    );
    if !payload_ptr.is_null() {
        let payload = CStr::from_ptr(payload_ptr).to_string_lossy().into_owned();
        let code = sc.to_str().unwrap_or("").to_string();
        // payload_ptr is strdup'd → one-time leak, negligible.
        app.lock().unwrap().homekit_pair = Some(telenot_app::HomekitPair { code, payload });
    }

    hap::hap_start();
    // The boot accessory set is built BEFORE hap_start (no auto-increment) → bump c# once
    // explicitly so paired controllers pick up the current set after a reboot without re-pairing.
    // Live changes AFTER hap_start (add/remove below) bump c# automatically via the SDK.
    hap::hap_update_config_number();
    log::info!(
        "HAP gestartet (Bridge, Ethernet) — Pairing-Code {} — {} Melder — freier Heap {} B",
        sc.to_str().unwrap_or("?"),
        sensors.len(),
        esp_idf_svc::sys::esp_get_free_heap_size()
    );

    let mut last = 255u8;
    loop {
        // 1. Live reconcile (stage → apply, no reboot): diff desired set against published set,
        //    add/remove accessories — the SDK bumps c# automatically.
        let reconcile = {
            let mut a = app.lock().unwrap();
            core::mem::replace(&mut a.homekit_reconcile, false)
        };
        if reconcile {
            match desired_sensors(&app) {
                Ok(want) => {
                    // Keep only if the address is desired AND the name is unchanged. Otherwise remove —
                    // gone OR renamed (HAP cannot change the name char → remove+re-add).
                    sensors.retain(|sa| {
                        let keep = want.iter().any(|(a, _, l)| *a == sa.addr && *l == sa.label);
                        if !keep {
                            hap::hap_remove_bridged_accessory(sa.acc);
                            log::info!("HomeKit: Melder {:#06x} live entfernt/erneuert", sa.addr);
                        }
                        keep
                    });
                    for (addr, class, label) in &want {
                        if sensors.iter().any(|s| s.addr == *addr) {
                            continue; // already published, name matches
                        }
                        if sensors.len() >= MAX_SENSOR_SERVS {
                            log::warn!("HomeKit: Melder-Limit erreicht — weitere übersprungen");
                            break;
                        }
                        if let Some(sa) = add_sensor_acc(*addr, *class, label) {
                            log::info!("HomeKit: Melder {addr:#06x} live hinzugefügt/aktualisiert");
                            sensors.push(sa);
                        }
                    }
                }
                // Published set stays untouched — never tear down accessories on an error.
                Err(_) => log::error!(
                    "HomeKit-Reconcile übersprungen: Konfiguration zu groß für den Gerätespeicher"
                ),
            }
        }

        // 2. State sync: arm_state → CurrentState + each detector → its state characteristic
        //    (only on change). State comes from the live snapshot (no serial access here).
        {
            let a = app.lock().unwrap();
            let hk = arm_state_to_hk(&a.live.arm_state);
            if hk != last {
                last = hk;
                let c = CURR_CHAR.load(Ordering::SeqCst);
                if !c.is_null() {
                    let mut val = hap::hap_val_t { u: hk as u32 };
                    hap::hap_char_update_val(c, &mut val);
                }
            }
            for sa in sensors.iter_mut() {
                if let Some(&(_, active)) = a.live.sensor_states.iter().find(|(x, _)| *x == sa.addr)
                {
                    if sa.last != active as i8 {
                        sa.last = active as i8;
                        let mut val = sa.class.val(active);
                        hap::hap_char_update_val(sa.ch, &mut val);
                    }
                }
            }
        }
        std::thread::sleep(Duration::from_millis(500));
    }
}
