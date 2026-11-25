//! Agents: saved agents (role, model, tools, skills) and how they perform.
//! Design: design/screens/agents.html. Starting point: the list.

use harness_core::AgentDef;

use super::prelude::*;

pub struct AgentsScreen {
    agents: Option<Vec<AgentDef>>,
    selected: Option<Id>,
    load: Option<Task<()>>,
    _subs: Vec<Subscription>,
}

impl AgentsScreen {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let app = AppState::global(cx);
        let sub = cx.subscribe_in(&app, window, |this, _, ev: &AppEvent, _, cx| match ev {
            AppEvent::StoreChanged { changes, .. } => {
                if changes.iter().any(|c| matches!(c, Change::Agent(_) | Change::External)) {
                    this.reload(cx);
                }
            }
            AppEvent::Navigated(Route::Agents { agent }) => {
                this.selected = agent.clone();
                cx.notify();
            }
            _ => {}
        });
        let selected = match state::route(cx) {
            Route::Agents { agent } => agent,
            _ => None,
        };
        let mut this = AgentsScreen { agents: None, selected, load: None, _subs: vec![sub] };
        this.reload(cx);
        this
    }

    fn reload(&mut self, cx: &mut Context<Self>) {
        self.load = Some(load(cx, |s| s.agents(), |this, v, _| this.agents = Some(v)));
    }
}

impl Render for AgentsScreen {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let header = PageHeader::new("Agents");
        let content = match &self.agents {
            None => body().child(skeleton_rows("agents-loading", 3)),
            Some(v) if v.is_empty() => body().child(EmptyState::new(
                AppIcon::Bot,
                "No saved agents",
                "Save an agent to reuse its role, model, tools and skills across pipelines and chats.",
            )),
            Some(v) => body().gap(theme::S1).children(v.iter().map(|a| {
                let id = a.id.clone();
                let active = self.selected.as_ref() == Some(&a.id);
                div()
                    .id(SharedString::from(format!("agent-row-{}", a.id)))
                    .flex()
                    .items_center()
                    .gap(theme::S3)
                    .h(px(48.))
                    .px(theme::S3)
                    .rounded(theme::RADIUS_MD)
                    .cursor_pointer()
                    .when(active, |d| d.bg(theme::SURFACE_HOVER))
                    .when(!active, |d| d.hover(|s| s.bg(theme::SURFACE_RAISED)))
                    .child(VariantChip::initial(&a.name))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .child(div().text_body_strong().truncate().child(a.name.clone()))
                            .when(!a.description.is_empty(), |d| {
                                d.child(div().text_label().fg_muted().truncate().child(a.description.clone()))
                            }),
                    )
                    .child(Badge::new(Config::short_model(&a.model)))
                    .child(Badge::new(a.tools.label()))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.selected = Some(id.clone());
                        state::replace_route(Route::Agents { agent: Some(id.clone()) }, cx);
                        cx.notify();
                    }))
            })),
        };
        screen().child(header).child(content)
    }
}
