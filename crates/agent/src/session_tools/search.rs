//! `tool_search`: MCP tool schemas loaded on demand.
//!
//! Big MCP servers would add tens of thousands of tokens to every request. With more than
//! [`EAGER_LIMIT`] MCP tools, the model sees only their names in the system prompt; schemas it
//! loads with `tool_search` are offered from its next step on.

use std::collections::BTreeMap;

use mcp::McpTool;
use model::ToolSpec;
use serde_json::{Value, json};

use super::names::TOOL_SEARCH;
use super::object;

/// Up to this many MCP tools are offered with their schemas from the start.
pub(crate) const EAGER_LIMIT: usize = 15;

const DEFAULT_RESULTS: usize = 5;
const MAX_RESULTS: usize = 20;

pub(crate) fn spec() -> ToolSpec {
    ToolSpec {
        name: TOOL_SEARCH.into(),
        description: "Load the full definitions of MCP tools that the system prompt lists by name, so you can \
                      call them. Query forms: `select:<name>[,<name>...]` loads exact tools by name; otherwise \
                      keywords are matched against tool names and descriptions (prefix a keyword with `+` to \
                      require it in the name). Loaded tools can be called from your next step on."
            .into(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "query": { "type": "string", "description": "`select:name1,name2` or keywords, e.g. `prometheus query`" },
                "max_results": { "type": "integer", "description": "Most tools a keyword query loads (default 5, at most 20)" }
            },
            "required": ["query"]
        }),
    }
}

/// `(query, max_results)` from the input; a bare string is taken as the query.
pub(crate) fn parse(input: &Value) -> Result<(String, usize), String> {
    let bare = match input {
        Value::String(text) if serde_json::from_str::<Value>(text).map_or(true, |v| !v.is_object()) => Some(text),
        _ => None,
    };
    if let Some(query) = bare.map(|q| q.trim()).filter(|q| !q.is_empty()) {
        return Ok((query.to_string(), DEFAULT_RESULTS));
    }
    let map = object(TOOL_SEARCH, input)?;
    let query = map
        .get("query")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|q| !q.is_empty())
        .ok_or("tool_search: `query` is required, e.g. `select:mcp__server__tool` or keywords.")?;
    let max = map.get("max_results").and_then(Value::as_u64).map_or(DEFAULT_RESULTS, |n| n as usize);
    Ok((query.to_string(), max))
}

/// The tools a query finds. `select:` returns exact matches (a full id, or a bare tool name
/// when only one server has it) in the order asked; keywords are scored over name and
/// description, best first.
pub(crate) fn search<'a>(tools: &'a [McpTool], query: &str, max_results: usize) -> Vec<&'a McpTool> {
    let query = query.trim();
    if let Some(names) = query.strip_prefix("select:") {
        let mut out: Vec<&McpTool> = Vec::new();
        for name in names.split(',').map(str::trim).filter(|n| !n.is_empty()) {
            let exact = tools.iter().find(|t| t.id == name);
            let found = exact.or_else(|| {
                let suffix = format!("__{name}");
                let mut matching = tools.iter().filter(|t| t.id.ends_with(&suffix));
                match (matching.next(), matching.next()) {
                    (Some(one), None) => Some(one),
                    _ => None,
                }
            });
            if let Some(tool) = found
                && !out.iter().any(|t| t.id == tool.id)
            {
                out.push(tool);
            }
        }
        return out;
    }
    let terms: Vec<(String, bool)> = query
        .split_whitespace()
        .map(|term| match term.strip_prefix('+') {
            Some(required) => (required.to_lowercase(), true),
            None => (term.to_lowercase(), false),
        })
        .filter(|(term, _)| !term.is_empty())
        .collect();
    if terms.is_empty() {
        return Vec::new();
    }
    let mut scored: Vec<(usize, &McpTool)> = tools
        .iter()
        .filter_map(|tool| {
            let name = tool.id.to_lowercase();
            let description = tool.spec.description.to_lowercase();
            let mut score = 0;
            for (term, required) in &terms {
                let in_name = name.contains(term.as_str());
                if *required && !in_name {
                    return None;
                }
                if in_name {
                    score += 3;
                } else if description.contains(term.as_str()) {
                    score += 1;
                }
            }
            (score > 0).then_some((score, tool))
        })
        .collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.id.cmp(&b.1.id)));
    scored.into_iter().take(max_results.clamp(1, MAX_RESULTS)).map(|(_, tool)| tool).collect()
}

/// What the model reads back: the definitions it loaded.
pub(crate) fn render(found: &[&McpTool]) -> String {
    if found.is_empty() {
        return "No matching tools. Check the names listed under \"More MCP tools\" in the system prompt.".into();
    }
    let mut out = format!("Loaded {}; you can call them from your next step:\n", base::fmt::plural(found.len() as u64, "tool", "tools"));
    for tool in found {
        out.push_str(&format!("\n## {}\n{}\nInput schema: {}\n", tool.id, tool.spec.description, tool.spec.input_schema));
    }
    out
}

/// The "More MCP tools" section of the system prompt: every deferred tool by name, grouped by
/// server. It does not change when tools are loaded, so the prompt stays cached.
pub(crate) fn prompt_section(tools: &[McpTool]) -> String {
    let mut by_server: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for tool in tools {
        by_server.entry(tool.server.as_str()).or_default().push(tool.name.as_str());
    }
    let mut out = String::from(
        "These MCP tools exist, but their definitions are not loaded. To use one, first call `tool_search` \
         (for example `select:mcp__<server>__<tool>`), then call it by its id `mcp__<server>__<tool>`.\n",
    );
    for (server, names) in by_server {
        out.push_str(&format!("\n- {server}: {}", names.join(", ")));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tool(server: &str, name: &str, description: &str) -> McpTool {
        let id = mcp::tool_id(server, name);
        McpTool {
            id: id.clone(),
            server: server.into(),
            name: name.into(),
            spec: ToolSpec { name: id, description: description.into(), input_schema: json!({"type": "object"}) },
        }
    }

    fn tools() -> Vec<McpTool> {
        vec![
            tool("grafana", "query_prometheus", "Run a PromQL query"),
            tool("grafana", "query_loki_logs", "Run a LogQL query against Loki"),
            tool("grafana", "list_datasources", "List datasources"),
            tool("9router", "ask", "Ask one external model"),
            tool("other", "ask", "Ask something else"),
        ]
    }

    fn ids(found: &[&McpTool]) -> Vec<String> {
        found.iter().map(|t| t.id.clone()).collect()
    }

    #[test]
    fn select_takes_exact_and_unambiguous_bare_names() {
        let all = tools();
        let found = search(&all, "select:mcp__9router__ask, list_datasources, nope", 5);
        assert_eq!(ids(&found), ["mcp__9router__ask", "mcp__grafana__list_datasources"]);
        assert!(search(&all, "select:ask", 5).is_empty());
    }

    #[test]
    fn keywords_are_scored_and_plus_requires_the_name() {
        let all = tools();
        assert_eq!(search(&all, "query loki", 5)[0].id, "mcp__grafana__query_loki_logs");
        assert_eq!(ids(&search(&all, "+prometheus", 5)), ["mcp__grafana__query_prometheus"]);
        assert!(search(&all, "+nothing query", 5).is_empty());
        assert_eq!(search(&all, "query", 1).len(), 1);
        assert!(search(&all, "   ", 5).is_empty());
    }

    #[test]
    fn input_and_output_text() {
        assert_eq!(parse(&json!({"query": " x "})).expect("ok"), ("x".into(), 5));
        assert_eq!(parse(&json!({"query": "x", "max_results": 2})).expect("ok"), ("x".into(), 2));
        assert_eq!(parse(&json!("select:a")).expect("ok").0, "select:a");
        assert_eq!(parse(&json!(r#"{"query":"y"}"#)).expect("ok").0, "y");
        assert!(parse(&json!({})).is_err());
        let all = tools();
        let text = render(&search(&all, "select:query_prometheus", 5));
        assert!(text.starts_with("Loaded 1 tool;"), "{text}");
        assert!(text.contains("## mcp__grafana__query_prometheus\nRun a PromQL query\nInput schema: {\"type\":\"object\"}"));
        assert!(render(&[]).starts_with("No matching tools"));
        let section = prompt_section(&all);
        assert!(section.contains("- grafana: query_prometheus, query_loki_logs, list_datasources"));
        assert!(section.contains("- other: ask"));
    }
}
