//! Pipelines: the list of pipelines and the graph editor (Graph / Runs / Versions).
//! Design: design/screens/pipeline-editor.html. Starting point: the list.

use harness_core::Pipeline;

use super::prelude::*;

pub struct PipelinesScreen {
    pipelines: Option<Vec<Pipeline>>,
    selected: Option<Id>,
    load: Option<Task<()>>,
    _subs: Vec<Subscription>,
}

impl PipelinesScreen {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let app = AppState::global(cx);
        let sub = cx.subscribe_in(&app, window, |this, _, ev: &AppEvent, _, cx| match ev {
            AppEvent::StoreChanged { changes, .. } => {
                if changes.iter().any(|c| matches!(c, Change::Pipeline(_) | Change::Run(_) | Change::External)) {
                    this.reload(cx);
                }
            }
            AppEvent::Navigated(Route::Pipelines { pipeline }) => {
                this.selected = pipeline.clone();
                cx.notify();
            }
            _ => {}
        });
        let selected = match state::route(cx) {
            Route::Pipelines { pipeline } => pipeline,
            _ => None,
        };
        let mut this = PipelinesScreen { pipelines: None, selected, load: None, _subs: vec![sub] };
        this.reload(cx);
        this
    }

    fn reload(&mut self, cx: &mut Context<Self>) {
        self.load = Some(load(cx, |s| s.pipelines(), |this, v, _| this.pipelines = Some(v)));
    }
}

impl Render for PipelinesScreen {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let header = PageHeader::new("Pipelines").action(
            Button::new("pipelines-new", "New pipeline")
                .primary()
                .icon(AppIcon::Plus)
                .on_click(|_, _, cx| state::toast(ToastKind::Info, "Pick a template to start a pipeline.", cx)),
        );
        let content = match &self.pipelines {
            None => body().child(skeleton_rows("pipelines-loading", 3)),
            Some(v) if v.is_empty() => body().child(EmptyState::new(
                AppIcon::Workflow,
                "No pipelines yet",
                "A pipeline turns an issue into shipped work: source, agents, gates and tools wired as a graph.",
            )),
            Some(v) => body().gap(theme::S1).children(v.iter().map(|p| {
                let id = p.id.clone();
                let active = self.selected.as_ref() == Some(&p.id);
                let deployed = !p.graph.nodes.is_empty();
                div()
                    .id(SharedString::from(format!("pipeline-row-{}", p.id)))
                    .flex()
                    .items_center()
                    .gap(theme::S3)
                    .h(theme::ROW_H)
                    .px(theme::S3)
                    .rounded(theme::RADIUS_MD)
                    .cursor_pointer()
                    .when(active, |d| d.bg(theme::SURFACE_HOVER))
                    .when(!active, |d| d.hover(|s| s.bg(theme::SURFACE_RAISED)))
                    .child(if deployed {
                        dot_el(theme::rgb_u32(p.color), px(8.))
                    } else {
                        ring_el(theme::TEXT_MUTED, px(8.))
                    })
                    .child(div().flex_1().min_w_0().text_body_strong().truncate().child(p.name.clone()))
                    .when(deployed, |d| d.child(Badge::new(format!("v{}", p.version)).mono()))
                    .when(p.draft.is_some(), |d| d.child(Badge::new(format!("Draft v{}", if deployed { p.version + 1 } else { p.version })).accent()))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.selected = Some(id.clone());
                        state::replace_route(Route::Pipelines { pipeline: Some(id.clone()) }, cx);
                        cx.notify();
                    }))
            })),
        };
        screen().child(header).child(content)
    }
}
