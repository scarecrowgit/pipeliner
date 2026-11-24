//! Which view the shell shows for which route. Sections without a screen yet fall back to the
//! shell's placeholder.

use gpui::{AppContext as _, Focusable as _};
use hub::Route;
use screen_chat::{ChatsView, NewChatView, SessionView};
use screen_editor::EditorView;
use screen_home::HomeView;
use screen_agents::AgentsView;
use screen_inbox::InboxView;
use screen_issues::IssuesView;
use shell::{Header, Keep, Mounted, ScreenKind, Screens};

/// Every screen the app has.
pub fn all() -> Screens {
    Screens::new()
        .add(ScreenKind::Chat, Keep::PerRoute(8), |route, window, cx| {
            let session = match route {
                Route::Chat { session } => session.clone(),
                // The shell only calls a factory for its own kind.
                _ => hub::model::SessionId::new(""),
            };
            let view = cx.new(|cx| SessionView::new(session, window, cx));
            let for_header = view.clone();
            Mounted::new(view).header(move |window, cx| {
                let title = for_header.read(cx).title(cx);
                let (badges, actions) = for_header
                    .update(cx, |view, cx| (view.header_badges(window, cx), view.header_actions(window, cx)));
                let header = Header::new().crumb_to("Chats", Route::Chats).crumb(title);
                let header = badges.into_iter().fold(header, Header::after);
                actions.into_iter().fold(header, Header::action)
            })
        })
        .add(ScreenKind::NewChat, Keep::One, |_, window, cx| {
            let view = cx.new(|cx| NewChatView::new(window, cx));
            let for_header = view.clone();
            Mounted::new(view).header(move |window, cx| {
                let title = for_header.read(cx).title(cx);
                let badges = for_header.read(cx).header_badges(cx);
                let actions = for_header.update(cx, |view, cx| view.header_actions(window, cx));
                let header = Header::new().crumb_to("Chats", Route::Chats).crumb(title);
                let header = badges.into_iter().fold(header, Header::after);
                actions.into_iter().fold(header, Header::action)
            })
        })
        .add(ScreenKind::Chats, Keep::One, |_, window, cx| {
            let view = cx.new(|cx| ChatsView::new(window, cx));
            let for_header = view.clone();
            Mounted::new(view).header(move |window, cx| {
                let title = for_header.read(cx).title(cx);
                let actions = for_header.update(cx, |view, cx| view.header_actions(window, cx));
                let header = Header::title(title);
                actions.into_iter().fold(header, Header::action)
            })
        })
        .add(ScreenKind::Inbox, Keep::One, |route, window, cx| {
            let focus = match route {
                Route::Inbox { item } => item.clone(),
                _ => None,
            };
            let view = cx.new(|cx| InboxView::new(focus, window, cx));
            let shown = view.clone();
            let for_header = view.clone();
            Mounted::new(view)
                .on_show(move |route, window, cx| {
                    if let Route::Inbox { item } = route {
                        shown.update(cx, |view, cx| view.show(item.clone(), window, cx));
                    }
                })
                .header(move |window, cx| {
                    let (title, trail) = {
                        let view = for_header.read(cx);
                        (view.title(cx), view.trail(cx))
                    };
                    let (badges, actions) = for_header
                        .update(cx, |view, cx| (view.header_badges(window, cx), view.header_actions(window, cx)));
                    let header = if trail.is_empty() && title.as_ref() == "Inbox" {
                        Header::title(title)
                    } else {
                        let header = Header::new().crumb_to("Inbox", Route::Inbox { item: None });
                        trail.into_iter().fold(header, Header::crumb).crumb(title)
                    };
                    let header = badges.into_iter().fold(header, Header::after);
                    actions.into_iter().fold(header, Header::action)
                })
        })
        .add(ScreenKind::Issues, Keep::One, |route, window, cx| {
            let issue = match route {
                Route::Issues { issue } => issue.clone(),
                _ => None,
            };
            let view = cx.new(|cx| IssuesView::new(issue, window, cx));
            let shown = view.clone();
            let for_header = view.clone();
            Mounted::new(view)
                .on_show(move |route, window, cx| {
                    if let Route::Issues { issue } = route {
                        shown.update(cx, |view, cx| view.show(issue.clone(), window, cx));
                    }
                })
                .header(move |window, cx| {
                    let (title, project) = {
                        let view = for_header.read(cx);
                        (view.title(cx), view.project_name(cx))
                    };
                    let (badges, actions) = for_header
                        .update(cx, |view, cx| (view.header_badges(window, cx), view.header_actions(window, cx)));
                    let header = match project {
                        Some(project) => Header::new().crumb(project).crumb(title),
                        None => Header::title(title),
                    };
                    let header = badges.into_iter().fold(header, Header::after);
                    actions.into_iter().fold(header, Header::action)
                })
        })
        .add(ScreenKind::Agents, Keep::One, |route, window, cx| {
            let selected = match route {
                Route::Agents { runnable } => runnable.clone(),
                _ => None,
            };
            let view = cx.new(|cx| AgentsView::new(selected, window, cx));
            let shown = view.clone();
            let for_header = view.clone();
            Mounted::new(view)
                .on_show(move |route, window, cx| {
                    if let Route::Agents { runnable } = route {
                        shown.update(cx, |view, cx| view.show(runnable.clone(), window, cx));
                    }
                })
                .header(move |window, cx| {
                    let title = for_header.read(cx).title(cx);
                    let (badges, actions) = for_header
                        .update(cx, |view, cx| (view.header_badges(window, cx), view.header_actions(window, cx)));
                    let header = badges.into_iter().fold(Header::title(title), Header::after);
                    actions.into_iter().fold(header, Header::action)
                })
        })
        .add(ScreenKind::Home, Keep::One, |_, window, cx| {
            let view = cx.new(|cx| HomeView::new(window, cx));
            let shown = view.clone();
            let for_header = view.clone();
            Mounted::new(view)
                // Back on Home, J/K and Enter work at once: the kept view takes focus again.
                .on_show(move |_, window, cx| window.focus(&shown.focus_handle(cx)))
                .header(move |window, cx| {
                    let title = for_header.read(cx).title(cx);
                    let (badges, actions) = for_header
                        .update(cx, |view, cx| (view.header_badges(window, cx), view.header_actions(window, cx)));
                    let header = badges.into_iter().fold(Header::title(title), Header::after);
                    actions.into_iter().fold(header, Header::action)
                })
        })
        .add(ScreenKind::Editor, Keep::One, |route, window, cx| {
            let (pipeline, node) = match route {
                Route::Editor { pipeline, node } => (pipeline.clone(), node.clone()),
                // The shell only calls a factory for its own kind.
                _ => (hub::model::PipelineId::new(""), None),
            };
            let view = cx.new(|cx| EditorView::new(pipeline, node, window, cx));
            let shown = view.clone();
            let for_header = view.clone();
            Mounted::new(view)
                .on_show(move |route, window, cx| {
                    if let Route::Editor { pipeline, node } = route {
                        shown.update(cx, |view, cx| view.show(pipeline.clone(), node.clone(), window, cx));
                    }
                })
                .header(move |window, cx| {
                    let title = for_header.read(cx).title(cx);
                    let (badges, actions) = for_header
                        .update(cx, |view, cx| (view.header_badges(window, cx), view.header_actions(window, cx)));
                    let header = Header::new()
                        .crumb_to("Pipelines", Route::Agents { runnable: None })
                        .crumb(title);
                    let header = badges.into_iter().fold(header, Header::after);
                    actions.into_iter().fold(header, Header::action)
                })
        })
}
