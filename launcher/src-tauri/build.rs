fn main() {
    println!("cargo:rerun-if-env-changed=FZH_DEFAULT_SERVER");
    println!("cargo:rerun-if-env-changed=FZH_FORMER_SERVERS");
    if std::env::var("FZH_FORMER_SERVERS").is_ok_and(|v| v.contains('/')) {
        panic!("FZH_FORMER_SERVERS: leave out the relay part of each address (the commas separate the addresses)");
    }
    for dir in ["bundle/emulator", "bundle/licences"] {
        std::fs::create_dir_all(dir).expect("cannot create the bundle resource folders");
    }
    tauri_build::build()
}
