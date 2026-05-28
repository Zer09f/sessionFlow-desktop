fn main() {
    // Workaround: GCC 15's preprocessor rejects non-C tokens in RC files,
    // causing windres "preprocessing failed" when embed-resource compiles
    // the .rc file. We prepend a wrapper directory to PATH that contains
    // a `windres` script passing --preprocessor=cat to the real windres.
    if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("gnu") {
        let wrapper_dir = std::path::PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap())
            .join(".cargo")
            .join("bin");
        let current_path = std::env::var("PATH").unwrap_or_default();
        let new_path = if cfg!(target_os = "windows") {
            format!("{};{}", wrapper_dir.display(), current_path)
        } else {
            format!("{}:{}", wrapper_dir.display(), current_path)
        };
        std::env::set_var("PATH", &new_path);
    }
    tauri_build::build();
}
