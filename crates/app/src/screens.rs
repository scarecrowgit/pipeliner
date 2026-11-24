//! Which view the shell shows for which route. Sections without a screen yet fall back to the
//! shell's placeholder.

use gpui::AppContext as _;
use hub::Route;
use screen_chat::{ChatsView, NewChatView, SessionView};
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
}
