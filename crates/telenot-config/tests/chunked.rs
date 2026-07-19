//! Tests für das chunked, crash-sichere Storage-Format — mit HashMap-Mock
//! inklusive Fehlerinjektion (set_blob-Ausfall nach N Aufrufen, gezielte
//! Byte-Korruption).

use std::collections::HashMap;

use telenot_config::chunked::{chunk_key, load, load_meta, save, BlobStore, StoreError, META_KEY};
use telenot_config::{
    AreaCfg, Config, GmsVariant, PanelKind, PanelSettings, Polarity, Sensor, SensorKind,
    SensorTable, CURRENT_SCHEMA_VERSION,
};

/// NVS-Mock: HashMap-Blobs + injizierbarer set_blob-Ausfall.
#[derive(Default)]
struct MockStore {
    map: HashMap<String, Vec<u8>>,
    /// `Some(n)`: die nächsten n `set_blob`-Aufrufe gelingen noch, danach Fehler.
    fail_after_sets: Option<usize>,
}

impl MockStore {
    /// Kippt ein Byte im Blob `key` (Korruptions-Injektion).
    fn corrupt(&mut self, key: &str, byte_idx: usize) {
        self.map.get_mut(key).expect("Key existiert")[byte_idx] ^= 0xFF;
    }
}

impl BlobStore for MockStore {
    fn blob_len(&self, key: &str) -> Result<Option<usize>, StoreError> {
        Ok(self.map.get(key).map(Vec::len))
    }

    fn get_blob(&self, key: &str, buf: &mut [u8]) -> Result<Option<usize>, StoreError> {
        let Some(val) = self.map.get(key) else {
            return Ok(None);
        };
        if buf.len() < val.len() {
            return Err(StoreError(format!(
                "Puffer {} B zu klein für Blob {} B",
                buf.len(),
                val.len()
            )));
        }
        buf[..val.len()].copy_from_slice(val);
        Ok(Some(val.len()))
    }

    fn set_blob(&mut self, key: &str, val: &[u8]) -> Result<(), StoreError> {
        if let Some(remaining) = &mut self.fail_after_sets {
            if *remaining == 0 {
                return Err(StoreError("injizierter Schreibfehler".into()));
            }
            *remaining -= 1;
        }
        self.map.insert(key.into(), val.to_vec());
        Ok(())
    }

    fn remove(&mut self, key: &str) -> Result<(), StoreError> {
        self.map.remove(key);
        Ok(())
    }
}

/// Realistisch benannter Test-Sensor (Namen/Orte wie eine echte Installation).
fn sensor(i: usize) -> Sensor {
    const KINDS: [SensorKind; 4] = [
        SensorKind::Magnetkontakt,
        SensorKind::Bewegungsmelder,
        SensorKind::Rauchmelder,
        SensorKind::Schliesskontakt,
    ];
    const ROOMS: [&str; 5] = ["Wohnzimmer", "Küche", "Flur EG", "Schlafzimmer", "Keller"];
    Sensor {
        address: 0x0100 + i as u16,
        name: format!("MK Fenster {i:03}"),
        name_ha: format!("Fenster {i:03} Süd"),
        kind: KINDS[i % KINDS.len()],
        topic: format!("eg/fenster/{i:03}"),
        location: ROOMS[i % ROOMS.len()].into(),
        polarity: if i.is_multiple_of(3) {
            Polarity::ActiveHigh
        } else {
            Polarity::ActiveLow
        },
        confirmed: i.is_multiple_of(2),
        switchable: false,
        show_in_homekit: i.is_multiple_of(5),
    }
}

fn config(n: usize) -> Config {
    let sensors: Vec<Sensor> = (0..n).map(sensor).collect();
    Config {
        schema_version: CURRENT_SCHEMA_VERSION,
        sensors: SensorTable::from_sensors(&sensors).expect("Tabelle passt"),
        panel: PanelSettings {
            kind: PanelKind::Hiplex8400,
            gms_variant: GmsVariant::Plus,
            hiplex_cmds_verified: true,
            areas: vec![
                AreaCfg {
                    id: 1,
                    name: "Erdgeschoss".into(),
                },
                AreaCfg {
                    id: 16,
                    name: "Zentrale".into(),
                },
            ],
        },
    }
}

#[test]
fn roundtrip_various_sizes() {
    // 0 / 1 / genau ein Chunk / ein Chunk + 1 / weit über MAX_SENSORS (600 = 19 Chunks).
    for n in [0usize, 1, 32, 33, 600] {
        let mut store = MockStore::default();
        let cfg = config(n);
        save(&mut store, &cfg).unwrap_or_else(|e| panic!("save({n}): {e}"));
        let loaded = load(&store)
            .unwrap_or_else(|e| panic!("load({n}): {e}"))
            .unwrap_or_else(|| panic!("load({n}): None"));
        assert_eq!(loaded, cfg, "Roundtrip mit {n} Sensoren");
    }
}

#[test]
fn save_alternates_sets_and_increments_generation() {
    let mut store = MockStore::default();

    save(&mut store, &config(40)).expect("save 1");
    let m1 = load_meta(&store).expect("meta 1").expect("meta 1 da");
    assert!(!m1.active_b, "erster Save landet in Satz A");
    assert_eq!(m1.generation, 1);
    assert_eq!(load(&store).unwrap().unwrap(), config(40));

    save(&mut store, &config(41)).expect("save 2");
    let m2 = load_meta(&store).expect("meta 2").expect("meta 2 da");
    assert!(m2.active_b, "zweiter Save landet in Satz B");
    assert_eq!(m2.generation, 2);
    assert_eq!(load(&store).unwrap().unwrap(), config(41));

    save(&mut store, &config(42)).expect("save 3");
    let m3 = load_meta(&store).expect("meta 3").expect("meta 3 da");
    assert!(!m3.active_b, "dritter Save wieder Satz A");
    assert_eq!(m3.generation, 3);
    assert_eq!(load(&store).unwrap().unwrap(), config(42));
}

#[test]
fn torn_write_keeps_old_config_intact() {
    let mut store = MockStore::default();
    let old_cfg = config(100); // 4 Chunks
    save(&mut store, &old_cfg).expect("Erst-Save");

    // Zweiter Save (5 Writes: 4 Chunks + Meta) crasht nach 2 Chunk-Writes —
    // also NACH einigen Chunks, aber VOR dem Meta-Write.
    store.fail_after_sets = Some(2);
    let new_cfg = config(120);
    let err = save(&mut store, &new_cfg);
    assert!(err.is_err(), "Save muss den injizierten Fehler melden");

    // Alter Stand bleibt vollständig lesbar — die halb geschriebenen
    // B-Chunks sind unsichtbar, weil die Meta nie umgeschaltet wurde.
    store.fail_after_sets = None;
    let loaded = load(&store)
        .expect("load nach Torn Write")
        .expect("Config da");
    assert_eq!(loaded, old_cfg);
    let meta = load_meta(&store).unwrap().unwrap();
    assert_eq!(meta.generation, 1, "Generation unverändert");
    assert!(!meta.active_b, "Satz A weiterhin aktiv");
}

#[test]
fn crc_corruption_is_detected() {
    let mut store = MockStore::default();
    save(&mut store, &config(100)).expect("save");
    let meta = load_meta(&store).unwrap().unwrap();

    // Ein Byte mitten im zweiten aktiven Chunk kippen.
    store.corrupt(&chunk_key(meta.active_b, 1), 17);

    let err = load(&store).expect_err("Korruption muss ein Fehler sein, kein Teil-Ergebnis");
    assert!(
        err.0.contains("Chunk 1"),
        "Fehler nennt den Chunk-Index: {err}"
    );
}

#[test]
fn shrink_leaves_no_stale_chunks() {
    let mut store = MockStore::default();

    // 100 Sensoren = 4 Chunks (Satz A), nochmal 100 (Satz B) — beide Sätze voll.
    save(&mut store, &config(100)).expect("save A/100");
    save(&mut store, &config(100)).expect("save B/100");

    // Schrumpfen auf 10 (1 Chunk, Satz A): die alten cfg_a1..a3 müssen weg sein.
    save(&mut store, &config(10)).expect("save A/10");
    let meta = load_meta(&store).unwrap().unwrap();
    assert!(!meta.active_b);
    assert_eq!(meta.chunk_count, 1);
    for i in 1..4 {
        assert!(
            !store.map.contains_key(&chunk_key(false, i)),
            "Leichen-Chunk {} übrig",
            chunk_key(false, i)
        );
    }
    assert_eq!(load(&store).unwrap().unwrap(), config(10));

    // Wieder wachsen auf 100 (Satz B) — Load weiterhin korrekt.
    save(&mut store, &config(100)).expect("save B/100 erneut");
    assert_eq!(load(&store).unwrap().unwrap(), config(100));
}

#[test]
fn missing_meta_is_none() {
    let store = MockStore::default();
    assert!(load(&store)
        .expect("leerer Store ist kein Fehler")
        .is_none());
}

#[test]
fn broken_meta_is_error() {
    let mut store = MockStore::default();
    save(&mut store, &config(5)).expect("save");
    store.map.insert(META_KEY.into(), vec![0xFF; 7]);
    assert!(
        load(&store).is_err(),
        "kaputte Meta-Bytes müssen Err liefern"
    );
}
