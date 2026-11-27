//! Renders static HTML mockups to PNG with a headless Chromium.
//!
//! The default path drives Chromium over the DevTools Protocol so the whole page is captured
//! (not just the `w × h` viewport) and so every render uses its own throwaway profile.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Context, Result};
use base64::Engine as _;
use serde_json::{json, Value};

/// Tallest page we capture; keeps a runaway layout from producing a gigantic PNG.
const MAX_HEIGHT: u32 = 16_384;
/// Upper bound for one whole render (browser start, load, screenshot).
const RENDER_TIMEOUT: Duration = Duration::from_secs(120);

/// Where to look when `PATH` is minimal (the app launched from a desktop entry).
const FALLBACK_BIN_DIRS: [&str; 4] = [
    "/run/current-system/sw/bin",
    "/nix/var/nix/profiles/default/bin",
    "/usr/bin",
    "/usr/local/bin",
];

/// `bin` on `PATH`, then in the usual system locations.
fn find_bin(bin: &str) -> Option<PathBuf> {
    let from_path = std::env::var_os("PATH")
        .into_iter()
        .flat_map(|p| std::env::split_paths(&p).collect::<Vec<_>>());
    let home = std::env::var_os("HOME")
        .filter(|h| !h.is_empty())
        .map(|h| PathBuf::from(h).join(".nix-profile/bin"));
    let user = std::env::var_os("USER")
        .filter(|u| !u.is_empty())
        .map(|u| PathBuf::from("/etc/profiles/per-user").join(u).join("bin"));
    let (system, rest) = FALLBACK_BIN_DIRS.split_first().expect("fallback dirs");
    // `/run/current-system/sw/bin` comes after the per-user profiles, then the rest.
    from_path
        .chain(home)
        .chain(user)
        .chain(std::iter::once(PathBuf::from(system)))
        .chain(rest.iter().map(PathBuf::from))
        .map(|d| d.join(bin))
        .find(|p| p.is_file())
}

fn q(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// `<meta name="design:size" content="1440x2080">` in a mockup sets its render size.
/// Tag and attribute names are case-insensitive, either quote style works, `name` may come
/// before or after `content`, and `×` is accepted as the separator.
pub fn design_size(p: &Path) -> Option<(u32, u32)> {
    let text = std::fs::read_to_string(p).ok()?;
    parse_design_size(&text)
}

fn parse_design_size(text: &str) -> Option<(u32, u32)> {
    let lower = text.to_ascii_lowercase(); // same byte offsets as `text`
    let bytes = text.as_bytes();
    let mut from = 0;
    while let Some(i) = lower[from..].find("<meta") {
        let start = from + i + 5;
        from = start;
        // Not `<metadata>` and friends.
        match bytes.get(start) {
            Some(c) if c.is_ascii_whitespace() || *c == b'/' => {}
            _ => continue,
        }
        let (attrs, end) = parse_attrs(text, start);
        from = end.max(from);
        let get = |n: &str| attrs.iter().find(|(k, _)| k == n).map(|(_, v)| v.as_str());
        if get("name").map(|v| v.trim().eq_ignore_ascii_case("design:size")) != Some(true) {
            continue;
        }
        let content = get("content")?.to_lowercase();
        let (w, h) = content.split_once(['x', '×'])?;
        let (w, h): (u32, u32) = (w.trim().parse().ok()?, h.trim().parse().ok()?);
        if w > 0 && h > 0 {
            return Some((w, h));
        }
        return None;
    }
    None
}

/// Attributes of the tag whose name ends at `i`, plus the index after its closing `>`.
fn parse_attrs(text: &str, mut i: usize) -> (Vec<(String, String)>, usize) {
    let b = text.as_bytes();
    let mut out = vec![];
    while i < b.len() {
        while i < b.len() && (b[i].is_ascii_whitespace() || b[i] == b'/') {
            i += 1;
        }
        if i >= b.len() {
            break;
        }
        if b[i] == b'>' {
            return (out, i + 1);
        }
        let ns = i;
        while i < b.len() && !b[i].is_ascii_whitespace() && !matches!(b[i], b'=' | b'>' | b'/') {
            i += 1;
        }
        let name = text[ns..i].to_ascii_lowercase();
        while i < b.len() && b[i].is_ascii_whitespace() {
            i += 1;
        }
        let mut value = String::new();
        if i < b.len() && b[i] == b'=' {
            i += 1;
            while i < b.len() && b[i].is_ascii_whitespace() {
                i += 1;
            }
            if i < b.len() && (b[i] == b'"' || b[i] == b'\'') {
                let quote = b[i];
                i += 1;
                let vs = i;
                while i < b.len() && b[i] != quote {
                    i += 1;
                }
                value = text[vs..i].to_string();
                i = (i + 1).min(b.len());
            } else {
                let vs = i;
                while i < b.len() && !b[i].is_ascii_whitespace() && b[i] != b'>' {
                    i += 1;
                }
                value = text[vs..i].to_string();
            }
        }
        if !name.is_empty() {
            out.push((name, value));
        }
    }
    (out, i)
}

/// Renders `input` to `output`. `command` (from config/node) may use `{in}`, `{out}`, `{w}`, `{h}`.
/// With no command, Chromium captures the full page (at least `w × h`, at most 16384 tall).
pub fn render(command: &str, input: &Path, output: &Path, w: u32, h: u32) -> Result<()> {
    if let Some(parent) = output.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let _ = std::fs::remove_file(output);
    if !command.trim().is_empty() {
        let cmd = command
            .replace("{in}", &q(&input.display().to_string()))
            .replace("{out}", &q(&output.display().to_string()))
            .replace("{w}", &w.to_string())
            .replace("{h}", &h.to_string());
        // Browsers break with an app-specific LD_LIBRARY_PATH (e.g. from a nix shell).
        let mut env = std::collections::BTreeMap::new();
        env.insert("LD_LIBRARY_PATH".to_string(), String::new());
        env.insert("NIXOS_OZONE_WL".to_string(), String::new());
        let never = std::sync::atomic::AtomicBool::new(false);
        let out = crate::tools::run_command(
            input.parent().unwrap_or(Path::new("/")),
            "",
            &env,
            &cmd,
            Duration::from_secs(180),
            &never,
        )?;
        if !output.exists() {
            bail!("render failed: {}", out.lines().last().unwrap_or(""));
        }
        return Ok(());
    }
    let url = file_url(input)?;
    let png = capture(&url, input, w, h)?;
    std::fs::write(output, png)?;
    match std::fs::metadata(output) {
        Ok(m) if m.len() > 0 => Ok(()),
        _ => bail!("render failed: {} was not written", output.display()),
    }
}

/// `file://` URL for `path` (made absolute first), percent-encoding what a URL can't carry.
pub fn file_url(path: &Path) -> Result<String> {
    use std::os::unix::ffi::OsStrExt;
    let abs = path.canonicalize()?;
    let mut url = String::from("file://");
    for &b in abs.as_os_str().as_bytes() {
        match b {
            b'/' | b':' | b'-' | b'_' | b'.' | b'~' => url.push(b as char),
            b if b.is_ascii_alphanumeric() => url.push(b as char),
            b => url.push_str(&format!("%{b:02X}")),
        }
    }
    Ok(url)
}

// ---------------------------------------------------------------------------------------
// Chromium session

/// A running Chromium plus its throwaway profile; both are cleaned up on drop.
struct Session {
    child: Child,
    dir: PathBuf,
}

impl Drop for Session {
    fn drop(&mut self) {
        // Chromium's helper processes share the process group we started it in.
        #[cfg(unix)]
        let _ = Command::new("sh")
            .args([
                "-c",
                &format!("kill -KILL -{} 2>/dev/null || true", self.child.id()),
            ])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn temp_profile_dir() -> Result<PathBuf> {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    let dir = std::env::temp_dir().join(format!(
        "pipeliner-render-{}-{}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed),
        nanos
    ));
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

fn spawn_chromium(dir: &Path, w: u32, h: u32) -> Result<Child> {
    let flags = [
        "--headless=new".to_string(),
        "--ozone-platform=headless".into(),
        "--disable-gpu".into(),
        "--no-sandbox".into(),
        "--hide-scrollbars".into(),
        "--no-first-run".into(),
        "--no-default-browser-check".into(),
        "--disable-extensions".into(),
        "--disable-background-networking".into(),
        "--force-device-scale-factor=1".into(),
        "--disable-dev-shm-usage".into(),
        format!("--user-data-dir={}", dir.display()),
        format!("--window-size={w},{h}"),
        "--remote-debugging-port=0".into(),
        "about:blank".into(),
    ];
    let mut cmd = if let Some(bin) = [
        "chromium",
        "chromium-browser",
        "google-chrome",
        "google-chrome-stable",
    ]
    .into_iter()
    .find_map(find_bin)
    {
        let mut c = Command::new(bin);
        c.args(&flags);
        c
    } else if let Some(nix_shell) = find_bin("nix-shell") {
        let quoted: Vec<String> = flags.iter().map(|f| q(f)).collect();
        let mut c = Command::new(nix_shell);
        c.args([
            "-p",
            "chromium",
            "--run",
            &format!("exec chromium {}", quoted.join(" ")),
        ]);
        c
    } else {
        bail!("no Chromium found to render mockups; install chromium or set a render command in Settings");
    };
    // Never let a Wayland desktop's settings route headless Chromium to the user's compositor.
    cmd.env_remove("LD_LIBRARY_PATH")
        .env_remove("NIXOS_OZONE_WL")
        .current_dir(dir)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    cmd.spawn().context("could not start Chromium")
}

/// Waits for `DevTools listening on ws://127.0.0.1:<port>/...` on Chromium's stderr.
fn wait_for_port(child: &mut Child, deadline: Instant) -> Result<u16> {
    let stderr = child.stderr.take().context("no stderr from Chromium")?;
    let (tx, rx) = std::sync::mpsc::channel::<String>();
    std::thread::spawn(move || {
        // Keeps draining after the port is known so Chromium never blocks on a full pipe.
        for line in BufReader::new(stderr).lines() {
            let Ok(line) = line else { break };
            let _ = tx.send(line);
        }
    });
    let mut seen: Vec<String> = vec![];
    let timed_out = |seen: &[String]| {
        if seen.is_empty() {
            anyhow!("timed out waiting for Chromium to start")
        } else {
            anyhow!(
                "timed out waiting for Chromium to start: {}",
                seen[seen.len().saturating_sub(5)..].join(" | ")
            )
        }
    };
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(timed_out(&seen));
        }
        match rx.recv_timeout(left) {
            Ok(line) => {
                if let Some(i) = line.find("DevTools listening on ws://") {
                    let rest = &line[i + "DevTools listening on ws://".len()..];
                    let hostport = rest.split('/').next().unwrap_or("");
                    if let Some(port) = hostport.rsplit(':').next().and_then(|p| p.parse().ok()) {
                        return Ok(port);
                    }
                }
                if !line.trim().is_empty() {
                    seen.push(line);
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => return Err(timed_out(&seen)),
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                bail!(
                    "Chromium exited before it was ready: {}",
                    seen.last().map(String::as_str).unwrap_or("")
                )
            }
        }
    }
}

/// The first page target's WebSocket URL from `/json/list`.
fn page_ws_url(port: u16, deadline: Instant) -> Result<String> {
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(3))
        .timeout_read(Duration::from_secs(5))
        .timeout_write(Duration::from_secs(5))
        .build();
    let url = format!("http://127.0.0.1:{port}/json/list");
    let mut last = anyhow!("no page target");
    while Instant::now() < deadline {
        match agent.get(&url).call() {
            Ok(resp) => match resp.into_json::<Value>() {
                Ok(v) => {
                    let found = v.as_array().and_then(|a| {
                        a.iter()
                            .filter(|t| t["type"] == "page")
                            .find_map(|t| t["webSocketDebuggerUrl"].as_str())
                    });
                    if let Some(u) = found {
                        return Ok(u.to_string());
                    }
                }
                Err(e) => last = e.into(),
            },
            Err(e) => last = e.into(),
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    Err(last.context("could not find the browser page"))
}

fn capture(url: &str, input: &Path, w: u32, h: u32) -> Result<Vec<u8>> {
    let deadline = Instant::now() + RENDER_TIMEOUT;
    let dir = temp_profile_dir()?;
    let child = match spawn_chromium(&dir, w, h) {
        Ok(c) => c,
        Err(e) => {
            let _ = std::fs::remove_dir_all(&dir);
            return Err(e);
        }
    };
    let mut session = Session { child, dir };
    let port = wait_for_port(&mut session.child, deadline)?;
    let ws_url = page_ws_url(port, deadline)?;
    let mut cdp = Cdp::connect(&ws_url, deadline)?;

    cdp.call("Page.enable", json!({}))?;
    let metrics = |height: u32| json!({"width": w, "height": height, "deviceScaleFactor": 1, "mobile": false});
    // Start at the design height so `100vh` layouts measure against it, not a taller viewport.
    cdp.call("Emulation.setDeviceMetricsOverride", metrics(h))?;
    // The initial about:blank page may have fired its own load event by now.
    cdp.events.clear();
    let nav = cdp.call("Page.navigate", json!({"url": url}))?;
    if let Some(err) = nav["errorText"].as_str().filter(|s| !s.is_empty()) {
        bail!("Chromium could not open {}: {err}", input.display());
    }
    cdp.wait_event("Page.loadEventFired")?;
    let measured = cdp.eval(
        "(async () => {\
           const frame = () => new Promise(r => { let d = false; \
             const f = () => { if (!d) { d = true; r(); } }; \
             requestAnimationFrame(f); setTimeout(f, 300); });\
           if (document.fonts) { await document.fonts.ready; }\
           await frame(); await frame();\
           await new Promise(r => setTimeout(r, 80));\
           return Math.ceil(Math.max(\
             document.documentElement ? document.documentElement.scrollHeight : 0,\
             document.body ? document.body.scrollHeight : 0));\
         })()",
    )?;
    let measured_h = measured.as_f64().unwrap_or(0.0).max(0.0) as u32;
    let full_h = measured_h.max(h);
    let capture_h = full_h.min(MAX_HEIGHT);
    if capture_h != h {
        cdp.call("Emulation.setDeviceMetricsOverride", metrics(capture_h))?;
        cdp.eval(
            "new Promise(r => { let d = false; const f = () => { if (!d) { d = true; r(1); } }; \
             requestAnimationFrame(f); setTimeout(f, 300); })",
        )?;
    }
    let shot = cdp.call(
        "Page.captureScreenshot",
        json!({
            "format": "png",
            "captureBeyondViewport": true,
            "clip": {"x": 0, "y": 0, "width": w, "height": capture_h, "scale": 1}
        }),
    )?;
    let data = shot["data"]
        .as_str()
        .context("Chromium returned no screenshot data")?;
    let png = base64::engine::general_purpose::STANDARD
        .decode(data)
        .context("bad screenshot data")?;
    // Best effort: let Chromium shut down by itself; Session::drop kills whatever is left.
    let _ = cdp.call("Browser.close", json!({}));
    let quit_by = Instant::now() + Duration::from_secs(2);
    while Instant::now() < quit_by {
        if matches!(session.child.try_wait(), Ok(Some(_))) {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    Ok(png)
}

// ---------------------------------------------------------------------------------------
// DevTools Protocol over a minimal WebSocket client

struct Cdp {
    reader: BufReader<TcpStream>,
    writer: TcpStream,
    next_id: u64,
    /// Events that arrived while waiting for something else.
    events: Vec<String>,
    deadline: Instant,
}

impl Cdp {
    fn connect(ws_url: &str, deadline: Instant) -> Result<Cdp> {
        let rest = ws_url
            .strip_prefix("ws://")
            .context("unexpected DevTools URL")?;
        let (hostport, path) = match rest.find('/') {
            Some(i) => (&rest[..i], &rest[i..]),
            None => (rest, "/"),
        };
        let stream = TcpStream::connect(hostport).context("could not reach Chromium")?;
        let read_for = deadline
            .saturating_duration_since(Instant::now())
            .min(Duration::from_secs(60))
            .max(Duration::from_secs(1));
        stream.set_read_timeout(Some(read_for))?;
        stream.set_write_timeout(Some(Duration::from_secs(30)))?;
        stream.set_nodelay(true).ok();
        let mut writer = stream.try_clone()?;
        let mut reader = BufReader::new(stream);
        let key = base64::engine::general_purpose::STANDARD.encode(random_bytes::<16>());
        write!(
            writer,
            "GET {path} HTTP/1.1\r\nHost: {hostport}\r\nUpgrade: websocket\r\n\
             Connection: Upgrade\r\nSec-WebSocket-Key: {key}\r\nSec-WebSocket-Version: 13\r\n\r\n"
        )?;
        writer.flush()?;
        let mut status = String::new();
        reader.read_line(&mut status)?;
        if !status.contains(" 101") {
            bail!("DevTools refused the connection: {}", status.trim());
        }
        loop {
            let mut line = String::new();
            if reader.read_line(&mut line)? == 0 {
                bail!("DevTools closed the connection during the handshake");
            }
            if line.trim().is_empty() {
                break;
            }
        }
        Ok(Cdp {
            reader,
            writer,
            next_id: 0,
            events: vec![],
            deadline,
        })
    }

    fn send(&mut self, method: &str, params: Value) -> Result<u64> {
        self.next_id += 1;
        let msg = json!({"id": self.next_id, "method": method, "params": params});
        write_frame(&mut self.writer, 0x1, msg.to_string().as_bytes())?;
        Ok(self.next_id)
    }

    /// Next text message (continuations joined; pings answered; close is an error).
    fn recv(&mut self) -> Result<String> {
        let mut text: Vec<u8> = vec![];
        loop {
            if Instant::now() > self.deadline {
                bail!("render timed out");
            }
            let mut head = [0u8; 2];
            self.reader.read_exact(&mut head)?;
            let fin = head[0] & 0x80 != 0;
            let opcode = head[0] & 0x0f;
            let masked = head[1] & 0x80 != 0;
            let len = match head[1] & 0x7f {
                126 => {
                    let mut b = [0u8; 2];
                    self.reader.read_exact(&mut b)?;
                    u16::from_be_bytes(b) as u64
                }
                127 => {
                    let mut b = [0u8; 8];
                    self.reader.read_exact(&mut b)?;
                    u64::from_be_bytes(b)
                }
                n => n as u64,
            };
            if len > 512 * 1024 * 1024 {
                bail!("DevTools frame too large");
            }
            let mut mask = [0u8; 4];
            if masked {
                self.reader.read_exact(&mut mask)?;
            }
            let mut payload = vec![0u8; len as usize];
            self.reader.read_exact(&mut payload)?;
            if masked {
                for (i, b) in payload.iter_mut().enumerate() {
                    *b ^= mask[i % 4];
                }
            }
            match opcode {
                0x0..=0x2 => {
                    text.extend_from_slice(&payload);
                    if fin {
                        return String::from_utf8(text).context("DevTools sent invalid text");
                    }
                }
                0x8 => bail!("Chromium closed the DevTools connection"),
                0x9 => write_frame(&mut self.writer, 0xA, &payload)?,
                _ => {}
            }
        }
    }

    /// Sends a command and waits for its reply (events in between are remembered).
    fn call(&mut self, method: &str, params: Value) -> Result<Value> {
        let id = self.send(method, params)?;
        loop {
            let v: Value = serde_json::from_str(&self.recv()?).context("bad DevTools message")?;
            if v["id"].as_u64() == Some(id) {
                if let Some(e) = v.get("error") {
                    bail!(
                        "{method} failed: {}",
                        e["message"].as_str().unwrap_or("error")
                    );
                }
                return Ok(v["result"].clone());
            }
            if let Some(m) = v["method"].as_str() {
                self.events.push(m.to_string());
            }
        }
    }

    fn wait_event(&mut self, method: &str) -> Result<()> {
        loop {
            if let Some(i) = self.events.iter().position(|m| m == method) {
                self.events.remove(i);
                return Ok(());
            }
            let v: Value = serde_json::from_str(&self.recv()?).context("bad DevTools message")?;
            if let Some(m) = v["method"].as_str() {
                self.events.push(m.to_string());
            }
        }
    }

    /// `Runtime.evaluate` of an async expression; returns its value.
    fn eval(&mut self, expression: &str) -> Result<Value> {
        let r = self.call(
            "Runtime.evaluate",
            json!({"expression": expression, "awaitPromise": true, "returnByValue": true}),
        )?;
        if let Some(ex) = r.get("exceptionDetails") {
            bail!(
                "page script failed: {}",
                ex["exception"]["description"]
                    .as_str()
                    .or(ex["text"].as_str())
                    .unwrap_or("error")
            );
        }
        Ok(r["result"]["value"].clone())
    }
}

fn random_bytes<const N: usize>() -> [u8; N] {
    use std::hash::{BuildHasher, Hasher};
    let mut out = [0u8; N];
    let mut i = 0;
    while i < N {
        // RandomState is seeded from the OS; plenty for a handshake nonce and frame masks.
        let n = std::collections::hash_map::RandomState::new()
            .build_hasher()
            .finish()
            .to_le_bytes();
        for b in n {
            if i < N {
                out[i] = b;
                i += 1;
            }
        }
    }
    out
}

/// One masked, final client frame (RFC 6455 §5.2).
fn write_frame(w: &mut impl Write, opcode: u8, payload: &[u8]) -> Result<()> {
    let mut frame = Vec::with_capacity(payload.len() + 14);
    frame.push(0x80 | opcode);
    let len = payload.len();
    if len < 126 {
        frame.push(0x80 | len as u8);
    } else if len <= u16::MAX as usize {
        frame.push(0x80 | 126);
        frame.extend_from_slice(&(len as u16).to_be_bytes());
    } else {
        frame.push(0x80 | 127);
        frame.extend_from_slice(&(len as u64).to_be_bytes());
    }
    let mask = random_bytes::<4>();
    frame.extend_from_slice(&mask);
    frame.extend(payload.iter().enumerate().map(|(i, b)| b ^ mask[i % 4]));
    w.write_all(&frame)?;
    w.flush()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn design_size_variants() {
        let p = |s: &str| parse_design_size(s);
        assert_eq!(
            p(r#"<meta name="design:size" content="1440x1840">"#),
            Some((1440, 1840))
        );
        assert_eq!(
            p(r#"<meta content="1280x960" name="design:size">"#),
            Some((1280, 960))
        );
        assert_eq!(
            p("<head><META NAME='design:size' CONTENT='900x700' /></head>"),
            Some((900, 700))
        );
        assert_eq!(
            p(r#"<meta charset="utf-8"><meta name="design:size" content="1440×2000">"#),
            Some((1440, 2000))
        );
        assert_eq!(
            p("<html><meta name=\"viewport\" content=\"1x1\"></html>"),
            None
        );
        assert_eq!(p(r#"<meta name="design:size" content="wide">"#), None);
        assert_eq!(p(r#"<meta name="design:size" content="0x900">"#), None);
        assert_eq!(p(r#"<meta name="design:size" content="1440x-5">"#), None);
        assert_eq!(p(r#"<meta name="design:size">"#), None);
    }

    #[test]
    fn file_url_encodes_special_characters() {
        let dir = std::env::temp_dir().join(format!("pipeliner url test-{}", std::process::id()));
        let odd = dir.join("a #b%c?d é.html");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(&odd, "x").unwrap();
        let url = file_url(&odd);
        let _ = std::fs::remove_dir_all(&dir);
        let url = url.unwrap();
        assert!(url.starts_with("file:///"), "{url}");
        assert!(url.ends_with("/a%20%23b%25c%3Fd%20%C3%A9.html"), "{url}");
        assert!(url.contains("pipeliner%20url%20test-"), "{url}");
        assert!(!url.contains(['#', '?', ' ']), "{url}");
        assert!(file_url(Path::new("/definitely/not/here")).is_err());
    }

    #[test]
    fn websocket_frame_lengths() {
        for len in [0usize, 5, 125, 126, 70_000] {
            let mut out = vec![];
            write_frame(&mut out, 0x1, &vec![b'a'; len]).unwrap();
            let header = match len {
                0..=125 => 2,
                126..=65535 => 4,
                _ => 10,
            };
            assert_eq!(out.len(), header + 4 + len);
            assert_eq!(out[0], 0x81);
            assert_eq!(out[1] & 0x80, 0x80);
        }
    }

    #[test]
    fn renders_the_whole_page() {
        if find_bin("chromium").is_none()
            && find_bin("chromium-browser").is_none()
            && find_bin("nix-shell").is_none()
        {
            eprintln!("no chromium available; skipping");
            return;
        }
        let dir =
            std::env::temp_dir().join(format!("pipeliner-render-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let html = dir.join("tall.html");
        let png = dir.join("out").join("tall.png");
        std::fs::write(
            &html,
            r#"<!doctype html><html><head><meta name="design:size" content="800x400">
<style>body{margin:0}</style></head>
<body><div style="height: 950px; background: #223344">tall</div></body></html>"#,
        )
        .unwrap();
        let (w, h) = design_size(&html).unwrap();
        assert_eq!((w, h), (800, 400));
        let res = render("", &html, &png, w, h);
        let bytes = std::fs::read(&png).unwrap_or_default();
        let _ = std::fs::remove_dir_all(&dir);
        res.unwrap();
        let (_, _, pw, ph) = harness_core::image::sniff(&bytes).expect("a PNG");
        assert_eq!(pw, 800);
        assert!(ph >= 950, "height {ph}");
    }
}
