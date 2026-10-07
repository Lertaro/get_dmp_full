fn main() {
    println!("cargo:rerun-if-changed=build.rs");

    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows")
        && std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc")
    {
        // Embed the UAC requirement so Windows requests elevation before main runs.
        println!("cargo:rustc-link-arg-bin=get_dump_full=/MANIFEST:EMBED");
        println!(
            "cargo:rustc-link-arg-bin=get_dump_full=/MANIFESTUAC:level='requireAdministrator' uiAccess='false'"
        );
    }
}
