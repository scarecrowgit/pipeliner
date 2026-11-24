//! Long values (a 200-character workdir path, a long model id, long names and tool names)
//! stay inside the panel: they end in `…` or wrap.

use gpui::{Bounds, Pixels, TestAppContext, VisualTestContext, px};
use hub::model::{ModelRef, NodeKind, ToolGrants};

use super::{click, graph, leak, open, problem, select};
use crate::Selection;

fn bounds(cx: &mut VisualTestContext, selector: &str) -> Bounds<Pixels> {
    cx.run_until_parked();
    cx.debug_bounds(leak(selector)).unwrap_or_else(|| panic!("`{selector}` was not laid out"))
}

/// What `selector` draws lies inside the inspector's width.
fn assert_inside(cx: &mut VisualTestContext, selector: &str) {
    let panel = bounds(cx, "inspector");
    let b = bounds(cx, selector);
    assert!(
        b.left() >= panel.left() - px(0.5) && b.right() <= panel.right() + px(0.5),
        "{selector} {b:?} is outside the inspector {panel:?}"
    );
}

#[gpui::test]
fn long_values_stay_inside_the_panel(cx: &mut TestAppContext) {
    let long_name = "A very long name for a node that somebody typed without stopping to think about it".to_string();
    let mut g = graph();
    let path = format!("/home/me/{}", "deeply-nested-folder/".repeat(10));
    assert!(path.len() >= 200);
    g.workdirs[0].path = path.into();
    g.workdirs[0].name = long_name.clone();
    for node in &mut g.nodes {
        match (&mut node.kind, node.id.as_str()) {
            (NodeKind::Agent(agent), "fixer") => {
                node.name = long_name.clone();
                agent.model = Some(ModelRef::new("anthropic", "claude-some-model-with-an-unusually-long-identifier-20261124"));
                agent.files_out = vec![format!("out/{}", "very-long-file-name-".repeat(6))];
                agent.tools = Some(ToolGrants {
                    allow: vec!["mcp__a_server_with_a_long_name__a_tool_with_an_even_longer_name_for_testing".into()],
                    ..Default::default()
                });
            }
            (NodeKind::Subagent(_), _) => node.name = long_name.clone(),
            _ => {}
        }
    }
    let (t, cx) = open(cx, g, Selection::None);
    click(cx, "insp-wd-0-edit");
    for selector in ["insp-wd-0-head", "insp-wd-0-summary", "insp-wd-0-path"] {
        assert_inside(cx, selector);
    }

    let revision = t.draft.read_with(cx, |d, _| d.revision());
    let message = format!("Fixer writes to {} which no node reads", "a/very/long/path/".repeat(8));
    t.draft.update(cx, |d, cx| d.set_problems(revision, vec![problem("fixer", &message)], cx));
    select(&t, cx, Selection::Node("fixer".into()));
    for selector in ["insp-problems", "insp-files-out-0", "insp-tools-allow-0", "insp-name", "insp-runnable"] {
        assert_inside(cx, selector);
    }
}
