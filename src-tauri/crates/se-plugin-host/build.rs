fn main() {
    // Cargo's target is authoritative, including ABI and vendor components.
    let target = std::env::var("TARGET").expect("Cargo must provide TARGET");
    println!("cargo:rustc-env=SE_PLUGIN_TARGET={target}");
    println!("cargo:rerun-if-env-changed=TARGET");
}
