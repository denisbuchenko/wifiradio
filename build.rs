use std::{env, fs, path::PathBuf};

fn main() {
    // One private build is copied to both PCs. Keep the key across rebuilds.
    let local = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap()).join(".local");
    fs::create_dir_all(&local).expect("create local build directory");
    let path = local.join("room.key");
    if !path.exists() {
        let mut key = [0_u8; 32];
        getrandom::getrandom(&mut key).expect("Windows random source");
        fs::write(&path, key).expect("save private room key");
    }
    let key = fs::read(&path).expect("read private room key");
    assert_eq!(
        key.len(),
        32,
        ".local/room.key must contain exactly 32 bytes"
    );
    fs::write(
        PathBuf::from(env::var_os("OUT_DIR").unwrap()).join("room.key"),
        key,
    )
    .expect("embed room key");
    println!("cargo:rerun-if-changed={}", path.display());
    println!("cargo:rerun-if-changed=build.rs");
}
