//! Policies: not part of this release. The screen says so; it shows no data.

use super::prelude::*;

pub struct PoliciesScreen;

impl PoliciesScreen {
    pub fn new(_window: &mut Window, _cx: &mut Context<Self>) -> Self {
        PoliciesScreen
    }
}

impl Render for PoliciesScreen {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        screen()
            .child(PageHeader::new("Policies").meta(Badge::new("Not in this release")))
            .child(
                body().child(
                    EmptyState::not_available("Policies (budgets, approvals and spawn limits)")
                        .id("policies-na"),
                ),
            )
    }
}
