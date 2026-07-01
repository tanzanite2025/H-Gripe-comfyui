use std::path::PathBuf;

fn main() {
    tauri_build::build();

    // With the `native-ffmpeg` feature, the vendored libav* DLLs must sit next
    // to the binary that links them (Windows resolves DLLs from the executable
    // directory). Copy them beside both the app binary and the cargo-test
    // binaries so `cargo test --features native-ffmpeg` can load them. Build
    // scripts don't see `#[cfg(feature = ...)]`, so gate on the CARGO_FEATURE_*
    // env var cargo sets for enabled features.
    if std::env::var_os("CARGO_FEATURE_NATIVE_FFMPEG").is_some() {
        copy_ffmpeg_runtime_dlls();
    }
}

fn copy_ffmpeg_runtime_dlls() {
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let bin = manifest.join("../../../third_party/ffmpeg/win-x64/bin");
    println!("cargo:rerun-if-changed={}", bin.display());

    // OUT_DIR is `<target>/<profile>/build/<pkg>-<hash>/out`; the profile dir
    // (where the app + test binaries land, alongside `deps/`) is four levels up.
    let out_dir = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let Some(profile_dir) = out_dir.ancestors().nth(3) else {
        return;
    };
    let dests = [profile_dir.to_path_buf(), profile_dir.join("deps")];

    let entries = match std::fs::read_dir(&bin) {
        Ok(entries) => entries,
        Err(err) => {
            println!(
                "cargo:warning=native-ffmpeg: cannot read {} ({err}); DLLs not copied",
                bin.display()
            );
            return;
        }
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("dll") {
            continue;
        }
        let Some(name) = path.file_name() else {
            continue;
        };
        for dest in &dests {
            let _ = std::fs::create_dir_all(dest);
            let _ = std::fs::copy(&path, dest.join(name));
        }
    }
}
