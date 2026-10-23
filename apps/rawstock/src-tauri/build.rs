use std::path::PathBuf;
use std::process::Command;

fn main() {
    // `generate_context!` embeds `frontendDist` in release builds. A fresh
    // `tauri dev` has not produced `dist/` yet; the dev server is `devUrl`.
    // Create a placeholder only when the real build output is absent, and
    // never replace an `npm run build`.
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let dist = manifest.join("../dist");
    if !dist.join("index.html").exists() {
        std::fs::create_dir_all(&dist).expect("create frontend dist");
        std::fs::write(
            dist.join("index.html"),
            "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><title>rawstock</title></head><body></body></html>\n",
        )
        .expect("write frontend placeholder");
    }
    stamp_build();
    tauri_build::build()
}

/// The About dialog shows the commit and build date. `RAWSTOCK_GIT_COMMIT`
/// and `SOURCE_DATE_EPOCH` override them for a reproducible release build.
fn stamp_build() {
    println!("cargo:rerun-if-env-changed=RAWSTOCK_GIT_COMMIT");
    println!("cargo:rerun-if-env-changed=SOURCE_DATE_EPOCH");
    let commit = std::env::var("RAWSTOCK_GIT_COMMIT")
        .ok()
        .or_else(|| git(&["rev-parse", "--short=12", "HEAD"]))
        .unwrap_or_else(|| "unknown".to_string());
    if let Some(dir) = git(&["rev-parse", "--absolute-git-dir"]) {
        // logs/HEAD changes on every commit and checkout.
        let log = PathBuf::from(dir).join("logs").join("HEAD");
        if log.exists() {
            println!("cargo:rerun-if-changed={}", log.display());
        }
    }
    let epoch = std::env::var("SOURCE_DATE_EPOCH")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or_else(|| {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|duration| duration.as_secs())
                .unwrap_or(0)
        });
    println!("cargo:rustc-env=RAWSTOCK_GIT_COMMIT={commit}");
    println!("cargo:rustc-env=RAWSTOCK_BUILD_EPOCH={epoch}");
}

fn git(args: &[&str]) -> Option<String> {
    let output = Command::new("git").args(args).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8(output.stdout).ok()?;
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_string())
}
