//! How screens plug into the shell without the shell knowing them: the `app` binary registers
//! one [`Screen`] per [`ScreenKind`] in a [`Screens`] registry, and each mounted screen hands
//! back its view and what the header bar shows ([`Mounted`], [`Header`]).

use std::collections::HashMap;
use std::rc::Rc;

use gpui::{AnyElement, AnyView, App, IntoElement, SharedString, Window};
use hub::{Route, Section};

/// Which screen a route shows. Every [`Route`] variant has its own kind, so the chat list,
/// a chat session and the new-chat page can be three different screens of the Chats section.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ScreenKind {
    Home,
    Inbox,
    /// The list of every chat.
    Chats,
    /// One chat session.
    Chat,
    /// The new-chat page.
    NewChat,
    Issues,
    Agents,
    /// The pipeline editor.
    Editor,
}

impl ScreenKind {
    /// Every kind, in sidebar order.
    pub const ALL: [ScreenKind; 8] = [
        ScreenKind::Home,
        ScreenKind::Inbox,
        ScreenKind::Chats,
        ScreenKind::Chat,
        ScreenKind::NewChat,
        ScreenKind::Issues,
        ScreenKind::Agents,
        ScreenKind::Editor,
    ];

    /// The kind of screen that shows `route`.
    pub fn of(route: &Route) -> ScreenKind {
        match route {
            Route::Home => ScreenKind::Home,
            Route::Inbox { .. } => ScreenKind::Inbox,
            Route::Chats => ScreenKind::Chats,
            Route::Chat { .. } => ScreenKind::Chat,
            Route::NewChat => ScreenKind::NewChat,
            Route::Issues { .. } => ScreenKind::Issues,
            Route::Agents { .. } => ScreenKind::Agents,
            Route::Editor { .. } => ScreenKind::Editor,
        }
    }
}

/// The sidebar label of a section, as the mockups write it.
pub fn section_label(section: Section) -> &'static str {
    match section {
        Section::Home => "Home",
        Section::Inbox => "Inbox",
        Section::Chats => "Chats",
        Section::Issues => "Issues",
        Section::Agents => "Agents & pipelines",
    }
}

/// Which built views the shell keeps alive when the route moves on.
///
/// The caching rule: a view is built the first time its key is shown and reused every time
/// that key is shown again. With [`Keep::One`] the key is the kind, so one view serves every
/// route of the kind and follows parameter changes itself (through [`Mounted::on_show`] or by
/// observing the router). With [`Keep::PerRoute`] the key is the whole route (a chat session
/// view per session); the `n` most recently shown routes of that kind stay alive and older
/// ones are dropped, which also drops the models only they held.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Keep {
    /// One view for every route of the kind, kept for the life of the window.
    #[default]
    One,
    /// One view per distinct route; the `n` most recently shown are kept (at least one).
    PerRoute(usize),
}

/// Builds the header bar's content for this frame.
pub type HeaderFn = Rc<dyn Fn(&mut Window, &mut App) -> Header>;
/// Called with the route a kept view is shown for.
pub type ShowFn = Rc<dyn Fn(&Route, &mut Window, &mut App)>;

/// A built screen: its view, and how to fill the header bar above it.
#[derive(Clone)]
pub struct Mounted {
    pub(crate) body: AnyView,
    pub(crate) header: Option<HeaderFn>,
    pub(crate) on_show: Option<ShowFn>,
}

impl Mounted {
    /// A screen whose header shows only its section's label, until [`Self::header`] says more.
    pub fn new(body: impl Into<AnyView>) -> Self {
        Self { body: body.into(), header: None, on_show: None }
    }

    /// Fill the header bar from the screen's state. Called every frame the screen is shown,
    /// so it reads the screen's entities directly.
    pub fn header(mut self, f: impl Fn(&mut Window, &mut App) -> Header + 'static) -> Self {
        self.header = Some(Rc::new(f));
        self
    }

    /// Called whenever this view becomes the shown one, with the route it is shown for: the
    /// first time and on every return. A [`Keep::One`] screen learns new parameters here.
    pub fn on_show(mut self, f: impl Fn(&Route, &mut Window, &mut App) + 'static) -> Self {
        self.on_show = Some(Rc::new(f));
        self
    }

    /// The screen's view.
    pub fn body(&self) -> &AnyView {
        &self.body
    }
}

/// One link of the breadcrumb. All but the last are links (muted, going to `route`); the last
/// is the title.
#[derive(Clone, Debug)]
pub struct Crumb {
    pub label: SharedString,
    pub route: Option<Route>,
}

/// What the header bar of a screen shows: a breadcrumb, then state badges or a note right
/// after it, a spacer, and the actions at the right (ghost, secondary, primary, danger last).
#[derive(Default)]
pub struct Header {
    pub(crate) crumbs: Vec<Crumb>,
    pub(crate) after: Vec<AnyElement>,
    pub(crate) actions: Vec<AnyElement>,
    pub(crate) hidden: bool,
}

impl Header {
    /// An empty header; add crumbs and actions.
    pub fn new() -> Self {
        Self::default()
    }

    /// A header with one crumb: the screen's title.
    pub fn title(label: impl Into<SharedString>) -> Self {
        Self::new().crumb(label)
    }

    /// No header bar at all: the screen draws its own top.
    pub fn hidden() -> Self {
        Self { hidden: true, ..Self::default() }
    }

    /// A crumb that is not a link (usually the last one, the title).
    pub fn crumb(mut self, label: impl Into<SharedString>) -> Self {
        self.crumbs.push(Crumb { label: label.into(), route: None });
        self
    }

    /// A crumb that goes to `route` when clicked.
    pub fn crumb_to(mut self, label: impl Into<SharedString>, route: Route) -> Self {
        self.crumbs.push(Crumb { label: label.into(), route: Some(route) });
        self
    }

    /// Something right after the breadcrumb: a badge ("8 open"), a muted note, a picker.
    pub fn after(mut self, el: impl IntoElement) -> Self {
        self.after.push(el.into_any_element());
        self
    }

    /// An action at the right, in the order added.
    pub fn action(mut self, el: impl IntoElement) -> Self {
        self.actions.push(el.into_any_element());
        self
    }

    /// The crumbs, for tests and checks.
    pub fn crumbs(&self) -> &[Crumb] {
        &self.crumbs
    }
}

/// A screen the shell can show: builds a view for a route.
pub trait Screen: 'static {
    /// Which built views to keep alive (see [`Keep`]).
    fn keep(&self) -> Keep {
        Keep::One
    }

    /// Build the view for `route`. Runs on the UI thread when a key is first shown: create
    /// entities here, load nothing synchronously.
    fn mount(&self, route: &Route, window: &mut Window, cx: &mut App) -> Mounted;
}

/// A [`Screen`] from a closure.
struct ScreenFn<F> {
    keep: Keep,
    build: F,
}

impl<F> Screen for ScreenFn<F>
where
    F: Fn(&Route, &mut Window, &mut App) -> Mounted + 'static,
{
    fn keep(&self) -> Keep {
        self.keep
    }

    fn mount(&self, route: &Route, window: &mut Window, cx: &mut App) -> Mounted {
        (self.build)(route, window, cx)
    }
}

/// The registry the `app` binary fills: one screen per kind. A kind nobody registered shows a
/// neutral placeholder.
///
/// ```ignore
/// let screens = Screens::new()
///     .add(ScreenKind::Home, Keep::One, |_, window, cx| {
///         let home = cx.new(|cx| HomeScreen::new(window, cx));
///         let h = home.clone();
///         Mounted::new(home).header(move |_, cx| h.read(cx).header(cx))
///     })
///     .add(ScreenKind::Chat, Keep::PerRoute(8), |route, window, cx| { .. });
/// ```
#[derive(Clone, Default)]
pub struct Screens {
    entries: HashMap<ScreenKind, Rc<dyn Screen>>,
}

impl Screens {
    /// An empty registry: every screen is a placeholder.
    pub fn new() -> Self {
        Self::default()
    }

    /// Register `screen` for `kind`, replacing an earlier one.
    pub fn register(mut self, kind: ScreenKind, screen: impl Screen) -> Self {
        self.entries.insert(kind, Rc::new(screen));
        self
    }

    /// Register a closure that builds the screen of `kind`, keeping views as `keep` says.
    pub fn add(
        self,
        kind: ScreenKind,
        keep: Keep,
        build: impl Fn(&Route, &mut Window, &mut App) -> Mounted + 'static,
    ) -> Self {
        self.register(kind, ScreenFn { keep, build })
    }

    /// The screen registered for `kind`.
    pub fn get(&self, kind: ScreenKind) -> Option<&Rc<dyn Screen>> {
        self.entries.get(&kind)
    }

    /// A screen is registered for `kind`.
    pub fn has(&self, kind: ScreenKind) -> bool {
        self.entries.contains_key(&kind)
    }
}
