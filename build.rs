fn main() {
    // linkall.x pulls in the rest of esp-hal's linker scripts; it must come last.
    println!("cargo:rustc-link-arg=-Tlinkall.x");
    // The build time, for setting the clock chip on first boot when its time is invalid. No rerun-if-changed, so every real build re-stamps.
    let unix = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
    let out = std::path::PathBuf::from(std::env::var_os("OUT_DIR").unwrap());
    std::fs::write(out.join("build_time.rs"), format!("pub const BUILD_UNIX_SECS: i64 = {unix};\n")).unwrap();
}
