//! Model-facing tool specs (names, descriptions, JSON Schemas).

use serde_json::json;

fn spec(name: &str, description: &str, input_schema: serde_json::Value) -> llm::ToolSpec {
    llm::ToolSpec { name: name.to_string(), description: description.to_string(), input_schema }
}

pub(crate) fn tool_specs() -> Vec<llm::ToolSpec> {
    vec![
        spec(
            "read_file",
            "Read a text file from the workspace. `path` is relative to the workspace root. \
             Returns lines prefixed with their 1-based line numbers (`  12\\tcode`); the number \
             and tab are not part of the file. Returns at most 2000 lines / 256 KB per call; use \
             `offset` (1-based first line) and `limit` (number of lines) to page through large \
             files. Binary files are refused. Always read a file before editing it.",
            json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "File path relative to the workspace root, e.g. `src/main.rs`." },
                    "offset": { "type": "integer", "minimum": 1, "description": "1-based line number to start reading from (default 1)." },
                    "limit": { "type": "integer", "minimum": 1, "description": "Maximum number of lines to return (default 2000)." }
                },
                "required": ["path"],
                "additionalProperties": false
            }),
        ),
        spec(
            "list_dir",
            "List the entries of a workspace directory, sorted; directories end with `/`. \
             `path` is relative to the workspace root (default: the root itself). `.git`, \
             `target` and `node_modules` are skipped; at most 500 entries are shown.",
            json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Directory relative to the workspace root (default `.`)." }
                },
                "additionalProperties": false
            }),
        ),
        spec(
            "grep",
            "Search file contents with a regular expression (Rust `regex` syntax; use `(?i)` for \
             case-insensitive). Searches recursively under `path` (relative to the workspace \
             root, default the root), respecting .gitignore and skipping hidden and binary \
             files. Optional `glob` restricts which files are searched, e.g. `*.rs` or \
             `src/**/*.ts` (comma-separate several). Output lines are `path:line: text`; at \
             most 200 matches are returned, so prefer specific patterns.",
            json!({
                "type": "object",
                "properties": {
                    "pattern": { "type": "string", "description": "Regular expression to search for." },
                    "path": { "type": "string", "description": "File or directory to search, relative to the workspace root (default `.`)." },
                    "glob": { "type": "string", "description": "Glob filter for file paths, e.g. `*.rs` or `crates/**/*.toml`." }
                },
                "required": ["pattern"],
                "additionalProperties": false
            }),
        ),
        spec(
            "edit_file",
            "Edit an existing file by replacing one exact occurrence of `old_string` with \
             `new_string`. Rules: read the file with read_file first; `old_string` must match \
             the file content exactly (whitespace and indentation included, without the line \
             number prefixes from read_file) and must occur exactly once — include enough \
             surrounding lines to make it unique. The edit fails if it is not found or is found \
             more than once. To delete text, use an empty `new_string`. Use write_file to \
             create new files. The user may be asked to approve the change.",
            json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "File path relative to the workspace root." },
                    "old_string": { "type": "string", "description": "Exact text to replace; must be unique in the file." },
                    "new_string": { "type": "string", "description": "Replacement text." }
                },
                "required": ["path", "old_string", "new_string"],
                "additionalProperties": false
            }),
        ),
        spec(
            "write_file",
            "Create a file or overwrite it entirely with `content`. `path` is relative to the \
             workspace root; missing parent directories are created. Prefer edit_file for \
             changes to existing files; if you must overwrite one, read it first. The user may \
             be asked to approve the change.",
            json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "File path relative to the workspace root." },
                    "content": { "type": "string", "description": "Full new content of the file." }
                },
                "required": ["path", "content"],
                "additionalProperties": false
            }),
        ),
        spec(
            "bash",
            "Run a shell command with `sh -c` in the workspace root (each call starts fresh \
             there; `cd` does not persist). Stdin is closed, so commands must be \
             non-interactive. Returns combined stdout/stderr (capped at 30 KB: head and tail \
             are kept) and the exit code. Default timeout 120 s, maximum 600 s; the process is \
             killed when it times out. Use it for builds, tests, git and other CLI tools; \
             prefer read_file / list_dir / grep / edit_file for reading and editing files. The \
             user may be asked to approve the command.",
            json!({
                "type": "object",
                "properties": {
                    "command": { "type": "string", "description": "Shell command to run." },
                    "timeout_secs": { "type": "integer", "minimum": 1, "maximum": 600, "description": "Timeout in seconds (default 120, max 600)." }
                },
                "required": ["command"],
                "additionalProperties": false
            }),
        ),
    ]
}
