//! One-shot migration: old Node.js config (JSON) → new schema (canonical addresses).
//! Usage: `cargo run -p telenot-config --example migrate -- <legacy.json> [out.json]`
//! Prints the new config to stdout (or writes to out.json) and lists issues on stderr.

use std::env;
use telenot_config::{Config, Severity};

fn main() {
    let mut args = env::args().skip(1);
    let in_path = args
        .next()
        .expect("usage: migrate <legacy.json> [out.json]");
    let out_path = args.next();

    let bytes = std::fs::read(&in_path).expect("Legacy-JSON lesbar");
    let imp = Config::import_legacy(&bytes).expect("Import");

    eprintln!(
        "{} Sensoren importiert, {} Hinweise:",
        imp.config.sensors.len(),
        imp.issues.len()
    );
    for i in &imp.issues {
        let tag = match i.severity {
            Severity::Error => "FEHLER",
            Severity::Warning => "WARN",
        };
        eprintln!("  [{tag}] {}", i.message);
    }

    let json = imp.config.to_json().expect("Serialisierung");
    match out_path {
        Some(p) => {
            std::fs::write(&p, &json).expect("schreibbar");
            eprintln!("→ geschrieben: {p}");
        }
        None => println!("{json}"),
    }
}
