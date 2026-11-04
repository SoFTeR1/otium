//! Embeds the launcher's background music, if there is one: `assets/ambient.ogg` or
//! `assets/ambient.mp3` (the first found) goes into the exe; without it the build still works and
//! OTIUM simply plays nothing.

use std::path::Path;

fn main() {
    println!("cargo:rerun-if-changed=assets");
    let out = Path::new(&std::env::var("OUT_DIR").unwrap()).join("ambient.bin");
    let mut kind = "";
    let mut bytes = Vec::new();
    for (file, k) in [("assets/ambient.ogg", "ogg"), ("assets/ambient.mp3", "mp3")] {
        println!("cargo:rerun-if-changed={file}");
        if let Ok(b) = std::fs::read(file) {
            bytes = b;
            kind = k;
            break;
        }
    }
    std::fs::write(&out, &bytes).unwrap();
    println!("cargo:rustc-env=OTIUM_AMBIENT_KIND={kind}");
}
