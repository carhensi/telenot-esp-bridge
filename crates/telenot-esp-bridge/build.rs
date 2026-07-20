fn main() {
    embuild::espidf::sysenv::output();
    build_info();
}

/// Embeds git short hash (`*` = dirty tree) + commit time as `BUILD_INFO` so multiple dev
/// builds of the same CalVer version are distinguishable in the diagnostics UI. Uses the
/// COMMIT time (not build time) to keep builds reproducible. Empty when git is unavailable.
fn build_info() {
    let hash = git(&["rev-parse", "--short=7", "HEAD"]);
    let dirty = git(&["status", "--porcelain"]).is_some_and(|s| !s.is_empty());
    let time = git(&["log", "-1", "--format=%cd", "--date=format:%d.%m. %H:%M"]);
    let info = match (hash, time) {
        (Some(h), Some(t)) => format!("{h}{} · {t}", if dirty { "*" } else { "" }),
        (Some(h), None) => format!("{h}{}", if dirty { "*" } else { "" }),
        _ => String::new(),
    };
    println!("cargo:rustc-env=BUILD_INFO={info}");
    // Best effort: re-run when HEAD moves — .git/HEAD only changes on branch switch, so
    // additionally track the branch ref itself (commits on the same branch) and packed-refs
    // (after git gc/pack). Dirty-flag freshness is per-cargo-invocation.
    println!("cargo:rerun-if-changed=../../.git/HEAD");
    if let Some(ref_path) = git(&["symbolic-ref", "-q", "HEAD"]) {
        println!("cargo:rerun-if-changed=../../.git/{ref_path}");
    }
    println!("cargo:rerun-if-changed=../../.git/packed-refs");
}

fn git(args: &[&str]) -> Option<String> {
    let out = std::process::Command::new("git").args(args).output().ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
        .filter(|s| !s.is_empty())
}
