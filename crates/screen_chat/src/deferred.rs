//! Deferred MCP tool schemas (like Claude Code's tool search).
//!
//! Big MCP servers (grafana alone has ~80 tools) would add tens of thousands of input
//! tokens to every request. When the session has more than [`EAGER_LIMIT`] MCP tools, the
//! model only sees their names in the system prompt plus the [`TOOL_SEARCH`] tool; schemas
//! it loads with that tool are offered from the next step on, for the rest of the session.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Value, json};

use crate::extensions::McpTarget;

pub(crate) const TOOL_SEARCH: &str = "tool_search";

/// Up to this many MCP tools are offered with full schemas up-front.
pub(crate) const EAGER_LIMIT: usize = 15;

const DEFAULT_RESULTS: usize = 5;
const MAX_RESULTS: usize = 20;

pub(crate) fn spec() -> llm::ToolSpec {
    llm::ToolSpec {
        name: TOOL_SEARCH.into(),
        description: "Load the full definitions of deferred MCP tools (listed by name in the \
            system prompt) so you can call them. Query forms: `select:<name>[,<name>...]` loads \
            exact tools by name; otherwise keywords are matched against tool names and \
            descriptions (prefix a keyword with `+` to require it in the name). Loaded tools \
            become callable from your next step and stay loaded for this session."
            .into(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "`select:name1,name2` or keywords, e.g. `prometheus query`"
                },
                "max_results": {
                    "type": "integer",
                    "description": "Maximum tools to load for a keyword query (default 5, max 20)"
                }
            },
            "required": ["query"]
        }),
    }
}

/// System-prompt section listing deferred tools compactly, grouped by server.
pub(crate) fn prompt_section(
    index: &BTreeMap<String, McpTarget>,
    loaded: &BTreeSet<String>,
) -> String {
    let mut by_server: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for (id, (server, tool)) in index {
        if !loaded.contains(id) {
            by_server.entry(server).or_default().push(tool);
        }
    }
    if by_server.is_empty() {
        return String::new();
    }
    let mut s = String::from(
        "\n\n# Deferred MCP tools\n\nThese MCP tools exist but their definitions are not loaded. \
         To use one, first call `tool_search` (e.g. `select:mcp__server__tool`), then call it. \
         Tool id = `mcp__<server>__<tool>`.\n",
    );
    for (server, tools) in by_server {
        s.push_str(&format!("\n- {server}: {}", tools.join(", ")));
    }
    s
}

/// Find specs for a query. `select:` returns exact matches (full id or bare tool name when
/// unambiguous) in the given order; keywords are scored over name and description.
pub(crate) fn search<'a>(
    specs: &'a [llm::ToolSpec],
    query: &str,
    max_results: usize,
) -> Vec<&'a llm::ToolSpec> {
    let query = query.trim();
    if let Some(names) = query.strip_prefix("select:") {
        let mut out: Vec<&llm::ToolSpec> = Vec::new();
        for name in names.split(',').map(str::trim).filter(|n| !n.is_empty()) {
            let exact = specs.iter().find(|s| s.name == name);
            let found = exact.or_else(|| {
                let suffix = format!("__{name}");
                let mut it = specs.iter().filter(|s| s.name.ends_with(&suffix));
                match (it.next(), it.next()) {
                    (Some(one), None) => Some(one),
                    _ => None,
                }
            });
            if let Some(spec) = found
                && !out.iter().any(|s| s.name == spec.name)
            {
                out.push(spec);
            }
        }
        return out;
    }

    let terms: Vec<(String, bool)> = query
        .split_whitespace()
        .map(|t| match t.strip_prefix('+') {
            Some(req) => (req.to_lowercase(), true),
            None => (t.to_lowercase(), false),
        })
        .filter(|(t, _)| !t.is_empty())
        .collect();
    if terms.is_empty() {
        return Vec::new();
    }
    let mut scored: Vec<(usize, &llm::ToolSpec)> = specs
        .iter()
        .filter_map(|spec| {
            let name = spec.name.to_lowercase();
            let desc = spec.description.to_lowercase();
            let mut score = 0;
            for (term, required) in &terms {
                let in_name = name.contains(term.as_str());
                if *required && !in_name {
                    return None;
                }
                if in_name {
                    score += 3;
                } else if desc.contains(term.as_str()) {
                    score += 1;
                }
            }
            (score > 0).then_some((score, spec))
        })
        .collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.name.cmp(&b.1.name)));
    scored
        .into_iter()
        .take(max_results.clamp(1, MAX_RESULTS))
        .map(|(_, s)| s)
        .collect()
}

/// `(query, max_results)` from the tool input (string input is accepted as the query).
pub(crate) fn parse_input(input: &Value) -> Result<(String, usize), String> {
    let input = match input {
        Value::String(s) => serde_json::from_str(s).unwrap_or_else(|_| json!({ "query": s })),
        other => other.clone(),
    };
    let query = input
        .get("query")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|q| !q.is_empty())
        .ok_or("`query` is required, e.g. `select:mcp__server__tool` or keywords.")?;
    let max = input
        .get("max_results")
        .and_then(Value::as_u64)
        .map_or(DEFAULT_RESULTS, |n| n as usize);
    Ok((query.to_string(), max))
}

/// Tool-result text for the model: the loaded definitions.
pub(crate) fn render(matches: &[&llm::ToolSpec]) -> String {
    if matches.is_empty() {
        return "No matching tools. Check the names listed under \"Deferred MCP tools\".".into();
    }
    let mut s = format!(
        "Loaded {} tool(s); they are callable from your next step:\n",
        matches.len()
    );
    for spec in matches {
        s.push_str(&format!(
            "\n## {}\n{}\nInput schema: {}\n",
            spec.name, spec.description, spec.input_schema
        ));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(name: &str, desc: &str) -> llm::ToolSpec {
        llm::ToolSpec {
            name: name.into(),
            description: desc.into(),
            input_schema: json!({"type": "object"}),
        }
    }

    fn specs() -> Vec<llm::ToolSpec> {
        vec![
            spec("mcp__grafana__query_prometheus", "Run a PromQL query"),
            spec(
                "mcp__grafana__query_loki_logs",
                "Run a LogQL query against Loki",
            ),
            spec("mcp__grafana__list_datasources", "List datasources"),
            spec("mcp__9router__ask", "Ask one external model"),
            spec("mcp__other__ask", "Ask something else"),
        ]
    }

    fn names(v: &[&llm::ToolSpec]) -> Vec<String> {
        v.iter().map(|s| s.name.clone()).collect()
    }

    #[test]
    fn select_exact_and_bare_names() {
        let s = specs();
        let got = search(&s, "select:mcp__9router__ask, list_datasources, nope", 5);
        assert_eq!(
            names(&got),
            ["mcp__9router__ask", "mcp__grafana__list_datasources"]
        );
        // Bare `ask` is ambiguous (two servers) → not loaded.
        assert!(search(&s, "select:ask", 5).is_empty());
    }

    #[test]
    fn keyword_scoring_and_required_terms() {
        let s = specs();
        let got = search(&s, "query loki", 5);
        assert_eq!(got[0].name, "mcp__grafana__query_loki_logs");
        assert_eq!(
            names(&search(&s, "+prometheus", 5)),
            ["mcp__grafana__query_prometheus"]
        );
        assert!(search(&s, "+nothing query", 5).is_empty());
        assert_eq!(search(&s, "query", 1).len(), 1);
    }

    #[test]
    fn prompt_lists_only_unloaded_by_server() {
        let mut index = BTreeMap::new();
        index.insert("mcp__g__a".to_string(), ("g".to_string(), "a".to_string()));
        index.insert("mcp__g__b".to_string(), ("g".to_string(), "b".to_string()));
        let loaded: BTreeSet<String> = ["mcp__g__a".to_string()].into();
        let p = prompt_section(&index, &loaded);
        assert!(p.contains("- g: b"));
        assert!(!p.contains("g: a"));
        let all: BTreeSet<String> = index.keys().cloned().collect();
        assert!(prompt_section(&index, &all).is_empty());
    }

    #[test]
    fn input_parsing() {
        assert_eq!(
            parse_input(&json!({"query": " x "})).unwrap(),
            ("x".into(), 5)
        );
        assert_eq!(
            parse_input(&json!({"query": "x", "max_results": 2})).unwrap(),
            ("x".into(), 2)
        );
        assert_eq!(parse_input(&json!("select:a")).unwrap().0, "select:a");
        assert!(parse_input(&json!({})).is_err());
    }
}
