use std::path::PathBuf;

pub fn config_dir() -> PathBuf {
    let base = dirs::config_dir().unwrap_or_else(|| PathBuf::from("~/.config"));
    base.join("vanyline")
}

pub fn ensure_config_dir() {
    std::fs::create_dir_all(config_dir()).unwrap_or_else(|e| {
        eprintln!("Failed to create config dir: {e}");
        std::process::exit(1);
    });
    std::fs::create_dir_all(config_dir().join("conversations")).unwrap_or_else(|e| {
        eprintln!("Failed to create conversations dir: {e}");
        std::process::exit(1);
    });
}
