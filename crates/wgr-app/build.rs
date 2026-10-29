fn main() {
    let root = std::path::PathBuf::from(
        std::env::var_os("CARGO_MANIFEST_DIR").expect("manifest directory"),
    );
    let icon = root
        .join("../../assets/WGR.ico")
        .canonicalize()
        .expect("WGR icon");
    let out = std::path::PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR"));
    let resource = out.join("wgr.rc");
    let quote = |path: &std::path::Path| {
        path.to_string_lossy()
            .replace('\\', "/")
            .trim_start_matches("//?/")
            .to_string()
    };
    // GPUI's windows-manifest already activates Common Controls v6 and DPI.
    std::fs::write(&resource, format!("1 ICON \"{}\"\n", quote(&icon))).expect("resource source");
    embed_resource::compile(resource, embed_resource::NONE)
        .manifest_required()
        .expect("Windows resources");
    println!("cargo:rerun-if-changed={}", icon.display());
}
