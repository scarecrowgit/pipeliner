//! Edits and lookups on a graph that the inspector's fields need: renaming ids with the edges
//! that point at them, out-ports, upstream agents, workdir users, router rules and the field a
//! problem is about. Plain functions over `GraphDef`, so they are tested without a window.

use std::collections::{BTreeMap, HashSet, VecDeque};

use hub::model::{
    ActionNode, AgentNode, EdgeKind, GraphDef, NodeDef, NodeId, NodeKind, PORT_APPROVED, PORT_ELSE, PORT_OUT,
    RouterNode, RouterRule,
};

/// The node `id`, to change.
pub(crate) fn node_mut<'a>(graph: &'a mut GraphDef, id: &NodeId) -> Option<&'a mut NodeDef> {
    graph.nodes.iter_mut().find(|node| &node.id == id)
}

/// The agent settings of an agent or orchestrator node.
pub(crate) fn agent_of(node: &NodeDef) -> Option<&AgentNode> {
    match &node.kind {
        NodeKind::Agent(agent) | NodeKind::Orchestrator(agent) => Some(agent),
        _ => None,
    }
}

/// [`agent_of`], to change.
pub(crate) fn agent_mut(node: &mut NodeDef) -> Option<&mut AgentNode> {
    match &mut node.kind {
        NodeKind::Agent(agent) | NodeKind::Orchestrator(agent) => Some(agent),
        _ => None,
    }
}

/// Why `text` cannot become the id of node `old`, or `None` when it can. Only the shape and
/// uniqueness are checked.
pub(crate) fn node_id_error(graph: &GraphDef, old: &NodeId, text: &str) -> Option<&'static str> {
    if text.is_empty() {
        Some("An id is needed")
    } else if text.chars().any(char::is_whitespace) {
        Some("No spaces in an id")
    } else if text != old.as_str() && graph.nodes.iter().any(|node| node.id.as_str() == text) {
        Some("Another node already has this id")
    } else {
        None
    }
}

/// Gives node `old` the id `new` and re-points every edge and send-back that named it.
pub(crate) fn rename_node(graph: &mut GraphDef, old: &NodeId, new: &NodeId) {
    if old == new {
        return;
    }
    for node in &mut graph.nodes {
        if &node.id == old {
            node.id = new.clone();
        }
        if let NodeKind::Review(review) = &mut node.kind
            && review.send_back_to.as_ref() == Some(old)
        {
            review.send_back_to = Some(new.clone());
        }
    }
    for edge in &mut graph.edges {
        if &edge.from == old {
            edge.from = new.clone();
        }
        if &edge.to == old {
            edge.to = new.clone();
        }
    }
}

/// Removes node `id`, its edges and any send-back that pointed at it.
pub(crate) fn remove_node(graph: &mut GraphDef, id: &NodeId) {
    graph.nodes.retain(|node| &node.id != id);
    graph.edges.retain(|edge| &edge.from != id && &edge.to != id);
    for node in &mut graph.nodes {
        if let NodeKind::Review(review) = &mut node.kind
            && review.send_back_to.as_ref() == Some(id)
        {
            review.send_back_to = None;
        }
    }
}

/// `base`, or `base2`, `base3`… whichever is not taken by `taken`.
pub(crate) fn unique(base: &str, taken: impl Fn(&str) -> bool) -> String {
    if !taken(base) {
        return base.to_string();
    }
    (2..).map(|n| format!("{base}{n}")).find(|id| !taken(id)).unwrap_or_else(|| base.to_string())
}

/// The out-ports of `node`, in the order the canvas draws them.
pub(crate) fn out_ports(node: &NodeDef) -> Vec<String> {
    match &node.kind {
        NodeKind::Router(router) => {
            router.rules.iter().map(|rule| rule.id.clone()).chain([PORT_ELSE.to_string()]).collect()
        }
        NodeKind::Review(_) => vec![PORT_APPROVED.to_string(), "send_back".to_string()],
        _ => vec![PORT_OUT.to_string()],
    }
}

/// How a port is named to a person: a router rule by its label.
pub(crate) fn port_label(node: &NodeDef, port: &str) -> String {
    if let NodeKind::Router(router) = &node.kind {
        if port == PORT_ELSE {
            return "else".to_string();
        }
        if let Some(rule) = router.rules.iter().find(|rule| rule.id == port) {
            return format!("{} · {}", rule.label, rule.id);
        }
    }
    match port {
        "send_back" => "send back".to_string(),
        other => other.to_string(),
    }
}

/// Agent and orchestrator nodes a route leads from to `id`, nearest first.
pub(crate) fn upstream_agents(graph: &GraphDef, id: &NodeId) -> Vec<NodeId> {
    let mut seen: HashSet<&NodeId> = HashSet::from([id]);
    let mut queue = VecDeque::from([id]);
    let mut found = Vec::new();
    while let Some(at) = queue.pop_front() {
        for edge in graph.edges.iter().filter(|edge| &edge.to == at && edge.kind == EdgeKind::Route) {
            if seen.insert(&edge.from) {
                if graph.node(&edge.from).and_then(agent_of).is_some() {
                    found.push(edge.from.clone());
                }
                queue.push_back(&edge.from);
            }
        }
    }
    found
}

/// The workdir a node names, when its kind has one.
pub(crate) fn workdir_of(node: &NodeDef) -> Vec<&str> {
    match &node.kind {
        NodeKind::Agent(agent) | NodeKind::Orchestrator(agent) => vec![agent.workdir.as_str()],
        NodeKind::File(file) => vec![file.workdir.as_str()],
        NodeKind::Action(action) => action_workdir(action).into_iter().collect(),
        NodeKind::Router(router) => router.rules.iter().filter_map(|rule| rule.ask.as_ref()).map(|ask| ask.workdir.as_str()).collect(),
        _ => Vec::new(),
    }
}

/// Names of the nodes that work in workdir `id`.
pub(crate) fn workdir_users(graph: &GraphDef, id: &str) -> Vec<String> {
    graph.nodes.iter().filter(|node| workdir_of(node).contains(&id)).map(|node| node.name.clone()).collect()
}

/// Gives workdir `old` the id `new` and re-points the nodes that used it.
pub(crate) fn rename_workdir(graph: &mut GraphDef, old: &str, new: &str) {
    for workdir in &mut graph.workdirs {
        if workdir.id == old {
            workdir.id = new.to_string();
        }
    }
    let swap = |field: &mut String| {
        if field == old {
            *field = new.to_string();
        }
    };
    for node in &mut graph.nodes {
        match &mut node.kind {
            NodeKind::Agent(agent) | NodeKind::Orchestrator(agent) => swap(&mut agent.workdir),
            NodeKind::File(file) => swap(&mut file.workdir),
            NodeKind::Action(action) => {
                if let Some(workdir) = action_workdir_mut(action) {
                    swap(workdir);
                }
            }
            NodeKind::Router(router) => {
                for ask in router.rules.iter_mut().filter_map(|rule| rule.ask.as_mut()) {
                    swap(&mut ask.workdir);
                }
            }
            _ => {}
        }
    }
}

/// Why `text` cannot become the id of workdir `index`, or `None`.
pub(crate) fn workdir_id_error(graph: &GraphDef, index: usize, text: &str) -> Option<&'static str> {
    if text.is_empty() {
        Some("An id is needed")
    } else if text.chars().any(char::is_whitespace) {
        Some("No spaces in an id")
    } else if graph.workdirs.iter().enumerate().any(|(i, w)| i != index && w.id == text) {
        Some("Another workdir already has this id")
    } else {
        None
    }
}

/// `KEY=value` lines for the env field, sorted by key.
pub(crate) fn env_text(env: &BTreeMap<String, String>) -> String {
    env.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join("\n")
}

/// The env field's text as variables; a non-empty line without `=` is a shape error.
pub(crate) fn parse_env(text: &str) -> Result<BTreeMap<String, String>, &'static str> {
    let mut env = BTreeMap::new();
    for line in text.lines().map(str::trim).filter(|line| !line.is_empty()) {
        let (key, value) = line.split_once('=').ok_or("One KEY=value per line")?;
        let key = key.trim();
        if key.is_empty() {
            return Err("One KEY=value per line");
        }
        env.insert(key.to_string(), value.trim().to_string());
    }
    Ok(env)
}

/// The workdir of an action, when its kind has one.
pub(crate) fn action_workdir(action: &ActionNode) -> Option<&str> {
    match action {
        ActionNode::Command { workdir, .. }
        | ActionNode::Render { workdir, .. }
        | ActionNode::Merge { workdir }
        | ActionNode::PullRequest { workdir, .. } => Some(workdir),
        ActionNode::Report | ActionNode::UpdateIssue { .. } => None,
    }
}

/// [`action_workdir`], to change.
pub(crate) fn action_workdir_mut(action: &mut ActionNode) -> Option<&mut String> {
    match action {
        ActionNode::Command { workdir, .. }
        | ActionNode::Render { workdir, .. }
        | ActionNode::Merge { workdir }
        | ActionNode::PullRequest { workdir, .. } => Some(workdir),
        ActionNode::Report | ActionNode::UpdateIssue { .. } => None,
    }
}

/// The kinds of action, in the order the picker lists them.
pub(crate) const ACTION_KINDS: [&str; 6] = ["command", "render", "merge", "pull_request", "report", "update_issue"];

/// Index of `action`'s kind in [`ACTION_KINDS`].
pub(crate) fn action_kind(action: &ActionNode) -> usize {
    match action {
        ActionNode::Command { .. } => 0,
        ActionNode::Render { .. } => 1,
        ActionNode::Merge { .. } => 2,
        ActionNode::PullRequest { .. } => 3,
        ActionNode::Report => 4,
        ActionNode::UpdateIssue { .. } => 5,
    }
}

/// What the picker calls an action kind.
pub(crate) fn action_label(kind: usize) -> &'static str {
    ["Run a command", "Render", "Merge", "Pull request", "Report", "Update issue"].get(kind).copied().unwrap_or("Action")
}

/// One line on what an action kind does.
pub(crate) fn action_meaning(kind: usize) -> &'static str {
    [
        "Runs a shell command in a workdir. With tests on, its result is the Tests row of the review.",
        "Renders the HTML files the run changed to before and after images for the review.",
        "Merges the run's branch of a worktree into its base branch.",
        "Commits, pushes and opens a pull request, then posts it to the Inbox.",
        "Posts a summary of the run to the Inbox.",
        "Moves the issue to a status and optionally comments on it.",
    ]
    .get(kind)
    .copied()
    .unwrap_or("")
}

/// An action of kind `kind` (an index into [`ACTION_KINDS`]) that keeps `from`'s workdir.
pub(crate) fn with_action_kind(from: &ActionNode, kind: usize, fallback_workdir: &str) -> ActionNode {
    if action_kind(from) == kind {
        return from.clone();
    }
    let workdir = action_workdir(from).unwrap_or(fallback_workdir).to_string();
    match kind {
        0 => ActionNode::Command { workdir, command: String::new(), tests: false, parse: String::new(), timeout_secs: 0 },
        1 => ActionNode::Render { workdir, files: "**/*.html".to_string() },
        2 => ActionNode::Merge { workdir },
        3 => ActionNode::PullRequest { workdir, draft: false, push: true },
        4 => ActionNode::Report,
        _ => ActionNode::UpdateIssue { status: None, comment: String::new() },
    }
}

/// Adds a rule at the end of `router`, with a free port id. Returns its index.
pub(crate) fn add_rule(router: &mut RouterNode) -> usize {
    let taken = |id: &str| id == PORT_ELSE || router.rules.iter().any(|rule| rule.id == id);
    let id = (router.rules.len() + 1..).map(|n| format!("rule{n}")).find(|id| !taken(id)).unwrap_or_default();
    router.rules.push(RouterRule { id, label: "New rule".to_string(), when: String::new(), ask: None });
    router.rules.len() - 1
}

/// Removes rule `index` of router `node` and the edges leaving its port.
pub(crate) fn remove_rule(graph: &mut GraphDef, node: &NodeId, index: usize) {
    let Some(NodeKind::Router(router)) = node_mut(graph, node).map(|n| &mut n.kind) else { return };
    if index >= router.rules.len() {
        return;
    }
    let port = router.rules.remove(index).id;
    graph.edges.retain(|edge| !(&edge.from == node && edge.port == port));
}

/// Swaps rule `index` with its neighbour above (`up`) or below.
pub(crate) fn move_rule(router: &mut RouterNode, index: usize, up: bool) {
    let other = if up { index.checked_sub(1) } else { Some(index + 1) };
    if let Some(other) = other.filter(|o| *o < router.rules.len())
        && index < router.rules.len()
    {
        router.rules.swap(index, other);
    }
}

/// The field of a node's inspector a problem message is plainly about, and the label of the
/// router rule it names, if any.
pub(crate) fn problem_field(message: &str) -> (Option<&'static str>, Option<String>) {
    let rule = message
        .strip_prefix("Rule “")
        .and_then(|rest| rest.split_once('”'))
        .map(|(label, rest)| (label.to_string(), rest));
    if let Some((label, rest)) = rule {
        let field = if rest.starts_with(':') || rest.contains("condition") {
            "when"
        } else if rest.contains("empty question") {
            "ask"
        } else if rest.contains("unknown model") {
            "ask-model"
        } else if rest.contains("unknown workdir") {
            "ask-workdir"
        } else {
            "label"
        };
        return (Some(field), Some(label));
    }
    let lower = message.to_lowercase();
    let has = |words: &[&str]| words.iter().any(|w| lower.contains(w));
    let field = if has(&["unknown agent", "agent from your library"]) {
        Some("runnable")
    } else if has(&["no role"]) {
        Some("role")
    } else if has(&["unknown model"]) {
        Some("model")
    } else if has(&["variants"]) && !has(&["worktrees"]) {
        Some("variants")
    } else if has(&["workdir", "worktrees"]) {
        Some("workdir")
    } else if has(&["budget"]) {
        Some("budget")
    } else if has(&["steps"]) {
        Some("max-steps")
    } else if has(&["no command"]) {
        Some("command")
    } else if has(&["files to render"]) {
        Some("files")
    } else if has(&["no path"]) {
        Some("path")
    } else if has(&["has no id", "already has the id"]) {
        Some("id")
    } else if has(&["send back goes", "send-back"]) {
        Some("send-back")
    } else if has(&["sources are coming soon", "only one source"]) {
        Some("source")
    } else if has(&["status or a comment"]) {
        Some("status")
    } else if has(&["0 times per run"]) {
        Some("per-run")
    } else if has(&["may not spawn"]) {
        Some("spawn")
    } else if has(&["no rules", "no else route"]) {
        Some("rules")
    } else {
        None
    };
    (field, None)
}

#[cfg(test)]
mod tests {
    use hub::model::{EdgeDef, Limits, PipelineId, Pos, ReviewNode, WorkdirDef};

    use super::*;

    fn node(id: &str, kind: NodeKind) -> NodeDef {
        NodeDef { id: id.into(), name: id.to_uppercase(), pos: Pos::default(), kind }
    }

    fn edge(from: &str, port: &str, to: &str, kind: EdgeKind) -> EdgeDef {
        EdgeDef { from: from.into(), port: port.into(), to: to.into(), kind }
    }

    fn graph() -> GraphDef {
        let agent = AgentNode { workdir: "code".into(), variants: 1, ..Default::default() };
        GraphDef {
            schema: 1,
            id: PipelineId::new("p"),
            name: "P".into(),
            description: String::new(),
            limits: Limits::default(),
            workdirs: vec![WorkdirDef { id: "code".into(), name: "Code".into(), ..Default::default() }],
            nodes: vec![
                node("a", NodeKind::Agent(agent.clone())),
                node("b", NodeKind::Agent(agent)),
                node("r", NodeKind::Review(ReviewNode { send_back_to: Some("a".into()), skip_if_empty: false })),
            ],
            edges: vec![edge("a", PORT_OUT, "b", EdgeKind::Route), edge("b", PORT_OUT, "r", EdgeKind::Route), edge("r", "send_back", "a", EdgeKind::Back)],
        }
    }

    #[test]
    fn a_rename_re_points_edges_and_send_back() {
        let mut g = graph();
        assert_eq!(node_id_error(&g, &"a".into(), "b"), Some("Another node already has this id"));
        assert_eq!(node_id_error(&g, &"a".into(), "a"), None);
        assert_eq!(node_id_error(&g, &"a".into(), ""), Some("An id is needed"));
        rename_node(&mut g, &"a".into(), &"fixer".into());
        assert_eq!(g.edges[0].from.as_str(), "fixer");
        assert_eq!(g.edges[2].to.as_str(), "fixer");
        let NodeKind::Review(review) = &g.nodes[2].kind else { panic!() };
        assert_eq!(review.send_back_to.as_ref().map(NodeId::as_str), Some("fixer"));
    }

    #[test]
    fn upstream_agents_follow_routes_backwards() {
        let g = graph();
        let ids: Vec<String> = upstream_agents(&g, &"r".into()).into_iter().map(|id| id.0).collect();
        assert_eq!(ids, ["b", "a"]);
    }

    #[test]
    fn workdir_users_and_rename() {
        let mut g = graph();
        assert_eq!(workdir_users(&g, "code"), ["A", "B"]);
        rename_workdir(&mut g, "code", "repo");
        assert_eq!(workdir_users(&g, "repo").len(), 2);
        assert_eq!(workdir_id_error(&g, 0, "has space"), Some("No spaces in an id"));
    }

    #[test]
    fn rules_add_remove_and_move_with_their_edges() {
        let mut g = graph();
        g.nodes.push(node("t", NodeKind::Router(RouterNode::default())));
        let NodeKind::Router(router) = &mut g.nodes[3].kind else { panic!() };
        add_rule(router);
        add_rule(router);
        router.rules[0].label = "First".into();
        move_rule(router, 0, false);
        assert_eq!(router.rules[1].label, "First");
        assert_eq!(router.rules[0].id, "rule2");
        g.edges.push(edge("t", "rule2", "a", EdgeKind::Route));
        remove_rule(&mut g, &"t".into(), 0);
        assert!(!g.edges.iter().any(|e| e.port == "rule2"));
    }

    #[test]
    fn an_action_kind_change_keeps_the_workdir() {
        let pr = ActionNode::PullRequest { workdir: "code".into(), draft: true, push: true };
        let command = with_action_kind(&pr, 0, "");
        assert_eq!(action_workdir(&command), Some("code"));
        let report = with_action_kind(&command, 4, "");
        assert_eq!(action_workdir(&with_action_kind(&report, 2, "code")), Some("code"));
    }

    #[test]
    fn env_lines_parse_and_print() {
        let env = parse_env("B=2\n A = 1 \n\n").unwrap();
        assert_eq!(env_text(&env), "A=1\nB=2");
        assert!(parse_env("nothing").is_err());
    }

    #[test]
    fn problems_name_their_field() {
        assert_eq!(problem_field("Rule “Bug”: expected a value after ==").0, Some("when"));
        assert_eq!(problem_field("Rule “Bug”: expected").1.as_deref(), Some("Bug"));
        assert_eq!(problem_field("Fixer uses an unknown workdir “x”").0, Some("workdir"));
        assert_eq!(problem_field("Variants of Fixer need a workdir with worktrees").0, Some("workdir"));
        assert_eq!(problem_field("Fixer has 0 variants").0, Some("variants"));
        assert_eq!(problem_field("Nothing routes to Fixer, so it never runs").0, None);
    }
}
