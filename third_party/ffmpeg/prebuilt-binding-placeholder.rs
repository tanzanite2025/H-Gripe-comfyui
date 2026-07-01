// Placeholder referenced by FFMPEG_BINDING_PATH (see
// apps/desktop-tauri/src-tauri/.cargo/config.toml).
//
// rusty_ffmpeg's Windows link path requires either FFMPEG_BINDING_PATH or
// FFMPEG_INCLUDE_DIR to be set; pointing it here keeps it off the bindgen path
// (which would need libclang). This file is copied into OUT_DIR and then
// immediately overwritten by the `use_prebuilt_binding` feature with the
// crate's shipped FFmpeg 8 bindings, so its contents are never compiled.
