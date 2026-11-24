//! harness-cli: drive chats, issues, runs and imports without the GUI.
//!
//! `main` only wires the real terminal and engine into [`app::run`].

mod app;
mod args;
mod render;
mod signal;

#[cfg(test)]
mod tests;

use std::io::BufReader;

fn main() {
    signal::install();
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let deps = app::Deps {
        open: Box::new(engine::Engine::open_configured),
        input: Box::new(BufReader::new(std::io::stdin())),
        out: Box::new(std::io::stdout()),
        err: Box::new(std::io::stderr()),
        cwd: std::env::current_dir().unwrap_or_else(|_| ".".into()),
        env: Box::new(|name| std::env::var(name).ok()),
    };
    std::process::exit(app::run(&argv, deps));
}
