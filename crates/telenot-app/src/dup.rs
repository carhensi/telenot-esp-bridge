//! Duplicate-detection heuristic for the setup view: identifies tamper/twin contacts (same
//! name stem, one marked as sabotage or with a higher address) and links them via `dup_of`
//! to the "primary" point. Purely heuristic and reversible — UI hint only, nothing is
//! automatically excluded.

use std::collections::BTreeMap;
use telenot_config::SensorKind;

/// Normalizes a name to its root stem (lowercase, without tamper/parenthetical suffixes).
fn name_root(name: &str) -> String {
    let lower = name.to_lowercase();
    let mut out = String::with_capacity(lower.len());
    for token in lower.split_whitespace() {
        if token.contains("sabotage") || token == "sab" || token.starts_with("(sab") {
            continue;
        }
        let t = token.trim_matches(|c: char| !c.is_alphanumeric());
        if t.is_empty() {
            continue;
        }
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(t);
    }
    out
}

/// Is this point a tamper/twin type? Only such points are true duplicates of a primary detector.
fn is_tamper(k: SensorKind) -> bool {
    matches!(k, SensorKind::Sabotage | SensorKind::Gehaeuse)
}

/// Returns `addr → dup_of`. A duplicate is a **tamper/enclosure twin** of a same-named
/// primary detector — NOT two same-named detectors of the same type (e.g. left + right
/// window sash). Only tamper points that point to a non-tamper primary (lowest address)
/// with the same name stem are flagged.
pub fn compute_dup_map(items: &[(u16, String, SensorKind)]) -> BTreeMap<u16, u16> {
    // Group by name stem (skip empty stems).
    let mut groups: BTreeMap<String, Vec<(u16, SensorKind)>> = BTreeMap::new();
    for (addr, name, kind) in items {
        let root = name_root(name);
        if root.is_empty() {
            continue;
        }
        groups.entry(root).or_default().push((*addr, *kind));
    }

    let mut map = BTreeMap::new();
    for (_root, mut members) in groups {
        if members.len() < 2 {
            continue;
        }
        members.sort_by_key(|(a, _)| *a);
        // Primary = lowest address among non-tamper detectors. If none exists, the group
        // is ambiguous (e.g. tamper-only) → nothing to flag.
        let Some(primary) = members
            .iter()
            .find(|(_, k)| !is_tamper(*k))
            .map(|(a, _)| *a)
        else {
            continue;
        };
        // Only tamper twins point to the primary; same-named double-sash contacts remain unflagged.
        for (addr, kind) in members {
            if addr != primary && is_tamper(kind) {
                map.insert(addr, primary);
            }
        }
    }
    map
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sabotage_twin_points_to_primary() {
        let items = vec![
            (0x0042, "MK Haustür".to_string(), SensorKind::Magnetkontakt),
            (
                0x0400,
                "MK Haustür (Sabotage)".to_string(),
                SensorKind::Sabotage,
            ),
            (0x0050, "BM Flur".to_string(), SensorKind::Bewegungsmelder),
        ];
        let map = compute_dup_map(&items);
        assert_eq!(map.get(&0x0400), Some(&0x0042), "tamper -> primary");
        assert_eq!(map.get(&0x0042), None, "primary itself is not a duplicate");
        assert_eq!(map.get(&0x0050), None, "Einzelner Melder ohne Doublette");
    }

    #[test]
    fn double_sash_windows_are_not_duplicates() {
        // Left + right window sash: same name, same type → NOT a duplicate.
        let items = vec![
            (
                0x002C,
                "MK Fenster Party".to_string(),
                SensorKind::Magnetkontakt,
            ),
            (
                0x002D,
                "MK Fenster Party".to_string(),
                SensorKind::Magnetkontakt,
            ),
        ];
        let map = compute_dup_map(&items);
        assert!(
            map.is_empty(),
            "double-sash contacts must not be flagged as duplicates"
        );
    }
}
