use crate::domain::{Agent, SkillMeta, SkillSelection, Toolset};

#[cfg(test)]
use crate::domain::AgentMode;

/// Assemble le system prompt final d'un tour d'agent, dans l'ordre fixe décrit
/// par le design (section "Session engine") :
///
/// 1. `agent.system_prompt`
/// 2. `toolset.prompt` de chaque toolset RÉFÉRENCÉ par `agent.toolsets`, DANS
///    L'ORDRE de `agent.toolsets` (pas l'ordre de `all_toolsets`) — un nom de
///    `agent.toolsets` absent de `all_toolsets` est silencieusement ignoré (la
///    validation "toolset inconnu" est la responsabilité de l'appelant via
///    `ConfigStore::get_toolset`, tâche 3 ; cette fonction est pure et ne
///    remonte pas d'erreur)
/// 3. Index des skills — calculé à partir de `agent.skills` et `all_skills` :
///    - `SkillSelection::None` -> pas de section
///    - `SkillSelection::Auto` -> tous les `all_skills`, dans leur ordre d'origine
///    - `SkillSelection::Named(names)` -> les `all_skills` dont le nom est dans
///      `names`, dans l'ordre de `all_skills` (pas l'ordre de `names`)
///      Si la liste résultante est vide (ex. `Auto` mais `all_skills` vide),
///      la section est omise entièrement — pas de bloc vide.
///      Format : une ligne `- {name} : {description}` par skill, PAS de ligne
///      d'en-tête (le design ne mentionne que le format des lignes).
/// 4. `workspace_context` si `Some(s)` et `!s.trim().is_empty()` (une chaîne
///    vide ou uniquement des espaces ne produit pas de section)
///
/// Les sections non vides sont jointes par `"\n\n"` (une ligne blanche entre
/// chaque section). Aucune section vide n'introduit de séparateur superflu
/// (ex. si aucun toolset n'a de prompt ET aucun skill n'est sélectionné, le
/// résultat est juste `agent.system_prompt` + éventuellement le contexte
/// workspace, sans lignes blanches en trop).
pub fn assemble_system_prompt(
    agent: &Agent,
    all_toolsets: &[Toolset],
    all_skills: &[SkillMeta],
    workspace_context: Option<&str>,
) -> String {
    let mut sections = Vec::new();

    // 1. System prompt de l'agent (toujours inclus)
    sections.push(agent.system_prompt.clone());

    // 2. Prompts des toolsets, dans l'ordre de agent.toolsets
    let mut toolset_sections = String::new();
    for name in &agent.toolsets {
        let prompt = all_toolsets
            .iter()
            .find(|t| t.name == *name)
            .and_then(|t| t.prompt.as_ref())
            .map(|p| p.trim())
            .filter(|p| !p.is_empty());
        if let Some(p) = prompt {
            if !toolset_sections.is_empty() {
                toolset_sections.push_str("\n\n");
            }
            toolset_sections.push_str(p);
        }
    }
    if !toolset_sections.is_empty() {
        sections.push(toolset_sections);
    }

    // 3. Index des skills
    let skills_section = match agent.skills {
        SkillSelection::None => None,
        SkillSelection::Auto => {
            let lines: Vec<_> = all_skills
                .iter()
                .map(|s| format!("- {} : {}", s.name, s.description))
                .collect();
            if lines.is_empty() {
                None
            } else {
                Some(lines.join("\n"))
            }
        }
        SkillSelection::Named(ref names) => {
            let name_set: std::collections::HashSet<&str> = names.iter().map(|n| n.as_str()).collect();
            let lines: Vec<_> = all_skills
                .iter()
                .filter(|s| name_set.contains(s.name.as_str()))
                .map(|s| format!("- {} : {}", s.name, s.description))
                .collect();
            if lines.is_empty() {
                None
            } else {
                Some(lines.join("\n"))
            }
        }
    };
    if let Some(section) = skills_section {
        sections.push(section);
    }

    // 4. Workspace context
    if let Some(ctx) = workspace_context {
        let trimmed = ctx.trim();
        if !trimmed.is_empty() {
            sections.push(trimmed.to_string());
        }
    }

    let result = sections.join("\n\n");
    // Si system_prompt était vide et qu'il n'y a rien d'autre, retourner ""
    if agent.system_prompt.is_empty() && sections.len() == 1 {
        String::new()
    } else {
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_agent(toolsets: Vec<String>, skills: SkillSelection) -> Agent {
        Agent {
            name: "test-agent".to_string(),
            description: None,
            mode: AgentMode::Primary,
            model: "qwen".to_string(),
            toolsets,
            skills,
            system_prompt: "You are helpful.".to_string(),
        }
    }

    // 1. system_prompt_only
    #[test]
    fn system_prompt_only() {
        let agent = sample_agent(Vec::new(), SkillSelection::None);
        let result = assemble_system_prompt(&agent, &[], &[], None);
        assert_eq!(result, "You are helpful.");
        assert!(!result.ends_with('\n'));
        assert!(!result.ends_with('\r'));
    }

    // 2. toolset_prompts_in_agent_order
    #[test]
    fn toolset_prompts_in_agent_order() {
        let toolsets = vec![
            Toolset {
                name: "a".to_string(),
                description: None,
                prompt: Some("PROMPT_A".to_string()),
                local_tools: vec![],
                mcp: vec![],
            },
            Toolset {
                name: "b".to_string(),
                description: None,
                prompt: Some("PROMPT_B".to_string()),
                local_tools: vec![],
                mcp: vec![],
            },
        ];
        let agent = sample_agent(vec!["b".to_string(), "a".to_string()], SkillSelection::None);
        let result = assemble_system_prompt(&agent, &toolsets, &[], None);
        let pos_b = result.find("PROMPT_B").unwrap();
        let pos_a = result.find("PROMPT_A").unwrap();
        assert!(pos_b < pos_a);
    }

    // 3. toolset_without_prompt_contributes_nothing
    #[test]
    fn toolset_without_prompt_contributes_nothing() {
        let toolsets = vec![
            Toolset {
                name: "a".to_string(),
                description: None,
                prompt: None,
                local_tools: vec![],
                mcp: vec![],
            },
            Toolset {
                name: "b".to_string(),
                description: None,
                prompt: None,
                local_tools: vec![],
                mcp: vec![],
            },
        ];
        let agent = sample_agent(vec!["a".to_string(), "b".to_string()], SkillSelection::None);
        let result = assemble_system_prompt(&agent, &toolsets, &[], None);
        assert_eq!(result, "You are helpful.");
    }

    // 4. toolset_unknown_reference_ignored
    #[test]
    fn toolset_unknown_reference_ignored() {
        let toolsets: Vec<Toolset> = vec![];
        let agent = sample_agent(vec!["nonexistent".to_string()], SkillSelection::None);
        let result = assemble_system_prompt(&agent, &toolsets, &[], None);
        assert_eq!(result, "You are helpful.");
    }

    // 5. skills_none_no_section
    #[test]
    fn skills_none_no_section() {
        let skills = vec![
            SkillMeta {
                name: "pdf".to_string(),
                description: "PDF processing".to_string(),
            },
            SkillMeta {
                name: "web".to_string(),
                description: "Web search".to_string(),
            },
        ];
        let agent = sample_agent(Vec::new(), SkillSelection::None);
        let result = assemble_system_prompt(&agent, &[], &skills, None);
        assert!(!result.contains("PDF processing"));
        assert!(!result.contains("Web search"));
    }

    // 6. skills_auto_all_included
    #[test]
    fn skills_auto_all_included() {
        let skills = vec![
            SkillMeta {
                name: "pdf".to_string(),
                description: "PDF processing".to_string(),
            },
            SkillMeta {
                name: "web".to_string(),
                description: "Web search".to_string(),
            },
        ];
        let agent = sample_agent(Vec::new(), SkillSelection::Auto);
        let result = assemble_system_prompt(&agent, &[], &skills, None);
        assert!(result.contains("- pdf : PDF processing"));
        assert!(result.contains("- web : Web search"));
        let pos_pdf = result.find("- pdf : PDF processing").unwrap();
        let pos_web = result.find("- web : Web search").unwrap();
        assert!(pos_pdf < pos_web);
    }

    // 7. skills_named_filters
    #[test]
    fn skills_named_filters() {
        let skills = vec![
            SkillMeta {
                name: "pdf".to_string(),
                description: "PDF processing".to_string(),
            },
            SkillMeta {
                name: "web".to_string(),
                description: "Web search".to_string(),
            },
        ];
        let agent = sample_agent(
            Vec::new(),
            SkillSelection::Named(vec!["web".to_string()]),
        );
        let result = assemble_system_prompt(&agent, &[], &skills, None);
        assert!(result.contains("- web : Web search"));
        assert!(!result.contains("PDF processing"));
    }

    // 8. skills_auto_empty_list_no_section
    #[test]
    fn skills_auto_empty_list_no_section() {
        let agent = sample_agent(Vec::new(), SkillSelection::Auto);
        let result = assemble_system_prompt(&agent, &[], &[], None);
        assert_eq!(result, "You are helpful.");
    }

    // 9. workspace_context_included
    #[test]
    fn workspace_context_included() {
        let agent = sample_agent(Vec::new(), SkillSelection::None);
        let ctx = Some("# AGENTS.md\ncontent");
        let result = assemble_system_prompt(&agent, &[], &[], ctx);
        assert!(result.ends_with("content"));
    }

    // 10. workspace_context_blank_ignored
    #[test]
    fn workspace_context_blank_ignored() {
        let agent = sample_agent(Vec::new(), SkillSelection::None);
        let ctx = Some("   ");
        let result = assemble_system_prompt(&agent, &[], &[], ctx);
        assert_eq!(result, "You are helpful.");
    }

    // 11. full_assembly_order
    #[test]
    fn full_assembly_order() {
        let toolset = vec![Toolset {
            name: "shell".to_string(),
            description: None,
            prompt: Some("## Shell tool".to_string()),
            local_tools: vec![],
            mcp: vec![],
        }];
        let skills = vec![SkillMeta {
            name: "web".to_string(),
            description: "Web search".to_string(),
        }];
        let agent = sample_agent(
            vec!["shell".to_string()],
            SkillSelection::Auto,
        );
        let result = assemble_system_prompt(&agent, &toolset, &skills, Some("# AGENTS.md\ncontent"));

        let system_pos = result.find("You are helpful.").unwrap();
        let toolset_pos = result.find("## Shell tool").unwrap();
        let skill_pos = result.find("- web : Web search").unwrap();
        let context_pos = result.find("# AGENTS.md").unwrap();

        assert!(system_pos < toolset_pos);
        assert!(toolset_pos < skill_pos);
        assert!(skill_pos < context_pos);
    }
}
