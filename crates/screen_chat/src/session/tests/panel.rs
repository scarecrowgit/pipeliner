//! The context panel follows the session: todo list, files, sub-agents, MCP servers, and the
//! header's toggle folds it away.

use gpui::{AppContext as _, TestAppContext};
use hub::Route;
use hub::api::{AgentEvent, Event, FileChange, TodoItem, TodoStatus};
use hub::model::SessionId;

use super::{FIX, calls, click, open, shown, text};

fn agent(event: AgentEvent) -> Event {
    Event::Agent { session: SessionId::new(FIX), event }
}

#[gpui::test]
fn sections_follow_the_session(cx: &mut TestAppContext) {
    let (t, cx) = open(cx, FIX);
    assert!(shown(cx, "chat-panel-todo-3") && !shown(cx, "chat-panel-todo-4"));
    assert!(shown(cx, "chat-panel-file-1"));
    assert!(shown(cx, "chat-panel-subagent-1"));
    assert!(shown(cx, "chat-panel-skill-0"));

    let item = |text: &str| TodoItem { text: text.into(), status: TodoStatus::Pending };
    t.api.emit(agent(AgentEvent::TodoUpdated { items: (0..5).map(|n| item(&format!("step {n}"))).collect() }));
    t.api.emit(agent(AgentEvent::FilesChanged {
        files: vec![FileChange { path: "src/refunds/webhook.ts".into(), added: 4, removed: 1 }],
    }));
    cx.run_until_parked();
    assert!(shown(cx, "chat-panel-todo-4"));
    // Debug bounds outlive the frame that drew them, so the new list is checked by its row:
    // the first file row now names the new file.
    click(cx, "chat-panel-file-0");
    assert_eq!(text(&t, cx), "@src/refunds/webhook.ts ");

    click(cx, "chat-panel-mcp-2-reconnect");
    assert_eq!(calls(&t.api, "reconnect_mcp"), [format!("{FIX} sentry")]);

    click(cx, "chat-panel-compact");
    assert_eq!(calls(&t.api, "compact"), [FIX]);

    click(cx, "chat-panel-subagent-0");
    let route = cx.update(|_, cx| hub::router(cx).read(cx).current().clone());
    assert_eq!(route, Route::Chat { session: SessionId::new("chat-1423-explore") });
}

#[gpui::test]
fn the_toggle_folds_the_panel_for_every_chat(cx: &mut TestAppContext) {
    let (t, cx) = open(cx, FIX);
    assert!(shown(cx, "chat-panel"));
    click(cx, "chat-panel-toggle");
    assert!(!t.view.read_with(cx, |v, _| v.is_panel_open()));
    let again = cx.update(|window, cx| cx.new(|cx| crate::SessionView::new(SessionId::new(FIX), window, cx)));
    assert!(!again.read_with(cx, |v, _| v.is_panel_open()), "the choice is remembered");
    click(cx, "chat-panel-toggle");
    assert!(t.view.read_with(cx, |v, _| v.is_panel_open()));
}
