use std::path::PathBuf;

use uuid::Uuid;

use crate::config::{data_dir, ensure_data_dir};

pub fn list_conversations() -> Result<Vec<vanyline_lib::Conversation>, std::io::Error> {
    let dir = data_dir().join("conversations");
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut convs = Vec::new();
    for entry in std::fs::read_dir(&dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().is_some_and(|ext| ext == "json")
            && let Ok(c) = load_json::<vanyline_lib::Conversation>(&path)
        {
            convs.push(c);
        }
    }
    convs.sort_by_key(|c| c.id);
    Ok(convs)
}

pub fn get_conversation(id: &Uuid) -> Result<vanyline_lib::Conversation, std::io::Error> {
    let path = data_dir().join("conversations").join(format!("{id}.json"));
    load_json(&path)
}

pub fn save_conversation(conv: &vanyline_lib::Conversation) -> Result<(), std::io::Error> {
    ensure_data_dir();
    let path = data_dir()
        .join("conversations")
        .join(format!("{}.json", conv.id));
    save_json(&path, conv)
}

pub fn delete_conversation(id: &Uuid) -> Result<(), std::io::Error> {
    let path = data_dir().join("conversations").join(format!("{id}.json"));
    if path.exists() {
        std::fs::remove_file(path)?;
    }
    Ok(())
}

pub fn set_active_conversation(id: &Uuid) -> Result<(), std::io::Error> {
    ensure_data_dir();
    let path = data_dir().join("active-conversation.json");
    save_json(&path, id)
}

pub fn get_active_conversation() -> Option<Uuid> {
    let path = data_dir().join("active-conversation.json");
    if path.exists() {
        load_json(&path).ok()
    } else {
        None
    }
}

fn load_json<T: serde::de::DeserializeOwned>(path: &PathBuf) -> Result<T, std::io::Error> {
    let content = std::fs::read_to_string(path)?;
    let val: T = serde_json::from_str(&content)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))?;
    Ok(val)
}

fn save_json<T: serde::Serialize>(path: &PathBuf, val: &T) -> Result<(), std::io::Error> {
    let content = serde_json::to_string_pretty(val)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))?;
    std::fs::write(path, content)?;
    Ok(())
}

/// Résout une référence utilisateur vers l'`Uuid` d'une conversation dans
/// `convs` : soit un index **1-based** (position dans `convs`, l'appelant
/// est responsable d'utiliser LE MÊME ordre que celui affiché par
/// `conversations list`), soit un préfixe (partiel ou complet,
/// insensible à la casse) de l'UUID textuel. Erreur si aucune
/// correspondance, ou si un préfixe correspond à plusieurs conversations
/// (ambigu). Fonction pure — ne touche pas au disque, ne connaît pas
/// `data_dir()` ; c'est l'appelant qui charge `convs` via
/// `list_conversations()`.
pub fn resolve_conversation_reference(
    convs: &[vanyline_lib::Conversation],
    reference: &str,
) -> Result<Uuid, String> {
    // Seule une chaîne décimale propre (pas de zéros leading) est interprétée
    // comme un index 1-based. Les zéros leading font tomber dans le matching préfixe.
    let is_clean_decimal = reference.chars().all(|c| c.is_ascii_digit())
        && !reference.is_empty()
        && (reference.len() == 1 || reference.as_bytes()[0] != b'0');

    if is_clean_decimal && let Ok(index) = reference.parse::<usize>() {
        if index >= 1 && index <= convs.len() {
            return Ok(convs[index - 1].id);
        }
        return Err(format!(
            "No conversation at index {index} (have {})",
            convs.len()
        ));
    }

    let lower = reference.to_lowercase();
    let matches: Vec<&vanyline_lib::Conversation> = convs
        .iter()
        .filter(|c| c.id.to_string().starts_with(&lower))
        .collect();

    match matches.len() {
        0 => Err(format!("No conversation matches '{reference}'")),
        1 => Ok(matches[0].id),
        _ => Err(format!(
            "Ambiguous reference '{reference}': matches {} conversations",
            matches.len()
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_conv(id: Uuid) -> vanyline_lib::Conversation {
        vanyline_lib::Conversation {
            id,
            agent: None,
            title: None,
            messages: Vec::new(),
            todo: None,
        }
    }

    #[test]
    fn resolves_by_one_based_index() {
        let convs = vec![
            make_conv(Uuid::parse_str("aaaaaaaa-0000-0000-0000-000000000001").unwrap()),
            make_conv(Uuid::parse_str("aaaaaaaa-0000-0000-0000-000000000002").unwrap()),
            make_conv(Uuid::parse_str("aaaaaaaa-0000-0000-0000-000000000003").unwrap()),
        ];
        let result = resolve_conversation_reference(&convs, "2").unwrap();
        assert_eq!(result, convs[1].id);
    }

    #[test]
    fn index_zero_errors() {
        let convs = vec![make_conv(uuid::Uuid::new_v4())];
        let result = resolve_conversation_reference(&convs, "0");
        assert!(result.is_err());
    }

    #[test]
    fn index_out_of_range_errors() {
        let convs = vec![
            make_conv(uuid::Uuid::new_v4()),
            make_conv(uuid::Uuid::new_v4()),
        ];
        let result = resolve_conversation_reference(&convs, "5");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("have 2"));
    }

    #[test]
    fn resolves_by_uuid_prefix() {
        let convs = vec![
            make_conv(Uuid::parse_str("aaaaaaaa-0000-0000-0000-000000000001").unwrap()),
            make_conv(Uuid::parse_str("bbbbbbbb-0000-0000-0000-000000000002").unwrap()),
        ];
        let result = resolve_conversation_reference(&convs, "aaaaaaaa").unwrap();
        assert_eq!(result, convs[0].id);
    }

    #[test]
    fn resolves_by_full_uuid() {
        let id = Uuid::parse_str("aaaaaaaa-0000-0000-0000-000000000001").unwrap();
        let convs = vec![make_conv(id)];
        let result = resolve_conversation_reference(&convs, &id.to_string()).unwrap();
        assert_eq!(result, id);
    }

    #[test]
    fn unknown_reference_errors() {
        let convs = vec![make_conv(Uuid::new_v4())];
        let result = resolve_conversation_reference(&convs, "zzz");
        assert!(result.is_err());
    }

    #[test]
    fn ambiguous_prefix_errors() {
        let convs = vec![
            make_conv(Uuid::parse_str("00000000-0000-0000-0000-000000000001").unwrap()),
            make_conv(Uuid::parse_str("00000000-0000-0000-0000-000000000002").unwrap()),
        ];
        let result = resolve_conversation_reference(&convs, "00000000");
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.contains("Ambiguous"));
        assert!(err.contains("matches 2"));
    }
}
