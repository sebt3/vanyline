use serde_json::json;

/// `read_file`, `write_file`, `edit_file`, `delete_file`, `list_directory`.
#[must_use]
pub fn filesystem_tools() -> Vec<serde_json::Value> {
    vec![
        json!({
            "name": "read_file",
            "description": "Read a file as numbered lines. Example: {\"path\": \"src/main.rs\"}",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "Path to the file"},
                    "offset": {"type": "integer", "description": "0-based line to start from (default 0)"},
                    "limit": {"type": "integer", "description": "Max lines to return (default 200)"}
                },
                "required": ["path"]
            }
        }),
        json!({
            "name": "write_file",
            "description": "Write content to a file, creating parent directories if needed. Example: {\"path\": \"src/foo.rs\", \"content\": \"fn main() {}\"}",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "Path to the file"},
                    "content": {"type": "string", "description": "File content"}
                },
                "required": ["path", "content"]
            }
        }),
        json!({
            "name": "edit_file",
            "description": "Replace an exact string in a file. Fails if old_string is missing, or found more than once unless replace_all is set. Example: {\"path\": \"src/foo.rs\", \"old_string\": \"foo\", \"new_string\": \"bar\"}",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "Path to the file"},
                    "old_string": {"type": "string", "description": "Exact text to replace"},
                    "new_string": {"type": "string", "description": "Replacement text"},
                    "replace_all": {"type": "boolean", "description": "Replace every occurrence (default false)"}
                },
                "required": ["path", "old_string", "new_string"]
            }
        }),
        json!({
            "name": "delete_file",
            "description": "Delete a file or an empty directory. Example: {\"path\": \"src/old.rs\"}",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "Path to the file or empty directory"}
                },
                "required": ["path"]
            }
        }),
        json!({
            "name": "list_directory",
            "description": "List a directory as a compact tree. Example: {\"path\": \"src\"}",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "Path to the directory"},
                    "depth": {"type": "integer", "description": "Levels to descend (default 1)"}
                },
                "required": ["path"]
            }
        }),
    ]
}

/// `find_files`, search.
#[must_use]
pub fn search_tools() -> Vec<serde_json::Value> {
    vec![
        json!({
            "name": "find_files",
            "description": "Find files by glob pattern. Example: {\"pattern\": \"**/*.rs\"}",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "pattern": {"type": "string", "description": "Glob pattern, e.g. **/*.rs"},
                    "path": {"type": "string", "description": "Root directory (default: current directory)"}
                },
                "required": ["pattern"]
            }
        }),
        json!({
            "name": "search",
            "description": "Search file contents by regex. Example: {\"pattern\": \"fn \\\\w+\"}",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "pattern": {"type": "string", "description": "Regex pattern"},
                    "path": {"type": "string", "description": "Root directory (default: current directory)"},
                    "glob": {"type": "string", "description": "Restrict to files matching this glob (default: all files)"}
                },
                "required": ["pattern"]
            }
        }),
    ]
}

/// `execute_command`.
#[must_use]
pub fn command_tools() -> Vec<serde_json::Value> {
    vec![json!({
        "name": "execute_command",
        "description": "Execute a shell command and capture output. Example: {\"command\": \"cargo test\"}",
        "inputSchema": {
            "type": "object",
            "properties": {
                "command": {"type": "string", "description": "Shell command to execute"},
                "timeout_secs": {"type": "integer", "description": "Timeout in seconds (default 30, 0 = no timeout)"},
                "cwd": {"type": "string", "description": "Working directory (default: current directory)"}
            },
            "required": ["command"]
        }
    })]
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    #[test]
    fn filesystem_tools_surface() {
        let tools = filesystem_tools();
        assert_eq!(tools.len(), 5);
        let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert!(names.contains(&"read_file"));
        assert!(names.contains(&"write_file"));
        assert!(names.contains(&"edit_file"));
        assert!(names.contains(&"delete_file"));
        assert!(names.contains(&"list_directory"));
    }

    #[test]
    fn search_tools_surface() {
        let tools = search_tools();
        assert_eq!(tools.len(), 2);
        let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert!(names.contains(&"find_files"));
        assert!(names.contains(&"search"));
    }

    #[test]
    fn command_tools_surface() {
        let tools = command_tools();
        assert_eq!(tools.len(), 1);
        let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert!(names.contains(&"execute_command"));
    }

    #[test]
    fn edit_file_required_fields() {
        let tools = filesystem_tools();
        let edit_file = tools.iter().find(|t| t["name"] == "edit_file").unwrap();
        let required = edit_file["inputSchema"]["required"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(required, vec!["path", "old_string", "new_string"]);
        // replace_all should NOT be in required
        assert!(!required.contains(&"replace_all"));
    }

    #[test]
    fn read_file_required_fields() {
        let tools = filesystem_tools();
        let read_file = tools.iter().find(|t| t["name"] == "read_file").unwrap();
        let required = read_file["inputSchema"]["required"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(required, vec!["path"]);
        // offset and limit should NOT be in required
        assert!(!required.contains(&"offset"));
        assert!(!required.contains(&"limit"));
    }

    #[test]
    fn no_create_directory_schema() {
        let tools = filesystem_tools();
        let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert!(!names.contains(&"create_directory"));
    }

    fn all_tools() -> Vec<serde_json::Value> {
        let mut all = filesystem_tools();
        all.extend(search_tools());
        all.extend(command_tools());
        all
    }

    fn names_of(tools: &[serde_json::Value]) -> Vec<&str> {
        tools.iter().map(|t| t["name"].as_str().unwrap()).collect()
    }

    fn required_of(tool: &serde_json::Value) -> Vec<&str> {
        tool["inputSchema"]["required"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect()
    }

    fn find<'a>(tools: &'a [serde_json::Value], name: &str) -> &'a serde_json::Value {
        tools.iter().find(|t| t["name"] == name).unwrap()
    }

    /// Scenario `filesystem_tools_cinq_descripteurs_dans_ordre_fixe`.
    #[test]
    fn filesystem_tools_cinq_descripteurs_dans_ordre_fixe() {
        let tools = filesystem_tools();
        assert_eq!(tools.len(), 5);
        assert_eq!(
            names_of(&tools),
            vec![
                "read_file",
                "write_file",
                "edit_file",
                "delete_file",
                "list_directory"
            ]
        );
    }

    /// Scenario `search_tools_deux_descripteurs_dans_ordre_fixe`.
    #[test]
    fn search_tools_deux_descripteurs_dans_ordre_fixe() {
        let tools = search_tools();
        assert_eq!(tools.len(), 2);
        assert_eq!(names_of(&tools), vec!["find_files", "search"]);
    }

    /// Scenario `command_tools_unique_execute_command`.
    #[test]
    fn command_tools_unique_execute_command() {
        let tools = command_tools();
        assert_eq!(tools.len(), 1);
        assert_eq!(names_of(&tools), vec!["execute_command"]);
    }

    /// Scenario `huit_noms_de_tools_uniques`.
    #[test]
    fn huit_noms_de_tools_uniques() {
        let all = all_tools();
        let names = names_of(&all);
        assert_eq!(names.len(), 8);
        let mut sorted = names.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), 8, "noms dupliqués : {names:?}");
        let mut expected = vec![
            "read_file",
            "write_file",
            "edit_file",
            "delete_file",
            "list_directory",
            "find_files",
            "search",
            "execute_command",
        ];
        expected.sort_unstable();
        assert_eq!(sorted, expected);
    }

    /// Scenario `descripteur_de_trois_cles_seulement`.
    #[test]
    fn descripteur_de_trois_cles_seulement() {
        for tool in all_tools() {
            let obj = tool.as_object().unwrap();
            let mut keys: Vec<&str> = obj.keys().map(String::as_str).collect();
            keys.sort_unstable();
            assert_eq!(keys, vec!["description", "inputSchema", "name"]);
            assert!(!tool["description"].as_str().unwrap().is_empty());

            let schema = tool["inputSchema"].as_object().unwrap();
            let mut schema_keys: Vec<&str> = schema.keys().map(String::as_str).collect();
            schema_keys.sort_unstable();
            assert_eq!(schema_keys, vec!["properties", "required", "type"]);
            assert_eq!(tool["inputSchema"]["type"], "object");
            assert!(!required_of(&tool).is_empty());
        }
    }

    /// Scenario `required_de_read_file_est_path`.
    #[test]
    fn required_de_read_file_est_path() {
        let tools = filesystem_tools();
        let read = find(&tools, "read_file");
        assert_eq!(required_of(read), vec!["path"]);
        assert_eq!(
            read["inputSchema"]["properties"]["offset"]["type"],
            "integer"
        );
        assert_eq!(
            read["inputSchema"]["properties"]["limit"]["type"],
            "integer"
        );
        assert!(!required_of(read).contains(&"offset"));
        assert!(!required_of(read).contains(&"limit"));
    }

    /// Scenario `required_de_write_file_est_path_et_content`.
    #[test]
    fn required_de_write_file_est_path_et_content() {
        let tools = filesystem_tools();
        let write = find(&tools, "write_file");
        assert_eq!(required_of(write), vec!["path", "content"]);
        assert_eq!(write["inputSchema"]["properties"]["path"]["type"], "string");
        assert_eq!(
            write["inputSchema"]["properties"]["content"]["type"],
            "string"
        );
    }

    /// Scenario `required_de_edit_file_sans_replace_all`.
    #[test]
    fn required_de_edit_file_sans_replace_all() {
        let tools = filesystem_tools();
        let edit = find(&tools, "edit_file");
        assert_eq!(required_of(edit), vec!["path", "old_string", "new_string"]);
        assert_eq!(
            edit["inputSchema"]["properties"]["replace_all"]["type"],
            "boolean"
        );
        assert!(!required_of(edit).contains(&"replace_all"));
    }

    /// Scenario `required_de_execute_command_est_command`.
    #[test]
    fn required_de_execute_command_est_command() {
        let tools = command_tools();
        let exec = find(&tools, "execute_command");
        assert_eq!(required_of(exec), vec!["command"]);
        assert_eq!(
            exec["inputSchema"]["properties"]["timeout_secs"]["type"],
            "integer"
        );
        assert_eq!(exec["inputSchema"]["properties"]["cwd"]["type"], "string");
        assert!(!required_of(exec).contains(&"timeout_secs"));
        assert!(!required_of(exec).contains(&"cwd"));
    }

    /// Scenario `required_des_outils_restants`.
    #[test]
    fn required_des_outils_restants() {
        let fs = filesystem_tools();
        let search = search_tools();
        assert_eq!(required_of(find(&fs, "delete_file")), vec!["path"]);

        let list = find(&fs, "list_directory");
        assert_eq!(required_of(list), vec!["path"]);
        assert_eq!(
            list["inputSchema"]["properties"]["depth"]["type"],
            "integer"
        );

        let find_f = find(&search, "find_files");
        assert_eq!(required_of(find_f), vec!["pattern"]);
        assert_eq!(
            find_f["inputSchema"]["properties"]["path"]["type"],
            "string"
        );

        let grep = find(&search, "search");
        assert_eq!(required_of(grep), vec!["pattern"]);
        assert_eq!(grep["inputSchema"]["properties"]["path"]["type"], "string");
        assert_eq!(grep["inputSchema"]["properties"]["glob"]["type"], "string");
    }

    /// Scenario `chaque_description_porte_un_exemple_json`.
    #[test]
    fn chaque_description_porte_un_exemple_json() {
        for tool in all_tools() {
            let desc = tool["description"].as_str().unwrap();
            assert!(desc.contains("Example: {"), "pas d'exemple dans {desc:?}");
        }
    }

    /// Scenario `description_de_edit_file_annonce_les_deux_echecs`.
    #[test]
    fn description_de_edit_file_annonce_les_deux_echecs() {
        let tools = filesystem_tools();
        let desc = find(&tools, "edit_file")["description"]
            .as_str()
            .unwrap()
            .to_owned();
        assert!(desc.contains("old_string is missing"));
        assert!(desc.contains("more than once unless replace_all is set"));
    }

    /// Scenario `deux_appels_rendent_des_valeurs_independantes`.
    #[test]
    fn deux_appels_rendent_des_valeurs_independantes() {
        let mut first = filesystem_tools();
        assert_eq!(first.len(), 5);
        first.push(serde_json::json!({"name": "intrus"}));
        first[0] = serde_json::json!(null);

        let second = filesystem_tools();
        assert_eq!(second.len(), 5);
        assert_eq!(names_of(&second)[0], "read_file");
        assert!(!names_of(&second).contains(&"intrus"));
    }

    /// Scenario `absence_de_create_directory`.
    #[test]
    fn absence_de_create_directory() {
        assert!(!names_of(&all_tools()).contains(&"create_directory"));
    }
}
