// Build-time stamping and embed preparation.
//
// - Ensures ui/dist exists so the rust-embed macro compiles even before the
//   console has been built (empty dir ⇒ the daemon serves API-only, same as
//   the old missing-dist behavior).
// - Stamps the git sha and build date into the binary for `--version` and
//   /api/node.

use std::path::Path;
use std::process::Command;

fn main() {
    // ui/dist lives at the workspace root: crates/aspen/../../ui/dist
    let manifest = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    let dist = Path::new(&manifest).join("../../ui/dist");
    let _ = std::fs::create_dir_all(&dist);
    println!("cargo:rerun-if-changed={}", dist.display());

    // Restamp when the checked-out commit moves, not only on dist changes.
    let git = Path::new(&manifest).join("../../.git");
    println!("cargo:rerun-if-changed={}", git.join("HEAD").display());
    println!(
        "cargo:rerun-if-changed={}",
        git.join("refs/heads").display()
    );

    let sha = Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_owned())
        .unwrap_or_else(|| "unknown".into());
    println!("cargo:rustc-env=ASPEN_GIT_SHA={sha}");

    // Date only — a full timestamp would make builds non-reproducible for
    // no benefit.
    let date = Command::new("date")
        .arg("+%Y-%m-%d")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_owned())
        .unwrap_or_else(|| "unknown".into());
    println!("cargo:rustc-env=ASPEN_BUILD_DATE={date}");

    // Target triple, for picking the right release asset in `aspen update`.
    let target = std::env::var("TARGET").unwrap_or_else(|_| "unknown".into());
    println!("cargo:rustc-env=ASPEN_TARGET={target}");
}
