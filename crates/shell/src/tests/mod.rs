//! The shell in a test window over the fixture backend: navigation, view caching, sidebar data
//! following hub events, collapse, the placeholder, toasts, row actions and keys.

mod chrome;
mod sidebar;

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use gpui::{
    AnyView, Bounds, Context, Entity, IntoElement, Modifiers, Render, TestAppContext, VisualTestContext, Window,
    WindowBounds, WindowOptions, div, prelude::*, px,
};
use hub::fixtures::{FixtureApi, World};
use hub::{Hub, Route};

use crate::{Keep, Mounted, ScreenKind, Screens, Shell};

/// The fixed clock of the test world.
const NOW: i64 = 1_795_000_000_000;

/// Which screens were built, in order.
type Log = Rc<RefCell<Vec<String>>>;

/// A registered test screen: shows its route as text.
struct Page {
    route: Route,
}

impl Render for Page {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_full().child(format!("{:?}", self.route))
    }
}

/// A stand-in palette for the palette slot.
struct Palette;

impl Render for Palette {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().id("palette").debug_selector(|| "palette".into()).occlude().size(px(200.))
    }
}

fn page(kind: &'static str, log: &Log) -> impl Fn(&Route, &mut Window, &mut gpui::App) -> Mounted + 'static {
    let log = log.clone();
    move |route, _, cx| {
        log.borrow_mut().push(format!("{kind} {route:?}"));
        let route = route.clone();
        Mounted::new(cx.new(|_| Page { route }))
    }
}

/// Home, Inbox, Chats, Chat (kept per session) and New chat; Issues, Agents and the editor
/// are left to the placeholder.
fn screens(log: &Log) -> Screens {
    Screens::new()
        .add(ScreenKind::Home, Keep::One, page("home", log))
        .add(ScreenKind::Inbox, Keep::One, page("inbox", log))
        .add(ScreenKind::Chats, Keep::One, page("chats", log))
        .add(ScreenKind::Chat, Keep::PerRoute(4), page("chat", log))
        .add(ScreenKind::NewChat, Keep::One, page("new-chat", log))
}

struct Setup {
    api: Arc<FixtureApi>,
    hub: Entity<Hub>,
    shell: Entity<Shell>,
    log: Log,
    cx: &'static mut VisualTestContext,
}

fn setup(cx: &mut TestAppContext) -> Setup {
    let api = Arc::new(FixtureApi::new(World::acme(NOW)));
    let hub = cx.update(|cx| {
        ui::motion::set_reduced(true);
        ui::keys::init(cx);
        crate::init(cx);
        Hub::new(api.clone(), cx)
    });
    let log: Log = Rc::default();
    let registry = screens(&log);
    let slot: Rc<RefCell<Option<Entity<Shell>>>> = Rc::default();
    let out = slot.clone();
    let window = cx
        .update(|cx| {
            let bounds = Bounds::maximized(None, cx);
            cx.open_window(
                WindowOptions { window_bounds: Some(WindowBounds::Windowed(bounds)), ..Default::default() },
                move |window, cx| {
                    let (shell, root) = crate::mount(registry, window, cx);
                    *out.borrow_mut() = Some(shell);
                    root
                },
            )
        })
        .expect("a test window");
    let vcx = VisualTestContext::from_window(window.into(), cx).into_mut();
    vcx.run_until_parked();
    let shell = slot.borrow_mut().take().expect("the shell");
    Setup { api, hub, shell, log, cx: vcx }
}

impl Setup {
    fn click(&mut self, selector: &str) {
        let selector: &'static str = Box::leak(selector.to_string().into_boxed_str());
        let bounds = self.cx.debug_bounds(selector).unwrap_or_else(|| panic!("`{selector}` was not laid out"));
        self.cx.simulate_click(bounds.center(), Modifiers::none());
        self.cx.run_until_parked();
    }

    fn hover(&mut self, selector: &str) {
        let selector: &'static str = Box::leak(selector.to_string().into_boxed_str());
        let bounds = self.cx.debug_bounds(selector).unwrap_or_else(|| panic!("`{selector}` was not laid out"));
        self.cx.simulate_mouse_move(bounds.center(), None, Modifiers::none());
        self.cx.run_until_parked();
    }

    /// The element was laid out in some frame so far (GPUI keeps debug bounds across frames,
    /// so this cannot say an element is gone).
    fn drawn(&mut self, selector: &str) -> bool {
        let selector: &'static str = Box::leak(selector.to_string().into_boxed_str());
        self.cx.debug_bounds(selector).is_some()
    }

    fn navigate(&mut self, route: Route) {
        self.cx.update(|_, cx| hub::navigate(route, cx));
        self.cx.run_until_parked();
    }

    fn route(&mut self) -> Route {
        let shell = self.shell.clone();
        self.cx.update(|_, cx| shell.read(cx).route().cloned()).expect("a route")
    }

    fn body(&mut self) -> AnyView {
        let shell = self.shell.clone();
        self.cx.update(|_, cx| shell.read(cx).body().cloned()).expect("a screen")
    }

    fn shown(&mut self) -> crate::Shown {
        let shell = self.shell.clone();
        self.cx.update(|_, cx| shell.read(cx).sidebar().read(cx).shown().clone())
    }

    fn recorded(&self, name: &str) -> Vec<String> {
        self.api.recorded().into_iter().filter(|r| r.name == name).map(|r| r.detail).collect()
    }
}
