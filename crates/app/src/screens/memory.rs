//! Memory: not part of this release. The screen says so; it shows no data.

use super::prelude::*;

pub struct MemoryScreen;

impl MemoryScreen {
    pub fn new(_window: &mut Window, _cx: &mut Context<Self>) -> Self {
        MemoryScreen
    }
}

impl Render for MemoryScreen {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        screen()
            .child(PageHeader::new("Memory").meta(Badge::new("Not in this release")))
            .child(
                body().child(
                    EmptyState::not_available("Memory (notes agents keep between runs)")
                        .id("memory-na"),
                ),
            )
    }
}
