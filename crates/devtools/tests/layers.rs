//! The workspace is layered: domain crates never see GPUI, and screens only see the UI kit,
//! the hub and plain data. This test reads `cargo metadata` and fails on any other edge.

use std::collections::{BTreeMap, BTreeSet};
use std::process::Command;

/// GPUI-free crates. They may depend on each other and on third-party crates, never on UI.
const DOMAIN: &[&str] = &[
    "base", "model", "store", "llm", "mcp", "tools", "vcs", "catalog", "agent", "issues", "pipeline",
    "engine", "cli",
];

/// UI crates and the workspace crates each one may depend on.
const UI: &[(&str, &[&str])] = &[
    ("ui", &[]),
    ("ui_rich", &["ui", "base"]),
    ("hub", &["base", "model", "engine"]),
    ("shell", VIEW),
    ("palette", VIEW),
    ("screen_home", VIEW),
    ("screen_chat", VIEW),
    ("screen_inbox", VIEW),
    ("screen_issues", VIEW),
    ("screen_agents", VIEW),
    ("screen_editor", VIEW),
    ("devtools", &["ui", "ui_rich", "hub", "base", "model"]),
];

/// What a screen may use: the kit, the bridge and plain data. No other screen, no domain logic.
const VIEW: &[&str] = &["ui", "ui_rich", "hub", "model", "base"];

/// The binary that wires everything together.
const APP: &str = "app";

fn workspace_deps() -> BTreeMap<String, BTreeSet<String>> {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let out = Command::new(cargo)
        .args(["metadata", "--format-version", "1", "--no-deps"])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("run cargo metadata");
    assert!(out.status.success(), "cargo metadata failed: {}", String::from_utf8_lossy(&out.stderr));
    let meta: serde_json::Value = serde_json::from_slice(&out.stdout).expect("metadata is JSON");
    meta["packages"]
        .as_array()
        .expect("packages")
        .iter()
        .map(|p| {
            let name = p["name"].as_str().expect("name").to_string();
            let deps = p["dependencies"]
                .as_array()
                .expect("dependencies")
                .iter()
                .map(|d| d["name"].as_str().expect("dep name").to_string())
                .collect();
            (name, deps)
        })
        .collect()
}

#[test]
fn every_crate_belongs_to_a_layer() {
    let known: BTreeSet<&str> =
        DOMAIN.iter().copied().chain(UI.iter().map(|(n, _)| *n)).chain([APP]).collect();
    let unknown: Vec<String> =
        workspace_deps().into_keys().filter(|n| !known.contains(n.as_str())).collect();
    assert!(unknown.is_empty(), "add these crates to a layer in tests/layers.rs: {unknown:?}");
}

#[test]
fn domain_crates_do_not_depend_on_gpui_or_ui() {
    let deps = workspace_deps();
    let ui: BTreeSet<&str> = UI.iter().map(|(n, _)| *n).chain(["gpui", APP]).collect();
    let mut bad = Vec::new();
    for name in DOMAIN {
        for dep in deps.get(*name).into_iter().flatten() {
            if ui.contains(dep.as_str()) {
                bad.push(format!("{name} -> {dep}"));
            }
        }
    }
    assert!(bad.is_empty(), "domain crates must stay GPUI-free: {bad:?}");
}

#[test]
fn ui_crates_only_use_their_allowed_workspace_crates() {
    let deps = workspace_deps();
    let members: BTreeSet<&String> = deps.keys().collect();
    let mut bad = Vec::new();
    for (name, allowed) in UI {
        for dep in deps.get(*name).into_iter().flatten() {
            if members.contains(dep) && !allowed.contains(&dep.as_str()) {
                bad.push(format!("{name} -> {dep}"));
            }
        }
    }
    assert!(bad.is_empty(), "UI crates reach past their layer: {bad:?}");
}
