//! Real input through `Window::dispatch_event`: pointer sequences with a painted frame
//! between steps (as a platform would interleave them), keystrokes with their key-up, and
//! the frame barrier behind `wait`.

use std::cell::Cell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui::{
    AnyWindowHandle, App, AppContext, AsyncApp, KeyUpEvent, Keystroke, Modifiers, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, PlatformInput, Point, ScrollDelta,
    ScrollWheelEvent, TouchPhase, point, px,
};

use super::probe;

/// How long a step waits for the window to paint before the command fails.
const FRAME_TIMEOUT: Duration = Duration::from_secs(2);

/// Moves a drag is split into between mouse down and mouse up.
const DRAG_STEPS: usize = 12;

/// One step of a pointer sequence.
pub(super) enum Step {
    Event(PlatformInput),
    /// Wait until the window painted a frame after the events so far.
    Frame,
}

fn mouse_move(at: Point<Pixels>, pressed: Option<MouseButton>) -> Step {
    Step::Event(PlatformInput::MouseMove(MouseMoveEvent {
        position: at,
        pressed_button: pressed,
        modifiers: Modifiers::default(),
    }))
}

fn mouse_down(at: Point<Pixels>, click_count: usize) -> Step {
    Step::Event(PlatformInput::MouseDown(MouseDownEvent {
        button: MouseButton::Left,
        position: at,
        modifiers: Modifiers::default(),
        click_count,
        first_mouse: false,
    }))
}

fn mouse_up(at: Point<Pixels>, click_count: usize) -> Step {
    Step::Event(PlatformInput::MouseUp(MouseUpEvent {
        button: MouseButton::Left,
        position: at,
        modifiers: Modifiers::default(),
        click_count,
    }))
}

/// Move there and let the window repaint (hover state, hover-revealed controls).
pub(super) fn hover(at: Point<Pixels>) -> Vec<Step> {
    vec![mouse_move(at, None), Step::Frame]
}

/// Left click: move, frame, down, up, frame.
pub(super) fn click(at: Point<Pixels>) -> Vec<Step> {
    vec![mouse_move(at, None), Step::Frame, mouse_down(at, 1), mouse_up(at, 1), Step::Frame]
}

/// Double click: two down/up pairs with click counts 1 and 2.
pub(super) fn dblclick(at: Point<Pixels>) -> Vec<Step> {
    vec![
        mouse_move(at, None),
        Step::Frame,
        mouse_down(at, 1),
        mouse_up(at, 1),
        mouse_down(at, 2),
        mouse_up(at, 2),
        Step::Frame,
    ]
}

/// A wheel turn in pixels at a point (negative `dy` scrolls content down).
pub(super) fn scroll(at: Point<Pixels>, dx: f32, dy: f32) -> Vec<Step> {
    vec![
        mouse_move(at, None),
        Step::Frame,
        Step::Event(PlatformInput::ScrollWheel(ScrollWheelEvent {
            position: at,
            delta: ScrollDelta::Pixels(point(px(dx), px(dy))),
            modifiers: Modifiers::default(),
            touch_phase: TouchPhase::Moved,
        })),
        Step::Frame,
    ]
}

/// Press at `from`, move in steps with the button held, release at `to`; a frame after each.
pub(super) fn drag(from: Point<Pixels>, to: Point<Pixels>) -> Vec<Step> {
    let mut steps = vec![mouse_move(from, None), Step::Frame, mouse_down(from, 1), Step::Frame];
    for i in 1..=DRAG_STEPS {
        let t = i as f32 / DRAG_STEPS as f32;
        let at = point(from.x + (to.x - from.x) * t, from.y + (to.y - from.y) * t);
        steps.push(mouse_move(at, Some(MouseButton::Left)));
        steps.push(Step::Frame);
    }
    steps.push(mouse_up(to, 1));
    steps.push(Step::Frame);
    steps
}

/// Run a pointer sequence in `window`; one update per event, so effects flush in between.
pub(super) async fn run(
    window: AnyWindowHandle,
    steps: Vec<Step>,
    cx: &mut AsyncApp,
) -> Result<(), String> {
    for step in steps {
        match step {
            Step::Event(event) => {
                cx.update_window(window, |_, w, cx| {
                    w.dispatch_event(event, cx);
                })
                .map_err(|e| format!("window gone: {e}"))?;
            }
            Step::Frame => next_frame(window, cx).await?,
        }
    }
    Ok(())
}

/// Force a frame and return after it was drawn. The frame callback runs just before the
/// draw; the task it spawns can only run once the platform's frame callback, draw included,
/// has returned. The frame's probes are published even when there are none.
pub(super) async fn next_frame(window: AnyWindowHandle, cx: &mut AsyncApp) -> Result<(), String> {
    let done = Rc::new(Cell::new(false));
    let flag = done.clone();
    cx.update_window(window, move |_, w, _| {
        w.refresh();
        w.on_next_frame(move |w, cx| {
            let id = w.window_handle().window_id().as_u64();
            probe::begin_frame(id);
            cx.spawn(async move |_| {
                probe::end_frame(id);
                flag.set(true);
            })
            .detach();
        });
    })
    .map_err(|e| format!("window gone: {e}"))?;
    let deadline = Instant::now() + FRAME_TIMEOUT;
    while !done.get() {
        if Instant::now() > deadline {
            return Err("the window painted no frame within 2 s (is it visible?)".into());
        }
        cx.background_executor().timer(Duration::from_millis(4)).await;
    }
    Ok(())
}

/// Press and release `stroke`: key-down through `dispatch_keystroke` (which hands an
/// unhandled printable key to the focused input), then a real key-up (which is what
/// fires Enter/Space "keyboard clicks" on a focused control). Whether the key-down was
/// handled.
pub(super) fn press(window: AnyWindowHandle, stroke: Keystroke, cx: &mut App) -> Result<bool, String> {
    let up = KeyUpEvent { keystroke: stroke.clone().with_simulated_ime() };
    cx.update_window(window, |_, w, cx| {
        let handled = w.dispatch_keystroke(stroke, cx);
        w.dispatch_event(PlatformInput::KeyUp(up), cx);
        handled
    })
    .map_err(|e| format!("window gone: {e}"))
}

/// The keystroke a keyboard would produce for `ch`.
pub fn char_keystroke(ch: char) -> Keystroke {
    let key = match ch {
        ' ' => "space".to_string(),
        '\n' => "enter".to_string(),
        '\t' => "tab".to_string(),
        c => c.to_lowercase().collect(),
    };
    let modifiers = Modifiers { shift: ch.is_uppercase(), ..Modifiers::default() };
    Keystroke { modifiers, key, key_char: Some(ch.to_string()) }
}
