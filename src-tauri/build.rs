fn main() {
    // rustc's MSVC linker currently ignores `strip` and still requests a PDB.
    if std::env::var("PROFILE").as_deref() == Ok("release")
        && std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc")
    {
        println!("cargo:rustc-link-arg-bin=GitBoost=/DEBUG:NONE");
    }
    tauri_build::build()
}
