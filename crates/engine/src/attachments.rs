//! Image attachments for chat messages: saving, loading for the model, and screenshots.

use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{anyhow, bail, Context as _, Result};
use base64::Engine as _;
use harness_core::{image, new_id, paths, Attachment};

use crate::llm::ImageData;

/// Largest image we accept.
pub const MAX_BYTES: usize = 20 * 1024 * 1024;

/// Validates `bytes` as an image and stores it in `paths::attachments_dir()`.
/// `name` defaults to `Screenshot HH.MM` (local time).
pub fn save_bytes(bytes: &[u8], name: Option<&str>, pasted: bool) -> Result<Attachment> {
    save_in(&paths::attachments_dir(), bytes, name, pasted)
}

fn save_in(dir: &Path, bytes: &[u8], name: Option<&str>, pasted: bool) -> Result<Attachment> {
    if bytes.len() > MAX_BYTES {
        bail!("image is larger than 20 MB");
    }
    let (mime, ext, width, height) =
        image::sniff(bytes).ok_or_else(|| anyhow!("not a PNG, JPEG, GIF or WebP image"))?;
    std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    let path = dir.join(format!("{}.{ext}", new_id()));
    std::fs::write(&path, bytes).with_context(|| format!("writing {}", path.display()))?;
    let name = name
        .map(|n| n.trim().to_string())
        .filter(|n| !n.is_empty())
        .unwrap_or_else(screenshot_name);
    Ok(Attachment {
        path: path.to_string_lossy().into_owned(),
        name,
        mime: mime.into(),
        width,
        height,
        pasted,
    })
}

fn screenshot_name() -> String {
    chrono::Local::now().format("Screenshot %H.%M").to_string()
}

/// Copies an image file into the attachments folder.
pub fn add_file(path: &Path) -> Result<Attachment> {
    let len = std::fs::metadata(path)
        .with_context(|| format!("reading {}", path.display()))?
        .len();
    if len > MAX_BYTES as u64 {
        bail!("image is larger than 20 MB");
    }
    let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned());
    save_bytes(&bytes, name.as_deref(), false)
}

/// Deletes an attachment file, only if it is inside the attachments folder. Errors are ignored.
pub fn discard(a_path: &str) {
    discard_in(&paths::attachments_dir(), a_path);
}

fn discard_in(dir: &Path, a_path: &str) {
    let (Ok(dir), Ok(file)) = (dir.canonicalize(), Path::new(a_path).canonicalize()) else {
        return;
    };
    if file.is_file() && file.starts_with(&dir) {
        let _ = std::fs::remove_file(file);
    }
}

/// Reads an attachment for the model. `None` if the file is gone.
pub fn load(a: &Attachment) -> Option<ImageData> {
    let bytes = std::fs::read(&a.path).ok()?;
    Some(ImageData {
        mime: a.mime.clone(),
        base64: base64::engine::general_purpose::STANDARD.encode(bytes),
    })
}

/// Loads the images of a message. Files that are missing add a note to `text` instead.
pub fn images_for(attachments: &[Attachment], mut text: String) -> (String, Vec<ImageData>) {
    let mut images = vec![];
    for a in attachments {
        match load(a) {
            Some(i) => images.push(i),
            None => text.push_str(&format!("\n[image {} is no longer on disk]", a.name)),
        }
    }
    (text, images)
}

// ---- screenshots -----------------------------------------------------------

const CANCELLED: &str = "Screenshot cancelled";

fn find_tool(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|d| d.join(name))
        .find(|p| p.is_file())
}

/// Takes an interactive region screenshot with the first tool available. Blocks until the
/// user selects a region or cancels, so call it from a background thread.
pub fn capture_screenshot() -> Result<Attachment> {
    let dir = paths::attachments_dir();
    std::fs::create_dir_all(&dir)?;
    let tmp = dir.join(format!("capture-{}.png", new_id()));
    let res = capture_to(&tmp);
    let bytes = match res {
        Ok(Some(raw)) => raw,
        Ok(None) => std::fs::read(&tmp).unwrap_or_default(),
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            return Err(e);
        }
    };
    let _ = std::fs::remove_file(&tmp);
    if bytes.is_empty() {
        bail!(CANCELLED);
    }
    save_bytes(&bytes, Some(&screenshot_name()), false)
}

fn run_ok(cmd: &mut Command) -> bool {
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Runs a capture tool. Returns `Some(bytes)` for tools that print the image to stdout,
/// `None` when the image was written to `file`.
fn capture_to(file: &Path) -> Result<Option<Vec<u8>>> {
    let f = file.to_string_lossy().into_owned();
    let ok = if cfg!(target_os = "macos") {
        if find_tool("screencapture").is_none() {
            bail!(NO_TOOL);
        }
        run_ok(Command::new("screencapture").args(["-i", &f]))
    } else if let (true, Some(_), Some(_)) = (
        std::env::var_os("WAYLAND_DISPLAY").is_some(),
        find_tool("grim"),
        find_tool("slurp"),
    ) {
        let out = Command::new("slurp")
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .context("running slurp")?;
        let geom = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if !out.status.success() || geom.is_empty() {
            bail!(CANCELLED);
        }
        run_ok(Command::new("grim").args(["-g", &geom, &f]))
    } else if find_tool("gnome-screenshot").is_some() {
        run_ok(Command::new("gnome-screenshot").args(["-a", "-f", &f]))
    } else if find_tool("spectacle").is_some() {
        run_ok(Command::new("spectacle").args(["-b", "-n", "-r", "-o", &f]))
    } else if find_tool("flameshot").is_some() {
        // `flameshot gui --raw` prints the PNG to stdout.
        let mut child = Command::new("flameshot")
            .args(["gui", "--raw"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .context("running flameshot")?;
        let mut buf = vec![];
        if let Some(mut out) = child.stdout.take() {
            let _ = out.read_to_end(&mut buf);
        }
        let ok = child.wait().map(|s| s.success()).unwrap_or(false);
        if !ok {
            buf.clear();
        }
        return Ok(Some(buf));
    } else if find_tool("scrot").is_some() {
        run_ok(Command::new("scrot").args(["-s", &f]))
    } else if find_tool("import").is_some() {
        run_ok(Command::new("import").arg(&f))
    } else {
        bail!(NO_TOOL);
    };
    if !ok {
        bail!(CANCELLED);
    }
    Ok(None)
}

const NO_TOOL: &str = "No screenshot tool found (install grim+slurp, gnome-screenshot, spectacle, flameshot, scrot or ImageMagick)";

#[cfg(test)]
mod tests {
    use super::*;

    fn png(w: u32, h: u32) -> Vec<u8> {
        let mut b = b"\x89PNG\r\n\x1a\n".to_vec();
        b.extend_from_slice(&13u32.to_be_bytes());
        b.extend_from_slice(b"IHDR");
        b.extend_from_slice(&w.to_be_bytes());
        b.extend_from_slice(&h.to_be_bytes());
        b.extend_from_slice(&[8, 6, 0, 0, 0]);
        b
    }

    fn temp_dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("pipeliner-att-{tag}-{}", new_id()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn saves_loads_and_discards() {
        let dir = temp_dir("save");
        let a = save_in(&dir, &png(10, 20), Some("shot"), true).unwrap();
        assert_eq!((a.width, a.height, a.mime.as_str()), (10, 20, "image/png"));
        assert!(a.pasted && a.name == "shot" && a.path.ends_with(".png"));
        let img = load(&a).unwrap();
        assert_eq!(img.mime, "image/png");
        assert!(!img.base64.is_empty());
        let unnamed = save_in(&dir, &png(1, 1), None, false).unwrap();
        assert!(unnamed.name.starts_with("Screenshot "));

        // Files outside the folder are never deleted.
        let outside = temp_dir("outside");
        let f = outside.join("keep.png");
        std::fs::write(&f, png(1, 1)).unwrap();
        discard_in(&dir, f.to_str().unwrap());
        assert!(f.exists());
        discard_in(&dir, &a.path);
        assert!(!Path::new(&a.path).exists());
        assert!(load(&a).is_none());
        let _ = std::fs::remove_dir_all(dir);
        let _ = std::fs::remove_dir_all(outside);
    }

    #[test]
    fn rejects_non_images_and_big_files() {
        let dir = temp_dir("reject");
        let e = save_in(&dir, b"just text", None, false).unwrap_err();
        assert_eq!(e.to_string(), "not a PNG, JPEG, GIF or WebP image");
        let mut big = png(1, 1);
        big.resize(MAX_BYTES + 1, 0);
        assert!(save_in(&dir, &big, None, false).is_err());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn missing_file_adds_note() {
        let a = Attachment {
            path: "/nonexistent/x.png".into(),
            name: "x.png".into(),
            mime: "image/png".into(),
            width: 1,
            height: 1,
            pasted: false,
        };
        let (text, images) = images_for(&[a], "hi".into());
        assert!(images.is_empty());
        assert_eq!(text, "hi\n[image x.png is no longer on disk]");
    }
}
