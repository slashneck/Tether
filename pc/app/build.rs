fn main() {
    // The exe icon and version info for Explorer.
    println!("cargo:rerun-if-changed=tether.rc");
    println!("cargo:rerun-if-changed=../../assets/tether.ico");
    embed_resource::compile("tether.rc", embed_resource::NONE).manifest_optional().unwrap();
}
