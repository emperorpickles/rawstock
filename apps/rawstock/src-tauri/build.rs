use std::path::PathBuf;

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
    tauri_build::build()
}
