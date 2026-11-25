//! Chat: conversations with a model, an agent, an orchestrator or a pipeline.
//! Design: design/screens/chat.html. Starting point: the chat list and "New chat".

use harness_core::{Chat, ChatMode};

use super::prelude::*;

pub struct ChatScreen {
    chats: Option<Vec<Chat>>,
    selected: Option<Id>,
    load: Option<Task<()>>,
    _subs: Vec<Subscription>,
}

impl ChatScreen {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let app = AppState::global(cx);
        let sub = cx.subscribe_in(&app, window, |this, _, ev: &AppEvent, _, cx| match ev {
            AppEvent::StoreChanged { changes, .. } => {
                // Streaming tokens only touch messages; the list needs chats.
                if changes.iter().any(|c| matches!(c, Change::Chat(_) | Change::External)) {
                    this.reload(cx);
                }
            }
            AppEvent::Navigated(Route::Chat { chat: Some(id) }) => {
                this.selected = Some(id.clone());
                cx.notify();
            }
            _ => {}
        });
        let selected = match state::route(cx) {
            Route::Chat { chat } => chat,
            _ => None,
        };
        let mut this = ChatScreen { chats: None, selected, load: None, _subs: vec![sub] };
        this.reload(cx);
        this
    }

    fn reload(&mut self, cx: &mut Context<Self>) {
        self.load = Some(load(cx, |s| s.chats(), |this, v, _| this.chats = Some(v)));
    }

    fn new_chat(cx: &mut App) {
        let task = state::run_engine(cx, |e| e.new_chat(ChatMode::Direct));
        cx.spawn(async move |cx| {
            if let Some(chat) = task.await {
                let _ = cx.update(|cx| state::navigate(Route::Chat { chat: Some(chat.id) }, cx));
            }
        })
        .detach();
    }
}

impl Render for ChatScreen {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let header = PageHeader::new("Chat").action(
            Button::new("chat-new", "New chat").primary().icon(AppIcon::Plus).on_click(|_, _, cx| Self::new_chat(cx)),
        );
        let content = match &self.chats {
            None => body().child(skeleton_rows("chat-loading", 4)),
            Some(list) if list.is_empty() => body().child(
                EmptyState::new(
                    AppIcon::Chat,
                    "No chats yet",
                    "Talk to a model directly, hand a task to an agent, let an orchestrator plan it, or start a pipeline.",
                )
                .action(Button::new("chat-empty-new", "New chat").icon(AppIcon::Plus).on_click(|_, _, cx| Self::new_chat(cx))),
            ),
            Some(list) => body().gap(theme::S1).children(list.iter().map(|c| {
                let id = c.id.clone();
                let active = self.selected.as_ref() == Some(&c.id);
                div()
                    .id(SharedString::from(format!("chat-row-{}", c.id)))
                    .flex()
                    .items_center()
                    .gap(theme::S3)
                    .h(theme::ROW_H)
                    .px(theme::S3)
                    .rounded(theme::RADIUS_MD)
                    .cursor_pointer()
                    .when(active, |d| d.bg(theme::SURFACE_HOVER))
                    .when(!active, |d| d.hover(|s| s.bg(theme::SURFACE_RAISED)))
                    .child(icon(AppIcon::Chat).text_color(theme::TEXT_MUTED))
                    .child(div().flex_1().min_w_0().text_body_strong().truncate().child(c.title.clone()))
                    .child(Badge::new(c.mode.label()))
                    .child(div().w(px(56.)).text_right().text_label().fg_muted().child(ui::fmt_ago(c.updated_at)))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.selected = Some(id.clone());
                        state::replace_route(Route::Chat { chat: Some(id.clone()) }, cx);
                        cx.notify();
                    }))
            })),
        };
        screen().child(header).child(content)
    }
}
