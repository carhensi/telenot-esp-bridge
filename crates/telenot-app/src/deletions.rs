//! Bounded, reboot-safe HA cleanup journal. Topics are generated only at send time.
use crate::hadisco::{deletions_for_each, EntityDeletion};
use telenot_config::{
    chunked::{BlobStore, StoreError},
    Config, MAX_SENSORS,
};
const KEY: &str = "ha_deletions";
const LIMIT: usize = MAX_SENSORS * 2;
#[derive(Default)]
pub struct Deletions(Vec<EntityDeletion>);
impl Deletions {
    pub fn load(store: &impl BlobStore) -> Result<Self, StoreError> {
        let Some(len) = store.blob_len(KEY)? else {
            return Ok(Self::default());
        };
        if len > LIMIT * 3 || len % 3 != 0 {
            return Err(StoreError("Ungültiges HA-Löschjournal".into()));
        }
        let mut bytes = vec![0; len];
        if store.get_blob(KEY, &mut bytes)? != Some(len) {
            return Err(StoreError("HA-Löschjournal unvollständig".into()));
        }
        let mut entries = Vec::with_capacity(len / 3);
        // `len % 3 == 0` is checked above, so every 3-byte slice below is in bounds.
        for i in (0..bytes.len()).step_by(3) {
            let b = &bytes[i..i + 3];
            if b[2] > 1 {
                return Err(StoreError("Ungültiger HA-Entity-Typ".into()));
            }
            entries.push(EntityDeletion {
                address: u16::from_le_bytes([b[0], b[1]]),
                switch: b[2] == 1,
            });
        }
        Ok(Self(entries))
    }
    fn save_entries(
        entries: &[EntityDeletion],
        store: &mut impl BlobStore,
    ) -> Result<(), StoreError> {
        let mut bytes = Vec::with_capacity(entries.len() * 3);
        for e in entries {
            bytes.extend_from_slice(&e.address.to_le_bytes());
            bytes.push(u8::from(e.switch));
        }
        store.set_blob(KEY, &bytes)
    }
    pub fn prepare(
        &mut self,
        old: &Config,
        new: &Config,
        store: &mut impl BlobStore,
    ) -> Result<(), StoreError> {
        let mut overflow = false;
        deletions_for_each(old, new, &mut |e| {
            if !self.0.contains(&e) {
                if self.0.len() == LIMIT {
                    overflow = true;
                } else {
                    self.0.push(e);
                }
            }
        });
        if overflow {
            return Err(StoreError(
                "HA-Löschjournal voll; MQTT verbinden und erneut speichern".into(),
            ));
        }
        Self::save_entries(&self.0, store)
    }
    pub fn first(&self) -> Option<EntityDeletion> {
        self.0.first().copied()
    }
    pub fn complete(&mut self, store: &mut impl BlobStore) -> Result<(), StoreError> {
        if !self.0.is_empty() {
            Self::save_entries(&self.0[1..], store)?;
            self.0.remove(0);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[derive(Default)]
    struct Store {
        bytes: Option<Vec<u8>>,
        fail: bool,
    }
    impl BlobStore for Store {
        fn blob_len(&self, _: &str) -> Result<Option<usize>, StoreError> {
            Ok(self.bytes.as_ref().map(Vec::len))
        }
        fn get_blob(&self, _: &str, buf: &mut [u8]) -> Result<Option<usize>, StoreError> {
            Ok(self.bytes.as_ref().map(|b| {
                buf[..b.len()].copy_from_slice(b);
                b.len()
            }))
        }
        fn set_blob(&mut self, _: &str, val: &[u8]) -> Result<(), StoreError> {
            if self.fail {
                return Err(StoreError("write failed".into()));
            }
            self.bytes = Some(val.to_vec());
            Ok(())
        }
        fn remove(&mut self, _: &str) -> Result<(), StoreError> {
            self.bytes = None;
            Ok(())
        }
    }
    #[test]
    fn journal_survives_reboot_and_failed_completion() {
        let entry = EntityDeletion {
            address: 0x1234,
            switch: true,
        };
        let mut store = Store::default();
        let mut journal = Deletions(vec![entry]);
        journal
            .prepare(&Config::default(), &Config::default(), &mut store)
            .unwrap();
        assert_eq!(store.bytes.as_ref().unwrap().len(), 3);
        assert_eq!(Deletions::load(&store).unwrap().first(), Some(entry));
        store.fail = true;
        assert!(journal.complete(&mut store).is_err());
        assert_eq!(journal.first(), Some(entry));
        assert_eq!(Deletions::load(&store).unwrap().first(), Some(entry));
        store.fail = false;
        journal.complete(&mut store).unwrap();
        assert_eq!(Deletions::load(&store).unwrap().first(), None);
    }
    #[test]
    fn malformed_or_oversized_journal_fails_closed() {
        for bytes in [vec![0], vec![0, 0, 2], vec![0; LIMIT * 3 + 3]] {
            assert!(Deletions::load(&Store {
                bytes: Some(bytes),
                fail: false
            })
            .is_err());
        }
    }
}
