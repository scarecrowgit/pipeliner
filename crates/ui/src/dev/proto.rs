//! The line protocol shared by the app and the `devtools` client.

use std::time::Duration;

/// Where a pointer command lands: a probe's visible centre or a window point.
#[derive(Clone, Debug, PartialEq)]
pub enum Target {
    Probe(String),
    Point(f32, f32),
}

/// One driver command.
#[derive(Clone, Debug, PartialEq)]
pub enum Command {
    Windows,
    Probes,
    Click(Target),
    DblClick(Target),
    Hover(Target),
    Scroll(Target, f32, f32),
    Drag(Target, Target),
    Key(Vec<String>),
    Type(String),
    Text(String),
    Action(String),
    Wait,
    Quit,
}

impl Command {
    /// Parse one command line.
    pub fn parse(line: &str) -> Result<Command, String> {
        let words = split(line)?;
        let (verb, args) = words.split_first().ok_or("empty command")?;
        let one = |what: &str| match args {
            [arg] => Ok(arg.clone()),
            _ => Err(format!("{verb} takes one {what}")),
        };
        Ok(match verb.as_str() {
            "windows" if args.is_empty() => Command::Windows,
            "probes" if args.is_empty() => Command::Probes,
            "wait" if args.is_empty() => Command::Wait,
            "quit" if args.is_empty() => Command::Quit,
            "click" => Command::Click(target(args)?),
            "dblclick" => Command::DblClick(target(args)?),
            "hover" => Command::Hover(target(args)?),
            "scroll" if args.len() >= 3 => {
                let n = args.len();
                let (dx, dy) = (number(&args[n - 2])?, number(&args[n - 1])?);
                Command::Scroll(target(&args[..n - 2])?, dx, dy)
            }
            "drag" => {
                let (from, to) = two_targets(args)?;
                Command::Drag(from, to)
            }
            "key" if !args.is_empty() => Command::Key(args.to_vec()),
            "type" => Command::Type(one("text (quote it)")?),
            "text" => Command::Text(one("probe name")?),
            "action" => Command::Action(one("action name")?),
            _ => return Err(format!("unknown command or wrong arguments: {line}")),
        })
    }

    /// The command as one protocol line (without the newline).
    pub fn to_line(&self) -> String {
        let t = |t: &Target| match t {
            Target::Probe(name) => quote(name),
            Target::Point(x, y) => format!("{x} {y}"),
        };
        match self {
            Command::Windows => "windows".into(),
            Command::Probes => "probes".into(),
            Command::Wait => "wait".into(),
            Command::Quit => "quit".into(),
            Command::Click(target) => format!("click {}", t(target)),
            Command::DblClick(target) => format!("dblclick {}", t(target)),
            Command::Hover(target) => format!("hover {}", t(target)),
            Command::Scroll(target, dx, dy) => format!("scroll {} {dx} {dy}", t(target)),
            Command::Drag(from, to) => format!("drag {} {}", t(from), t(to)),
            Command::Key(keys) => {
                let keys: Vec<String> = keys.iter().map(|k| quote(k)).collect();
                format!("key {}", keys.join(" "))
            }
            Command::Type(text) => format!("type {}", quote(text)),
            Command::Text(name) => format!("text {}", quote(name)),
            Command::Action(name) => format!("action {}", quote(name)),
        }
    }

    /// How long the socket waits for the UI thread before answering `err`.
    pub fn timeout(&self) -> Duration {
        match self {
            Command::Drag(..) => Duration::from_secs(20),
            _ => Duration::from_secs(10),
        }
    }
}

fn target(args: &[String]) -> Result<Target, String> {
    match args {
        [name] => Ok(Target::Probe(name.clone())),
        [x, y] => Ok(Target::Point(number(x)?, number(y)?)),
        _ => Err("expected a probe name or x y".into()),
    }
}

/// `from to` where each is a probe name or an `x y` pair.
fn two_targets(args: &[String]) -> Result<(Target, Target), String> {
    let is_num = |s: &String| s.parse::<f32>().is_ok();
    let split_at = match args {
        [_, _] => 1,
        [a, b, _] if is_num(a) && is_num(b) => 2,
        [_, _, _] => 1,
        [_, _, _, _] => 2,
        _ => return Err("drag takes two targets: probe names or x y pairs".into()),
    };
    Ok((target(&args[..split_at])?, target(&args[split_at..])?))
}

fn number(s: &str) -> Result<f32, String> {
    s.parse::<f32>().map_err(|_| format!("not a number: {s}"))
}

/// Split a line into words; `"…"` groups words and takes `\" \\ \n \t \r` escapes.
pub fn split(line: &str) -> Result<Vec<String>, String> {
    let mut words = Vec::new();
    let mut chars = line.chars().peekable();
    loop {
        while chars.next_if(|c| c.is_whitespace()).is_some() {}
        let Some(&first) = chars.peek() else { break };
        let mut word = String::new();
        if first == '"' {
            chars.next();
            loop {
                match chars.next() {
                    None => return Err("unterminated quote".into()),
                    Some('"') => break,
                    Some('\\') => word.push(match chars.next() {
                        Some('n') => '\n',
                        Some('t') => '\t',
                        Some('r') => '\r',
                        Some(c @ ('"' | '\\')) => c,
                        Some(c) => return Err(format!("unknown escape \\{c}")),
                        None => return Err("unterminated quote".into()),
                    }),
                    Some(c) => word.push(c),
                }
            }
        } else {
            while let Some(c) = chars.next_if(|c| !c.is_whitespace()) {
                word.push(c);
            }
        }
        words.push(word);
    }
    Ok(words)
}

/// A word as [`split`] reads it back: bare when safe, else quoted and escaped.
pub fn quote(word: &str) -> String {
    let bare =
        !word.is_empty() && !word.chars().any(|c| c.is_whitespace() || c == '"' || c == '\\');
    if bare {
        return word.to_string();
    }
    let mut out = String::from('"');
    for c in word.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// `s` as a JSON string literal.
pub fn json_string(s: &str) -> String {
    let mut out = String::from('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// The reply line for a result: `ok <json>` or `err <message on one line>`.
pub fn format_reply(reply: &Result<String, String>) -> String {
    match reply {
        Ok(payload) => format!("ok {}", payload.replace(['\n', '\r'], " ")),
        Err(message) => format!("err {}", message.replace(['\n', '\r'], " ")),
    }
}

/// Read a reply line back into the payload or the error message.
pub fn parse_reply(line: &str) -> Result<String, String> {
    let line = line.trim_end_matches(['\n', '\r']);
    if line == "ok" {
        return Ok(String::new());
    }
    if let Some(payload) = line.strip_prefix("ok ") {
        return Ok(payload.to_string());
    }
    match line.strip_prefix("err") {
        Some(message) => Err(message.trim_start().to_string()),
        None => Err(format!("malformed reply: {line}")),
    }
}
