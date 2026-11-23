//! The client half of the dev driver: talk to an app's `HARNESS_DEV_SOCKET`, or launch an
//! app binary with a fresh socket and drive it.
//!
//! GPUI dlopens wayland and vulkan, so a launched binary needs the Nix shell's
//! `LD_LIBRARY_PATH`; run the caller inside `nix-shell shell.nix --run '…' </dev/null`.

use std::fs::File;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command as Process, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Context, anyhow, bail};
use serde_json::Value;
pub use ui::dev::proto::{Command, Target};
use ui::dev::{SOCKET_ENV, proto};

use crate::process::{ScratchDir, tail};

/// A probe target by name.
pub fn probe(name: &str) -> Target {
    Target::Probe(name.to_string())
}

/// A window-point target, in logical pixels.
pub fn at(x: f32, y: f32) -> Target {
    Target::Point(x, y)
}

/// One app window as the app sees it (logical pixels).
#[derive(Clone, Debug, PartialEq)]
pub struct WindowInfo {
    pub id: u64,
    pub width: f32,
    pub height: f32,
    pub scale: f32,
    pub active: bool,
}

/// One probe from the last painted frame (window pixels).
#[derive(Clone, Debug, PartialEq)]
pub struct ProbeInfo {
    pub name: String,
    pub window: u64,
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub visible: bool,
    pub value: Option<String>,
}

/// A connection to a running app's dev socket.
pub struct Driver {
    reader: BufReader<UnixStream>,
    writer: UnixStream,
}

impl Driver {
    /// Connect to the socket at `path`.
    pub fn connect(path: &Path) -> anyhow::Result<Driver> {
        let writer = UnixStream::connect(path)
            .with_context(|| format!("connect to the dev socket {}", path.display()))?;
        writer.set_read_timeout(Some(Duration::from_secs(20)))?;
        let reader = BufReader::new(writer.try_clone()?);
        Ok(Driver { reader, writer })
    }

    /// Send one raw protocol line; the payload of `ok`, or the app's `err` as an error.
    pub fn send_line(&mut self, line: &str) -> anyhow::Result<String> {
        if line.contains('\n') {
            bail!("a command is one line; quote newlines as \\n");
        }
        self.writer.write_all(line.as_bytes())?;
        self.writer.write_all(b"\n")?;
        let mut reply = String::new();
        if self.reader.read_line(&mut reply)? == 0 {
            bail!("the app closed the dev socket");
        }
        proto::parse_reply(&reply).map_err(|e| anyhow!("{line}: {e}"))
    }

    /// Send a command; its JSON payload.
    pub fn send(&mut self, command: &Command) -> anyhow::Result<Value> {
        let payload = self.send_line(&command.to_line())?;
        if payload.is_empty() {
            return Ok(Value::Null);
        }
        serde_json::from_str(&payload).with_context(|| format!("reply is not JSON: {payload}"))
    }

    /// The app's windows.
    pub fn windows(&mut self) -> anyhow::Result<Vec<WindowInfo>> {
        parse_windows(&self.send(&Command::Windows)?)
    }

    /// The probes of the last painted frame.
    pub fn probes(&mut self) -> anyhow::Result<Vec<ProbeInfo>> {
        parse_probes(&self.send(&Command::Probes)?)
    }

    /// Click a target (a probe or a window point) with real mouse events.
    pub fn click(&mut self, target: Target) -> anyhow::Result<()> {
        self.send(&Command::Click(target)).map(drop)
    }

    /// Double-click a target.
    pub fn dblclick(&mut self, target: Target) -> anyhow::Result<()> {
        self.send(&Command::DblClick(target)).map(drop)
    }

    /// Move the pointer over a target.
    pub fn hover(&mut self, target: Target) -> anyhow::Result<()> {
        self.send(&Command::Hover(target)).map(drop)
    }

    /// Turn the wheel over a target by a pixel delta (negative `dy` scrolls down).
    pub fn scroll(&mut self, target: Target, dx: f32, dy: f32) -> anyhow::Result<()> {
        self.send(&Command::Scroll(target, dx, dy)).map(drop)
    }

    /// Press at `from`, move to `to` with the button held, release.
    pub fn drag(&mut self, from: Target, to: Target) -> anyhow::Result<()> {
        self.send(&Command::Drag(from, to)).map(drop)
    }

    /// Press keystrokes (`cmd-k`, `escape`); whether each was handled.
    pub fn key(&mut self, keys: &[&str]) -> anyhow::Result<Vec<bool>> {
        let reply = self.send(&Command::Key(keys.iter().map(|k| k.to_string()).collect()))?;
        Ok(reply["handled"].as_array().into_iter().flatten().filter_map(Value::as_bool).collect())
    }

    /// Type text into whatever has focus; the number of characters nobody took.
    pub fn type_text(&mut self, text: &str) -> anyhow::Result<u64> {
        let reply = self.send(&Command::Type(text.into()))?;
        Ok(reply["unhandled"].as_u64().unwrap_or(0))
    }

    /// The value attached to a probe with `probe_value`.
    pub fn text(&mut self, name: &str) -> anyhow::Result<String> {
        let reply = self.send(&Command::Text(name.into()))?;
        reply.as_str().map(str::to_string).ok_or_else(|| anyhow!("text reply is not a string"))
    }

    /// Dispatch a registered action by name (`ui::Quit`).
    pub fn action(&mut self, name: &str) -> anyhow::Result<()> {
        self.send(&Command::Action(name.into())).map(drop)
    }

    /// Return once the app painted a frame after every earlier command.
    pub fn wait(&mut self) -> anyhow::Result<()> {
        self.send(&Command::Wait).map(drop)
    }

    /// Ask the app to quit.
    pub fn quit(&mut self) -> anyhow::Result<()> {
        self.send(&Command::Quit).map(drop)
    }
}

/// Read a `windows` payload.
pub fn parse_windows(value: &Value) -> anyhow::Result<Vec<WindowInfo>> {
    let list = value.as_array().ok_or_else(|| anyhow!("windows: not a list"))?;
    Ok(list
        .iter()
        .map(|w| WindowInfo {
            id: w["id"].as_u64().unwrap_or(0),
            width: num(&w["width"]),
            height: num(&w["height"]),
            scale: num(&w["scale"]),
            active: w["active"].as_bool().unwrap_or(false),
        })
        .collect())
}

/// Read a `probes` payload.
pub fn parse_probes(value: &Value) -> anyhow::Result<Vec<ProbeInfo>> {
    let list = value.as_array().ok_or_else(|| anyhow!("probes: not a list"))?;
    Ok(list
        .iter()
        .map(|p| ProbeInfo {
            name: p["name"].as_str().unwrap_or_default().to_string(),
            window: p["window"].as_u64().unwrap_or(0),
            x: num(&p["x"]),
            y: num(&p["y"]),
            width: num(&p["width"]),
            height: num(&p["height"]),
            visible: p["visible"].as_bool().unwrap_or(false),
            value: p["value"].as_str().map(str::to_string),
        })
        .collect())
}

fn num(v: &Value) -> f32 {
    v.as_f64().unwrap_or(0.) as f32
}

/// An app started by [`launch`]: its pid, its driver, its log. Quits (then kills) the app and
/// removes the socket directory on drop.
pub struct Running {
    pub pid: u32,
    pub socket: PathBuf,
    pub driver: Driver,
    child: Child,
    log: PathBuf,
    _dir: ScratchDir,
}

impl Running {
    /// The last lines the app printed.
    pub fn log_tail(&self) -> String {
        tail(&std::fs::read_to_string(&self.log).unwrap_or_default(), 20)
    }

    /// Quit the app, killing it if it does not exit within a second.
    pub fn stop(&mut self) {
        if matches!(self.child.try_wait(), Ok(None)) {
            let _ = self.driver.quit();
            let deadline = Instant::now() + Duration::from_secs(1);
            while Instant::now() < deadline {
                if !matches!(self.child.try_wait(), Ok(None)) {
                    return;
                }
                std::thread::sleep(Duration::from_millis(25));
            }
            let _ = self.child.kill();
        }
        let _ = self.child.wait();
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Start `binary` with `args` and `env`, dev mode on at a fresh socket; return once the
/// socket answers and the app has a window (60 s at most).
pub fn launch(binary: &Path, args: &[String], env: &[(String, String)]) -> anyhow::Result<Running> {
    let dir = ScratchDir::new("app")?;
    let socket = dir.path().join("dev.sock");
    let log = dir.path().join("app.log");
    let out = File::create(&log)?;
    let mut child = Process::new(binary)
        .args(args)
        .envs(env.iter().map(|(k, v)| (k, v)))
        .env(SOCKET_ENV, &socket)
        .stdin(Stdio::null())
        .stdout(out.try_clone()?)
        .stderr(out)
        .spawn()
        .with_context(|| format!("start {}", binary.display()))?;
    let pid = child.id();
    let deadline = Instant::now() + Duration::from_secs(60);
    let failure = loop {
        if let Ok(Some(status)) = child.try_wait() {
            break format!("the app exited ({status}) before its dev socket answered");
        }
        if Instant::now() > deadline {
            break "the dev socket did not answer within 60 s (is ui::dev::install called?)"
                .to_string();
        }
        if socket.exists()
            && let Ok(mut driver) = Driver::connect(&socket)
            && driver.windows().is_ok_and(|w| !w.is_empty())
        {
            return Ok(Running { pid, socket, driver, child, log, _dir: dir });
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    let _ = child.kill();
    let _ = child.wait();
    let printed = tail(&std::fs::read_to_string(&log).unwrap_or_default(), 20);
    bail!("{failure}\n{printed}")
}
