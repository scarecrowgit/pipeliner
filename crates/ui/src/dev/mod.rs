//! The dev driver: named probes and scripted input for checking the app in a real window.
//! Owned by the devtools agent; everything else in this crate is owned by the kit.
//!
//! Inert unless the environment variable [`SOCKET_ENV`] names a path. Then [`install`] listens
//! on a Unix socket there and runs one command per line on the UI thread; the client half
//! lives in the `devtools` crate (`harness-dev`). Views mark elements with
//! [`Probe::probe`] so scripts can aim at them by name.
//!
//! Protocol, one line each way (see [`proto`]): a command is space-separated words, any word
//! may be double-quoted with `\"`, `\\`, `\n`, `\t` escapes. The reply is `ok <json>` or
//! `err <message>`. A target is a probe name or an `x y` window point.
//!
//! | Command | Does |
//! | --- | --- |
//! | `windows` | id, bounds, viewport, scale and active flag of every window |
//! | `probes` | every probe of the last painted frame of each window |
//! | `click <target>` / `dblclick <target>` | mouse move, frame, down, up (left button) |
//! | `hover <target>` | mouse move, frame |
//! | `scroll <target> <dx> <dy>` | wheel in pixels (negative `dy` scrolls content down) |
//! | `drag <target> <target>` | down, 12 moves with the button held (a frame each), up |
//! | `key <keystroke>…` | each keystroke (`cmd-k`, `escape`, `enter`) pressed and released |
//! | `type <text>` | each character pressed; unhandled ones reach the focused input |
//! | `text <probe>` | the value attached with [`Probe::probe_value`] |
//! | `action <name>` | dispatch a registered action (`ui::Quit`) on the focused element |
//! | `wait` | replies after a frame painted after every earlier command |
//! | `quit` | replies, then quits the app |
//!
//! Events go through `Window::dispatch_event` (public with the workspace's vendored gpui), so
//! GPUI does its own hit testing. Pointer commands reply after their last frame was painted.
//! `wait` forces a frame and replies once it is drawn: every earlier command's events were
//! dispatched before that frame, and `probes` then lists exactly that frame (empty included).
//! It does not wait for animations (hover fades run 150 ms); sleep for those.

mod input;
mod probe;
pub mod proto;

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::Duration;

use gpui::{AnyWindowHandle, App, AppContext, Keystroke, Pixels, Point};

pub use input::char_keystroke;
pub use probe::{Probe, ProbeElement, Probed};
pub use proto::{Command, Target};

/// The environment variable that turns dev mode on and names the socket path.
pub const SOCKET_ENV: &str = "HARNESS_DEV_SOCKET";

static ENABLED: AtomicBool = AtomicBool::new(false);

/// Whether dev mode is on in this process (probes record, the socket listens).
pub fn enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

/// Tests: record probes in this process, as dev mode does.
#[cfg(test)]
pub(crate) fn enable_for_tests() {
    ENABLED.store(true, Ordering::SeqCst);
}

/// Tests: a probe named `name` was drawn in the last frame.
#[cfg(test)]
pub(crate) fn probe_drawn(name: &str) -> bool {
    probe::find(name, None).is_some()
}

/// Start the dev driver if [`SOCKET_ENV`] is set; otherwise do nothing. Call once after
/// `ui::init`. A second call is a no-op.
pub fn install(cx: &mut App) {
    let Some(path) = std::env::var_os(SOCKET_ENV).filter(|p| !p.is_empty()) else {
        return;
    };
    if ENABLED.swap(true, Ordering::SeqCst) {
        return;
    }
    let path = PathBuf::from(path);
    let _ = std::fs::remove_file(&path);
    let listener = match UnixListener::bind(&path) {
        Ok(listener) => listener,
        Err(err) => {
            eprintln!("dev driver: cannot listen on {}: {err}", path.display());
            ENABLED.store(false, Ordering::SeqCst);
            return;
        }
    };
    let (jobs_tx, jobs_rx) = mpsc::channel::<Job>();
    let spawned = std::thread::Builder::new()
        .name("harness-dev".into())
        .spawn(move || serve(listener, jobs_tx));
    if let Err(err) = spawned {
        eprintln!("dev driver: cannot start the socket thread: {err}");
        ENABLED.store(false, Ordering::SeqCst);
        return;
    }
    // The socket thread cannot touch the app; jobs cross over a channel that a foreground
    // task drains. Polling keeps `ui` free of an async channel dependency; 8 ms is below a
    // frame and only runs in dev mode.
    cx.spawn(async move |cx| {
        loop {
            loop {
                match jobs_rx.try_recv() {
                    Ok(job) => {
                        if cx.update(|cx| execute(job, cx)).is_err() {
                            return;
                        }
                    }
                    Err(mpsc::TryRecvError::Empty) => break,
                    Err(mpsc::TryRecvError::Disconnected) => return,
                }
            }
            cx.background_executor().timer(Duration::from_millis(8)).await;
        }
    })
    .detach();
    cx.on_app_quit(move |_| {
        let _ = std::fs::remove_file(&path);
        async {}
    })
    .detach();
}

// ---------------------------------------------------------------------------------------------
// Socket

type Reply = mpsc::Sender<Result<String, String>>;

struct Job {
    command: Command,
    reply: Reply,
}

fn serve(listener: UnixListener, jobs: mpsc::Sender<Job>) {
    for stream in listener.incoming() {
        let Ok(stream) = stream else { continue };
        let jobs = jobs.clone();
        let _ = std::thread::Builder::new()
            .name("harness-dev-conn".into())
            .spawn(move || connection(stream, jobs));
    }
}

/// One client: commands run one after another; the next line is read after the reply.
fn connection(stream: UnixStream, jobs: mpsc::Sender<Job>) {
    let Ok(mut out) = stream.try_clone() else { return };
    for line in BufReader::new(stream).lines() {
        let Ok(line) = line else { return };
        if line.trim().is_empty() {
            continue;
        }
        let reply = Command::parse(&line).and_then(|command| {
            let timeout = command.timeout();
            let (tx, rx) = mpsc::channel();
            jobs.send(Job { command, reply: tx })
                .map_err(|_| "the app is shutting down".to_string())?;
            rx.recv_timeout(timeout)
                .unwrap_or_else(|_| Err(format!("no answer within {} ms", timeout.as_millis())))
        });
        let line = proto::format_reply(&reply);
        if out.write_all(line.as_bytes()).and_then(|_| out.write_all(b"\n")).is_err() {
            return;
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Commands on the UI thread

fn execute(job: Job, cx: &mut App) {
    let Job { command, reply } = job;
    let result = match command {
        Command::Wait => {
            match main_window(cx) {
                Ok(window) => respond_async(reply, cx, async move |cx| {
                    input::next_frame(window, cx).await.map(|_| "null".into())
                }),
                Err(err) => drop(reply.send(Err(err))),
            }
            return;
        }
        Command::Quit => {
            let _ = reply.send(Ok("null".into()));
            cx.spawn(async move |cx| {
                cx.background_executor().timer(Duration::from_millis(50)).await;
                let _ = cx.update(|cx| cx.quit());
            })
            .detach();
            return;
        }
        Command::Click(target) => return pointer(&target, reply, cx, input::click),
        Command::DblClick(target) => return pointer(&target, reply, cx, input::dblclick),
        Command::Hover(target) => return pointer(&target, reply, cx, input::hover),
        Command::Scroll(target, dx, dy) => {
            return pointer(&target, reply, cx, move |at| input::scroll(at, dx, dy));
        }
        Command::Drag(from, to) => {
            let steps = resolve(&from, cx).and_then(|(window, a)| {
                let (other, b) = resolve(&to, cx)?;
                if other != window {
                    return Err("drag ends in another window".to_string());
                }
                Ok((window, input::drag(a, b), b))
            });
            match steps {
                Ok((window, steps, end)) => respond_async(reply, cx, async move |cx| {
                    input::run(window, steps, cx).await.map(|_| point_json(end))
                }),
                Err(err) => drop(reply.send(Err(err))),
            }
            return;
        }
        Command::Windows => Ok(windows_json(cx)),
        Command::Probes => Ok(probe::probes_json()),
        Command::Text(name) => match probe::find(&name, None) {
            Some((_, p)) => match p.value {
                Some(value) => Ok(proto::json_string(&value)),
                None => Err(format!("probe {name} has no value")),
            },
            None => Err(format!("no probe named {name}")),
        },
        Command::Key(keys) => keys_command(&keys, cx),
        Command::Type(text) => type_command(&text, cx),
        Command::Action(name) => action_command(&name, cx),
    };
    let _ = reply.send(result);
}

/// Run `f` as a foreground task and send its result as the reply.
fn respond_async(
    reply: Reply,
    cx: &mut App,
    f: impl AsyncFnOnce(&mut gpui::AsyncApp) -> Result<String, String> + 'static,
) {
    cx.spawn(async move |cx| {
        let _ = reply.send(f(cx).await);
    })
    .detach();
}

/// Resolve the target, then run the pointer sequence `steps(at)` and reply with the point.
fn pointer(
    target: &Target,
    reply: Reply,
    cx: &mut App,
    steps: impl FnOnce(Point<Pixels>) -> Vec<input::Step>,
) {
    match resolve(target, cx) {
        Ok((window, at)) => {
            let steps = steps(at);
            respond_async(reply, cx, async move |cx| {
                input::run(window, steps, cx).await.map(|_| point_json(at))
            });
        }
        Err(err) => {
            let _ = reply.send(Err(err));
        }
    }
}

/// The window commands without a probe act on: the active one, else the first.
fn main_window(cx: &App) -> Result<AnyWindowHandle, String> {
    cx.active_window()
        .or_else(|| cx.windows().first().copied())
        .ok_or_else(|| "the app has no window".to_string())
}

/// A target as a window and a point in it: a probe's visible centre, or `x y` in the main
/// window.
fn resolve(target: &Target, cx: &App) -> Result<(AnyWindowHandle, Point<Pixels>), String> {
    match target {
        Target::Point(x, y) => Ok((main_window(cx)?, gpui::point(gpui::px(*x), gpui::px(*y)))),
        Target::Probe(name) => {
            let prefer = main_window(cx).ok().map(|w| w.window_id().as_u64());
            let (window_id, probe) =
                probe::find(name, prefer).ok_or_else(|| format!("no probe named {name}"))?;
            if !probe.is_visible() {
                return Err(format!("probe {name} is clipped out of view"));
            }
            let window = cx
                .windows()
                .into_iter()
                .find(|w| w.window_id().as_u64() == window_id)
                .ok_or_else(|| format!("the window of probe {name} is gone"))?;
            Ok((window, probe.centre()))
        }
    }
}

fn keys_command(keys: &[String], cx: &mut App) -> Result<String, String> {
    let window = main_window(cx)?;
    let mut handled = Vec::new();
    for key in keys {
        let stroke = Keystroke::parse(key).map_err(|e| format!("bad keystroke {key}: {e}"))?;
        handled.push(if input::press(window, stroke, cx)? { "true" } else { "false" });
    }
    Ok(format!("{{\"handled\":[{}]}}", handled.join(",")))
}

/// Each character pressed as a keystroke carrying its `key_char`: bindings see it first, and
/// when none takes it GPUI hands it to the focused element's input handler.
fn type_command(text: &str, cx: &mut App) -> Result<String, String> {
    let window = main_window(cx)?;
    let (mut typed, mut unhandled) = (0, 0);
    for ch in text.chars() {
        if !input::press(window, char_keystroke(ch), cx)? {
            unhandled += 1;
        }
        typed += 1;
    }
    Ok(format!("{{\"typed\":{typed},\"unhandled\":{unhandled}}}"))
}

fn action_command(name: &str, cx: &mut App) -> Result<String, String> {
    let action = cx.build_action(name, None).map_err(|e| format!("{e}"))?;
    let window = main_window(cx)?;
    cx.update_window(window, |_, w, cx| w.dispatch_action(action, cx))
        .map_err(|e| format!("window gone: {e}"))?;
    Ok("null".into())
}

fn windows_json(cx: &mut App) -> String {
    let active = cx.active_window().map(|w| w.window_id().as_u64());
    let mut items = Vec::new();
    for handle in cx.windows() {
        let id = handle.window_id().as_u64();
        let info = cx.update_window(handle, |_, w, _| {
            (probe::rect(w.bounds()), w.viewport_size(), w.scale_factor())
        });
        if let Ok(([x, y, w, h], viewport, scale)) = info {
            items.push(format!(
                "{{\"id\":{id},\"x\":{x},\"y\":{y},\"width\":{w},\"height\":{h},\
                 \"viewport\":[{},{}],\"scale\":{scale},\"active\":{}}}",
                f32::from(viewport.width),
                f32::from(viewport.height),
                active == Some(id)
            ));
        }
    }
    format!("[{}]", items.join(","))
}

fn point_json(at: Point<Pixels>) -> String {
    format!("{{\"at\":[{},{}]}}", f32::from(at.x), f32::from(at.y))
}
