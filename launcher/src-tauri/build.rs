fn main() {
    println!("cargo:rerun-if-env-changed=FZH_DEFAULT_SERVER");
    for dir in ["bundle/emulator", "bundle/licences"] {
        std::fs::create_dir_all(dir).expect("cannot create the bundle resource folders");
    }
    tauri_build::build()
}
