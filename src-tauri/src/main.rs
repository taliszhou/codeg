// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

#[cfg(feature = "tauri-runtime")]
fn main() {
    // When called as a git credential helper, handle it immediately and exit.
    // This avoids starting the full Tauri GUI runtime.
    if std::env::args().any(|a| a == "--credential-helper") {
        codeg_lib::git_credential::run_credential_helper();
        return;
    }

    codeg_lib::run()
}

// When the desktop runtime feature is disabled (e.g. for CI clippy/test runs
// that pass `--no-default-features --tests`), the desktop bin still needs a
// `main` symbol but does nothing — codeg-server is the only useful binary in
// that build configuration.
#[cfg(not(feature = "tauri-runtime"))]
fn main() {
    eprintln!("This binary requires the `tauri-runtime` feature. Use `codeg-server` instead.");
    std::process::exit(2);
}
