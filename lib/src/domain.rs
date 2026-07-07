use std::collections::BTreeMap;

use serde::{Deserialize, Deserializer, Serialize, Serializer, ser::SerializeSeq};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Provider {
    pub name: String,
    #[serde(rename = "type")]
    pub provider_type: ProviderType,
    pub endpoint: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ProviderType {
    Ollama,
    OpenaiCompatible,
}

/// Un modèle PARAMÉTRÉ : la seule chose qu'un agent peut référencer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelProfile {
    pub name: String,
    pub provider: String,
    #[serde(rename = "model")]
    pub model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u64>,
    /// Passthrough provider-spécifique (ex. ollama: num_ctx, top_k).
    #[serde(
        default,
        skip_serializing_if = "serde_json::Map::is_empty",
        serialize_with = "serialize_options",
        deserialize_with = "deserialize_options"
    )]
    pub options: serde_json::Map<String, serde_json::Value>,
}

fn serialize_options<S>(
    options: &serde_json::Map<String, serde_json::Value>,
    serializer: S,
) -> std::result::Result<S::Ok, S::Error>
where
    S: Serializer,
{
    use serde::ser::SerializeMap;
    let mut map = serializer.serialize_map(Some(options.len()))?;
    for (k, v) in options {
        map.serialize_entry(k, v)?;
    }
    map.end()
}

fn deserialize_options<'de, D>(deserializer: D) -> std::result::Result<serde_json::Map<String, serde_json::Value>, D::Error>
where
    D: Deserializer<'de>,
{
    let value = serde_json::Value::deserialize(deserializer)?;
    match value {
        serde_json::Value::Object(map) => Ok(map),
        _ => Err(serde::de::Error::custom(format!(
            "expected object for 'options', got {}",
            value
        ))),
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServer {
    pub name: String,
    #[serde(rename = "type")]
    pub transport: McpTransport,
    pub url: String,
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum McpTransport {
    HttpStreamable,
}

/// Groupe cohérent d'outils + fragment de prompt système.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Toolset {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
    #[serde(default)]
    pub local_tools: Vec<String>,
    #[serde(default)]
    pub mcp: Vec<McpSelection>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpSelection {
    pub server: String,
    /// Patterns glob sur les noms d'outils du serveur. Vide = tous ("*").
    #[serde(default)]
    pub tools: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillMeta {
    pub name: String,
    pub description: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentMode {
    Primary,
    Subagent,
    All,
}

/// `auto` (tous les skills), `none`, ou une liste explicite de noms.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum SkillSelection {
    #[default]
    Auto,
    None,
    Named(Vec<String>),
}

impl Serialize for SkillSelection {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            SkillSelection::Auto => serializer.serialize_str("auto"),
            SkillSelection::None => serializer.serialize_str("none"),
            SkillSelection::Named(names) => {
                let mut seq = serializer.serialize_seq(Some(names.len()))?;
                for name in names {
                    seq.serialize_element(name)?;
                }
                seq.end()
            }
        }
    }
}

impl<'de> Deserialize<'de> for SkillSelection {
    fn deserialize<D>(deserializer: D) -> std::result::Result<SkillSelection, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct SkillSelectionVisitor;

        impl<'de> serde::de::Visitor<'de> for SkillSelectionVisitor {
            type Value = SkillSelection;

            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                write!(formatter, "a string (\"auto\" or \"none\") or an array of skill names")
            }

            fn visit_str<E>(self, value: &str) -> std::result::Result<SkillSelection, E>
            where
                E: serde::de::Error,
            {
                match value {
                    "auto" => Ok(SkillSelection::Auto),
                    "none" => Ok(SkillSelection::None),
                    other => Err(serde::de::Error::custom(format!(
                        "expected \"auto\" or \"none\", got {:?}",
                        other
                    ))),
                }
            }

            fn visit_seq<A>(self, mut seq: A) -> std::result::Result<SkillSelection, A::Error>
            where
                A: serde::de::SeqAccess<'de>,
            {
                let mut names = Vec::new();
                while let Some(name) = seq.next_element::<String>()? {
                    names.push(name);
                }
                Ok(SkillSelection::Named(names))
            }
        }

        deserializer.deserialize_any(SkillSelectionVisitor)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Agent {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default = "default_agent_mode")]
    pub mode: AgentMode,
    pub model: String,
    #[serde(default)]
    pub toolsets: Vec<String>,
    #[serde(default)]
    pub skills: SkillSelection,
    pub system_prompt: String,
}

fn default_agent_mode() -> AgentMode {
    AgentMode::Primary
}

#[cfg(test)]
mod tests {
    use crate::VnyError;
    use super::*;

    #[test]
    fn provider_type_serde() {
        assert_eq!(
            serde_json::to_value(ProviderType::OpenaiCompatible).unwrap(),
            serde_json::json!("openai-compatible")
        );
        assert_eq!(
            serde_json::to_value(ProviderType::Ollama).unwrap(),
            serde_json::json!("ollama")
        );

        let openai: ProviderType = serde_json::from_str("\"openai-compatible\"").unwrap();
        assert_eq!(openai, ProviderType::OpenaiCompatible);

        let ollama: ProviderType = serde_json::from_str("\"ollama\"").unwrap();
        assert_eq!(ollama, ProviderType::Ollama);
    }

    #[test]
    fn mcp_transport_serde() {
        assert_eq!(
            serde_json::to_value(McpTransport::HttpStreamable).unwrap(),
            serde_json::json!("http-streamable")
        );

        let transport: McpTransport = serde_json::from_str("\"http-streamable\"").unwrap();
        assert_eq!(transport, McpTransport::HttpStreamable);
    }

    #[test]
    fn agent_mode_serde() {
        assert_eq!(
            serde_json::to_value(AgentMode::Primary).unwrap(),
            serde_json::json!("primary")
        );
        assert_eq!(
            serde_json::to_value(AgentMode::Subagent).unwrap(),
            serde_json::json!("subagent")
        );
        assert_eq!(
            serde_json::to_value(AgentMode::All).unwrap(),
            serde_json::json!("all")
        );

        let p: AgentMode = serde_json::from_str("\"primary\"").unwrap();
        assert_eq!(p, AgentMode::Primary);
        let s: AgentMode = serde_json::from_str("\"subagent\"").unwrap();
        assert_eq!(s, AgentMode::Subagent);
        let a: AgentMode = serde_json::from_str("\"all\"").unwrap();
        assert_eq!(a, AgentMode::All);
    }

    #[test]
    fn skill_selection_serde() {
        // "auto" roundtrip
        let auto_json = " \"auto\"";
        let val: SkillSelection = serde_json::from_str(auto_json).unwrap();
        assert_eq!(val, SkillSelection::Auto);
        let back = serde_json::to_value(&val).unwrap();
        assert_eq!(back, serde_json::json!("auto"));

        // "none" roundtrip
        let none_json = "\"none\"";
        let val: SkillSelection = serde_json::from_str(none_json).unwrap();
        assert_eq!(val, SkillSelection::None);
        let back = serde_json::to_value(&val).unwrap();
        assert_eq!(back, serde_json::json!("none"));

        // Array roundtrip
        let sel_json = r#"["a","b"]"#;
        let val: SkillSelection = serde_json::from_str(sel_json).unwrap();
        assert_eq!(
            val,
            SkillSelection::Named(vec!["a".to_string(), "b".to_string()])
        );
        let sel_back = serde_json::to_string(&val).unwrap();
        assert_eq!(sel_back, r#"["a","b"]"#);

        // Error on unknown string
        let err: Result<SkillSelection, _> = serde_json::from_str("\"autre\"");
        assert!(err.is_err());
    }

    #[test]
    fn agent_defaults() {
        let json = r#"{"name":"a","model":"m","system_prompt":"p"}"#;
        let agent: Agent = serde_json::from_str(json).unwrap();

        assert_eq!(agent.name, "a");
        assert_eq!(agent.model, "m");
        assert_eq!(agent.system_prompt, "p");
        assert_eq!(agent.mode, AgentMode::Primary);
        assert!(agent.toolsets.is_empty());
        assert_eq!(agent.skills, SkillSelection::Auto);
        assert!(agent.description.is_none());
    }

    #[test]
    fn model_profile_roundtrip() {
        // Full profile with options
        let json = r#"{"name":"my-model","provider":"ollama","model":"qwen2.5","temperature":0.7,"max_tokens":4096,"options":{"num_ctx":65536}}"#;
        let profile: ModelProfile = serde_json::from_str(json).unwrap();

        assert_eq!(profile.name, "my-model");
        assert_eq!(profile.provider, "ollama");
        assert_eq!(profile.model, "qwen2.5");
        assert_eq!(profile.temperature, Some(0.7));
        assert_eq!(profile.max_tokens, Some(4096));
        assert_eq!(profile.options.get("num_ctx").unwrap().as_u64().unwrap(), 65536);

        let back = serde_json::to_string(&profile).unwrap();
        let parsed: ModelProfile = serde_json::from_str(&back).unwrap();
        assert_eq!(parsed, profile);

        // Minimal (only name/provider/model) serializes without optional fields
        let minimal_json = r#"{"name":"min","provider":"p","model":"m"}"#;
        let minimal: ModelProfile = serde_json::from_str(minimal_json).unwrap();
        assert_eq!(minimal.temperature, None);
        assert_eq!(minimal.max_tokens, None);
        assert!(minimal.options.is_empty());

        let serialized = serde_json::to_value(&minimal).unwrap();
        let opt_fields = &["temperature", "max_tokens", "options"];
        for field in opt_fields {
            assert!(!serialized.as_object().unwrap().contains_key(*field),
                "field '{}' should not be serialized for minimal profile", field);
        }
    }

    #[test]
    fn toolset_defaults() {
        let json = r#"{"name":"t"}"#;
        let toolset: Toolset = serde_json::from_str(json).unwrap();

        assert_eq!(toolset.name, "t");
        assert!(toolset.description.is_none());
        assert!(toolset.prompt.is_none());
        assert!(toolset.local_tools.is_empty());
        assert!(toolset.mcp.is_empty());
    }

    #[test]
    fn mcp_selection_empty_tools() {
        let json = r#"{"server":"s"}"#;
        let sel: McpSelection = serde_json::from_str(json).unwrap();
        assert_eq!(sel.tools, Vec::<String>::new());

        // Roundtrip keeps tools as empty array
        let back = serde_json::to_value(&sel).unwrap();
        assert!(back.as_object().unwrap().get("tools").unwrap().is_array());
    }

    #[test]
    fn error_codes() {
        let e2 = VnyError::DuplicateName("agent", "build".to_string());
        let msg2 = format!("{}", e2);
        assert!(msg2.contains("VNL-CFG-002"));

        let e3 = VnyError::UnknownReference("model", "qwen-code".to_string());
        let msg3 = format!("{}", e3);
        assert!(msg3.contains("VNL-CFG-003"));
    }
}
