//! Build script: validate the Tauri config and provision the `hc`
//! sidecar binary the `externalBin` entry names (ticket #156).
//!
//! Sidecars are NOT committed: at packaging time the installer
//! pipeline drops the real per-target `hc` beside the app bundle.
//! For dev builds this script links the local `hc` (from
//! `FACTORY_HC_SIDECAR`, else `hc` on PATH) into the gitignored
//! `src-tauri/bin/` dir Tauri resolves `externalBin` against. Without
//! any `hc`, the build stops here with the fix named.

fn main() {
    let triple = std::env::var("TARGET").unwrap_or_else(|_| "unknown".to_string());
    let dest_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("bin");
    let dest = dest_dir.join(format!("hc-{triple}"));
    if !dest.exists() {
        let src = std::env::var("FACTORY_HC_SIDECAR").ok().or_else(|| {
            std::env::var("PATH").ok().and_then(|path| {
                std::env::split_paths(&path).find_map(|dir| {
                    let candidate = dir.join("hc");
                    candidate
                        .is_file()
                        .then_some(candidate.to_string_lossy().into_owned())
                })
            })
        });
        match src {
            Some(src) => {
                std::fs::create_dir_all(&dest_dir).expect("create src-tauri/bin");
                std::fs::copy(&src, &dest).expect("stage hc sidecar for Tauri build");
                println!("cargo:warning=staged hc sidecar from {src}");
            }
            None => {
                panic!(
                    "no hc sidecar for Tauri build (want {}): install herdr-core as `hc` on PATH or set FACTORY_HC_SIDECAR",
                    dest.display()
                );
            }
        }
    }
    println!("cargo:rerun-if-env-changed=FACTORY_HC_SIDECAR");
    println!("cargo:rerun-if-env-changed=PATH");
    tauri_build::build()
}
