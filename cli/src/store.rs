use std::path::PathBuf;

use uuid::Uuid;

use crate::config::{config_dir, ensure_config_dir};

pub fn list_conversations() -> Result<Vec<vanyline_lib::Conversation>, std::io::Error> {
    let dir = config_dir().join("conversations");
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut convs = Vec::new();
    for entry in std::fs::read_dir(&dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().is_some_and(|ext| ext == "json") {
            if let Ok(c) = load_json::<vanyline_lib::Conversation>(&path) {
                convs.push(c);
            }
        }
    }
    convs.sort_by_key(|c| c.id);
    Ok(convs)
}

pub fn get_conversation(id: &Uuid) -> Result<vanyline_lib::Conversation, std::io::Error> {
    let path = config_dir().join("conversations").join(format!("{id}.json"));
    load_json(&path)
}

pub fn save_conversation(conv: &vanyline_lib::Conversation) -> Result<(), std::io::Error> {
    ensure_config_dir();
    let path = config_dir().join("conversations").join(format!("{}.json", conv.id));
    save_json(&path, conv)
}

pub fn delete_conversation(id: &Uuid) -> Result<(), std::io::Error> {
    let path = config_dir().join("conversations").join(format!("{id}.json"));
    if path.exists() {
        std::fs::remove_file(path)?;
    }
    Ok(())
}

pub fn set_active_conversation(id: &Uuid) -> Result<(), std::io::Error> {
    ensure_config_dir();
    let path = config_dir().join("active-conversation.json");
    save_json(&path, id)
}

pub fn get_active_conversation() -> Option<Uuid> {
    let path = config_dir().join("active-conversation.json");
    if path.exists() {
        load_json(&path).ok()
    } else {
        None
    }
}

fn load_json<T: serde::de::DeserializeOwned>(path: &PathBuf) -> Result<T, std::io::Error> {
    let content = std::fs::read_to_string(path)?;
    let val: T = serde_json::from_str(&content).map_err(|e| {
        std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string())
    })?;
    Ok(val)
}

fn save_json<T: serde::Serialize>(path: &PathBuf, val: &T) -> Result<(), std::io::Error> {
    let content = serde_json::to_string_pretty(val).map_err(|e| {
        std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string())
    })?;
    std::fs::write(path, content)?;
    Ok(())
}
