use serde_json::json;

pub fn filesystem_tools() -> Vec<serde_json::Value> {
    vec![
        json!({
            "name": "read_file",
            "description": "Read the contents of a file",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Path to the file" }
                },
                "required": ["path"]
            }
        }),
        json!({
            "name": "write_file",
            "description": "Write content to a file, creating it if necessary",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Path to the file" },
                    "content": { "type": "string", "description": "File content" }
                },
                "required": ["path", "content"]
            }
        }),
        json!({
            "name": "delete_file",
            "description": "Delete a file",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Path to the file" }
                },
                "required": ["path"]
            }
        }),
        json!({
            "name": "create_directory",
            "description": "Create a directory, including parents if needed",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Path to the directory" }
                },
                "required": ["path"]
            }
        }),
        json!({
            "name": "list_directory",
            "description": "List the contents of a directory",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Path to the directory" }
                },
                "required": ["path"]
            }
        }),
    ]
}

pub fn command_tools() -> Vec<serde_json::Value> {
    vec![
        json!({
            "name": "execute_command",
            "description": "Execute a shell command and capture output",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "command": { "type": "string", "description": "Shell command to execute" },
                    "timeout_secs": {
                        "type": "integer",
                        "description": "Timeout in seconds (default: 30)"
                    }
                },
                "required": ["command"]
            }
        }),
    ]
}
