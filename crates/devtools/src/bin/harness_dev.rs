//! `harness-dev`: drive a Harness window, screenshot it, render mockups and compare them.
//! Run inside the Nix shell (GPUI needs its `LD_LIBRARY_PATH`):
//! `nix-shell shell.nix --run 'cargo run -p devtools --bin harness-dev -- <command>' </dev/null`.

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, anyhow, bail};
use devtools::driver::{Driver, launch};
use devtools::script::{self, Step};
use devtools::{compare, mockup, niri};
use ui::dev::proto;

const USAGE: &str = "\
usage:
  harness-dev mockup <name|file.html> --out <png>
  harness-dev shot --pid <pid> --out <png> [--width <px>]
  harness-dev compare <mockup.png> <window.png> --out <prefix>
  harness-dev run <binary> [--script <file>] [--shot <png>] [--width <px>] [-- <app args>…]
  harness-dev drive --socket <path> <command…>

drive commands (a target is a probe name or `x y`): windows, probes, click <target>,
  dblclick <target>, hover <target>, scroll <target> <dx> <dy>, drag <target> <target>,
  key <keystroke…>, type <text>, text <probe>, action <name>, wait, quit
scripts: one drive command per line, plus `shot <png>` and `sleep <ms>`; `#` comments";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Err(err) = run_cli(&args) {
        eprintln!("harness-dev: {err:#}");
        std::process::exit(1);
    }
}

/// Command-line arguments split into positionals, `--flag value` options and what follows `--`.
struct Args {
    positional: Vec<String>,
    options: Vec<(String, String)>,
    rest: Vec<String>,
}

impl Args {
    fn parse(args: &[String]) -> anyhow::Result<Args> {
        let mut out = Args { positional: Vec::new(), options: Vec::new(), rest: Vec::new() };
        let mut it = args.iter();
        while let Some(arg) = it.next() {
            if arg == "--" {
                out.rest = it.cloned().collect();
                break;
            }
            if let Some(name) = arg.strip_prefix("--") {
                let value = it.next().ok_or_else(|| anyhow!("--{name} needs a value"))?;
                out.options.push((name.to_string(), value.clone()));
            } else {
                out.positional.push(arg.clone());
            }
        }
        Ok(out)
    }

    fn get(&self, name: &str) -> Option<&str> {
        self.options.iter().rev().find(|(n, _)| n == name).map(|(_, v)| v.as_str())
    }

    fn need(&self, name: &str) -> anyhow::Result<&str> {
        self.get(name).ok_or_else(|| anyhow!("--{name} is required\n{USAGE}"))
    }

    fn number<T: std::str::FromStr>(&self, name: &str) -> anyhow::Result<Option<T>> {
        self.get(name)
            .map(|v| v.parse().map_err(|_| anyhow!("--{name}: not a number: {v}")))
            .transpose()
    }
}

fn run_cli(args: &[String]) -> anyhow::Result<()> {
    let Some((cmd, rest)) = args.split_first() else {
        println!("{USAGE}");
        return Ok(());
    };
    if cmd == "drive" {
        return drive(rest);
    }
    let a = Args::parse(rest)?;
    match cmd.as_str() {
        "mockup" => {
            let [name] = a.positional.as_slice() else { bail!("mockup takes one name\n{USAGE}") };
            let html = mockup::resolve(name);
            let out = PathBuf::from(a.need("out")?);
            let (w, h) = mockup::render(&html, &out)?;
            println!("{} ({w}x{h})", out.display());
        }
        "shot" => {
            let pid: u32 = a.number("pid")?.ok_or_else(|| anyhow!("--pid is required"))?;
            let window = niri::window_for_pid(pid, Duration::from_secs(5))?;
            if let Some(width) = a.number::<u32>("width")? {
                niri::set_width(window.id, width)?;
                std::thread::sleep(Duration::from_millis(400));
            }
            let out = PathBuf::from(a.need("out")?);
            niri::screenshot(window.id, &out)?;
            println!("{}", out.display());
        }
        "compare" => {
            let [left, right] = a.positional.as_slice() else {
                bail!("compare takes two PNGs\n{USAGE}")
            };
            let prefix = PathBuf::from(a.need("out")?);
            let (sim, side, heat) = compare::compare_files(left.as_ref(), right.as_ref(), &prefix)?;
            println!("{}\n{}", side.display(), heat.display());
            println!(
                "similarity {:.3} (pixels within {} of each other), mean difference {:.1}/255 over {}x{}",
                sim.score,
                compare::TOLERANCE,
                sim.mean_diff,
                sim.width,
                sim.height
            );
        }
        "run" => run_app(&a)?,
        "help" | "--help" | "-h" => println!("{USAGE}"),
        other => bail!("unknown command {other}\n{USAGE}"),
    }
    Ok(())
}

/// `drive --socket <path> <command…>`: the words after the socket, re-quoted, are one line.
fn drive(rest: &[String]) -> anyhow::Result<()> {
    let [flag, socket, words @ ..] = rest else { bail!("drive --socket <path> <command…>") };
    if flag != "--socket" || words.is_empty() {
        bail!("drive --socket <path> <command…>");
    }
    let line: Vec<String> = words.iter().map(|w| proto::quote(w)).collect();
    let mut driver = Driver::connect(Path::new(socket))?;
    println!("{}", driver.send_line(&line.join(" "))?);
    Ok(())
}

fn run_app(a: &Args) -> anyhow::Result<()> {
    let [binary] = a.positional.as_slice() else { bail!("run takes one binary\n{USAGE}") };
    let steps = match a.get("script") {
        Some(file) => {
            let text = std::fs::read_to_string(file).with_context(|| format!("read {file}"))?;
            script::parse(&text)?
        }
        None => Vec::new(),
    };
    let width = a.number::<u32>("width")?;
    let final_shot = a.get("shot").map(PathBuf::from);
    let needs_niri = width.is_some()
        || final_shot.is_some()
        || steps.iter().any(|s| matches!(s, Step::Shot(_)));

    let mut app = launch(Path::new(binary), &a.rest, &[])?;
    eprintln!("launched pid {} (socket {})", app.pid, app.socket.display());
    let result = (|| -> anyhow::Result<()> {
        let window = if needs_niri {
            Some(niri::window_for_pid(app.pid, Duration::from_secs(10))?.id)
        } else {
            None
        };
        if let (Some(id), Some(width)) = (window, width) {
            niri::set_width(id, width)?;
            std::thread::sleep(Duration::from_millis(400));
            app.driver.wait()?;
        }
        let shot = |app: &mut devtools::driver::Running, path: &Path| -> anyhow::Result<()> {
            let id = window.ok_or_else(|| anyhow!("no niri window"))?;
            app.driver.wait()?;
            niri::screenshot(id, path)?;
            println!("shot {}", path.display());
            Ok(())
        };
        for step in &steps {
            match step {
                Step::Command(command) => {
                    let line = command.to_line();
                    let reply = app.driver.send_line(&line)?;
                    println!("> {line}\n{reply}");
                }
                Step::Shot(path) => shot(&mut app, path)?,
                Step::Sleep(d) => std::thread::sleep(*d),
            }
        }
        if let Some(path) = &final_shot {
            shot(&mut app, path)?;
        }
        Ok(())
    })();
    if let Err(err) = result {
        let log = app.log_tail();
        app.stop();
        if log.is_empty() {
            return Err(err);
        }
        bail!("{err:#}\napp output:\n{log}");
    }
    app.stop();
    Ok(())
}
