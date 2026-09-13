use std::{env, path::PathBuf, process::Command};

fn main() {
    println!("cargo:rerun-if-changed=native/macos/bridge.swift");

    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("macos") {
        return;
    }

    let out_dir = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR is set by Cargo"));
    let output = out_dir.join("agentplay-macos-bridge");
    let source = PathBuf::from("native/macos/bridge.swift");

    let status = Command::new("xcrun")
        .args(["swiftc", "-parse-as-library"])
        .arg(&source)
        .args(["-framework", "ScreenCaptureKit"])
        .args(["-framework", "CoreGraphics"])
        .args(["-framework", "ImageIO"])
        .args(["-framework", "ApplicationServices"])
        .arg("-o")
        .arg(&output)
        .status()
        .expect("xcrun must be available to build the macOS backend");

    assert!(status.success(), "failed to compile native macOS bridge");
    println!(
        "cargo:rustc-env=AGENTPLAY_MACOS_BRIDGE={}",
        output.display()
    );
}
