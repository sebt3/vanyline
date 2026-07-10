use serde_json::json;

/// read_file, write_file, edit_file, delete_file, list_directory.
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

/// find_files, search.
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

/// execute_command.
pub fn command_tools() -> Vec<serde_json::Value> {
    vec![
        json!({
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
        }),
    ]
}

#[cfg(test)]
mod tests {
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
}
