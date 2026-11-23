//! Reading shell commands for permission rules.
//!
//! The lexer models a subset of `sh` exactly: blanks, single quotes, double quotes with their
//! backslash rules, backslash escapes, operators, `$(…)`, backticks and `${…}`. Everything
//! else it does not model exactly (`$'…'`, `$"…"`, backslash-newline, brace expansion, `#`
//! comments, control characters, unbalanced quotes or parentheses) marks the command as
//! *exotic*. An Allow prefix matches only a single simple command with nothing exotic in it;
//! Deny and Ask look into every part, wrapper and nested command, and treat anything they
//! cannot decode as a match.

/// One word as the shell would pass it, quotes removed.
#[derive(Clone, Debug, Default)]
pub(crate) struct Word {
    pub text: String,
    /// Quotes or backslashes were used in it.
    pub quoted: bool,
    /// It holds `$…` or a command substitution, so the shell changes it.
    pub subst: bool,
    /// It holds unquoted glob or tilde characters, so the shell may change it.
    pub glob: bool,
}

/// A command split at every operator.
#[derive(Debug, Default)]
pub(crate) struct Lexed {
    pub segments: Vec<Vec<Word>>,
    /// It chains, pipes, redirects, groups, runs in the background or substitutes.
    pub compound: bool,
    /// It uses syntax the lexer does not model exactly.
    pub exotic: bool,
    /// Texts of `$(…)`, backticks and `${…}`, to be read as commands in turn.
    pub inner: Vec<String>,
}

/// Splits `command` into simple commands of words.
pub(crate) fn lex(command: &str) -> Lexed {
    let mut lexer = Lexer {
        chars: command.chars().collect(),
        i: 0,
        word: None,
        out: Lexed { segments: vec![Vec::new()], ..Lexed::default() },
    };
    // ANSI-C and locale quoting, and line continuations, change how the rest is read.
    if command.contains("$'") || command.contains("$\"") || command.contains("\\\n") {
        lexer.out.exotic = true;
    }
    lexer.run();
    lexer.out.segments.retain(|s| !s.is_empty());
    lexer.out
}

struct Lexer {
    chars: Vec<char>,
    i: usize,
    word: Option<Word>,
    out: Lexed,
}

impl Lexer {
    fn next(&mut self) -> Option<char> {
        let c = self.chars.get(self.i).copied();
        self.i += 1;
        c
    }

    fn peek(&self) -> Option<char> {
        self.chars.get(self.i).copied()
    }

    fn word(&mut self) -> &mut Word {
        self.word.get_or_insert_with(Word::default)
    }

    fn end_word(&mut self) {
        if let Some(word) = self.word.take() {
            match self.out.segments.last_mut() {
                Some(seg) => seg.push(word),
                None => self.out.segments.push(vec![word]),
            }
        }
    }

    fn split(&mut self) {
        self.end_word();
        self.out.compound = true;
        self.out.segments.push(Vec::new());
    }

    fn run(&mut self) {
        while let Some(c) = self.next() {
            let at_start = self.word.is_none();
            match c {
                ' ' | '\t' => self.end_word(),
                '\n' | ';' | '&' | '|' | '<' | '>' | '(' | ')' => self.split(),
                c if c.is_control() || c.is_whitespace() => {
                    self.out.exotic = true;
                    self.end_word();
                }
                '\\' => match self.next() {
                    None | Some('\n') => self.out.exotic = true,
                    Some(n) => {
                        let w = self.word();
                        w.text.push(n);
                        w.quoted = true;
                    }
                },
                '\'' => self.single_quoted(),
                '"' => self.double_quoted(),
                '$' => self.dollar(),
                '`' => self.backtick(),
                '{' | '}' if at_start && self.peek().is_none_or(|n| matches!(n, ' ' | '\t' | '\n' | ';')) => {
                    self.split()
                }
                '{' | '}' => {
                    // Brace expansion can turn one word into several, even into a command.
                    self.out.exotic = true;
                    let w = self.word();
                    w.text.push(c);
                    w.glob = true;
                }
                '#' if at_start => {
                    self.out.exotic = true;
                    self.word().text.push(c);
                }
                '*' | '?' | '[' => {
                    let w = self.word();
                    w.text.push(c);
                    w.glob = true;
                }
                '~' if at_start => {
                    let w = self.word();
                    w.text.push(c);
                    w.glob = true;
                }
                c => self.word().text.push(c),
            }
        }
        self.end_word();
    }

    fn single_quoted(&mut self) {
        self.word().quoted = true;
        loop {
            match self.next() {
                None => {
                    self.out.exotic = true;
                    return;
                }
                Some('\'') => return,
                Some(c) => self.word().text.push(c),
            }
        }
    }

    fn double_quoted(&mut self) {
        self.word().quoted = true;
        loop {
            match self.next() {
                None => {
                    self.out.exotic = true;
                    return;
                }
                Some('"') => return,
                Some('\\') => match self.next() {
                    None | Some('\n') => self.out.exotic = true,
                    Some(n @ ('$' | '`' | '"' | '\\')) => self.word().text.push(n),
                    Some(n) => {
                        let w = self.word();
                        w.text.push('\\');
                        w.text.push(n);
                    }
                },
                Some('$') => self.dollar(),
                Some('`') => self.backtick(),
                Some(c) => self.word().text.push(c),
            }
        }
    }

    /// After a `$`: a substitution, a parameter, or a plain dollar sign.
    fn dollar(&mut self) {
        match self.peek() {
            Some('(') => {
                self.i += 1;
                let inner = self.until_close(')');
                self.out.inner.push(inner);
                self.out.compound = true;
                self.word().subst = true;
            }
            Some('{') => {
                self.i += 1;
                let inner = self.until_close('}');
                // `${x:-$(cmd)}` runs `cmd`.
                if inner.contains("$(") || inner.contains('`') {
                    self.out.compound = true;
                }
                self.out.inner.push(inner);
                self.word().subst = true;
            }
            _ => {
                let w = self.word();
                w.text.push('$');
                w.subst = true;
            }
        }
    }

    fn backtick(&mut self) {
        let mut inner = String::new();
        loop {
            match self.next() {
                None => {
                    self.out.exotic = true;
                    break;
                }
                Some('`') => break,
                Some('\\') => inner.extend(self.next()),
                Some(c) => inner.push(c),
            }
        }
        self.out.inner.push(inner);
        self.out.compound = true;
        self.word().subst = true;
    }

    /// The text up to the bracket that closes the one just read, skipping quoted parts.
    fn until_close(&mut self, close: char) -> String {
        let open = if close == ')' { '(' } else { '{' };
        let mut depth = 1usize;
        let mut inner = String::new();
        while let Some(c) = self.next() {
            match c {
                '\\' => {
                    inner.push(c);
                    inner.extend(self.next());
                    continue;
                }
                '\'' | '"' => {
                    inner.push(c);
                    while let Some(q) = self.next() {
                        inner.push(q);
                        if q == c {
                            break;
                        }
                        if q == '\\' && c == '"' {
                            inner.extend(self.next());
                        }
                    }
                    continue;
                }
                c if c == open => depth += 1,
                c if c == close => {
                    depth -= 1;
                    if depth == 0 {
                        return inner;
                    }
                }
                _ => {}
            }
            inner.push(c);
        }
        self.out.exotic = true;
        inner
    }
}

/// Shells, interpreters and wrappers that run a command or code given as their arguments.
const WRAPPERS: &[&str] = &[
    "sh", "bash", "zsh", "fish", "dash", "ksh", "csh", "tcsh", "perl", "ruby", "env", "sudo", "doas", "su",
    "xargs", "find", "eval", "exec", "command", "builtin", "nohup", "time", "timeout", "watch", "ssh",
    "nix-shell", "nice", "ionice", "stdbuf", "setsid", "chroot", "script", "parallel", "flock", "busybox",
    ".", "source", "trap", "coproc", "enable",
];
/// Commands whose damage a prefix rule should never approve in advance.
const DESTRUCTIVE: &[&str] = &["rm", "dd", "chmod", "chown", "kill", "pkill", "killall", "shred"];
/// Interpreters that are fine to allow for scripts but not with code on the command line.
const INTERPRETERS: &[&str] = &["node", "deno", "bun", "bunx", "npx"];
/// Shells read a script from the argument after one of these.
const SHELLS: &[&str] = &["sh", "bash", "zsh", "fish", "dash", "ksh", "csh", "tcsh", "su", "script"];
/// Words that start a command without being it.
const KEYWORDS: &[&str] = &["!", "{", "}", "if", "then", "else", "elif", "fi", "do", "done", "while", "until"];

pub(crate) fn basename(word: &str) -> &str {
    match word.rsplit('/').next() {
        Some(name) if !name.is_empty() => name,
        _ => word,
    }
}

fn is_interpreter(name: &str) -> bool {
    name.starts_with("python") || INTERPRETERS.contains(&name)
}

fn is_wrapper(name: &str) -> bool {
    WRAPPERS.contains(&name)
}

/// `-c`, `-e`, `--eval` and friends among the leading flags, or `deno eval`.
fn has_code_flag(args: &[Word]) -> bool {
    for arg in args {
        let t = arg.text.as_str();
        if matches!(t, "--eval" | "--print" | "--call" | "--command") {
            return true;
        }
        if let Some(short) = t.strip_prefix('-') {
            if !short.starts_with('-') && short.contains(['c', 'e', 'p']) {
                return true;
            }
            continue;
        }
        return t == "eval";
    }
    false
}

/// Commands an Allow prefix never covers and "Always allow" is never offered for.
fn allow_blocked(words: &[Word]) -> bool {
    let Some(first) = words.first() else { return true };
    let name = basename(&first.text);
    let args = &words[1..];
    let sub = |set: &[&str]| args.iter().any(|a| set.contains(&a.text.as_str()));
    is_wrapper(name)
        || DESTRUCTIVE.contains(&name)
        || name.starts_with("mkfs")
        || (name == "nix" && sub(&["develop", "run", "shell"]))
        || (matches!(name, "docker" | "podman") && sub(&["run", "exec"]))
        || (is_interpreter(name) && has_code_flag(args))
}

pub(crate) fn is_assignment(word: &str) -> bool {
    let Some((name, _)) = word.split_once('=') else { return false };
    let name = name.strip_suffix('+').unwrap_or(name);
    let name = match name.split_once('[') {
        Some((base, index)) if index.ends_with(']') => base,
        Some(_) => return false,
        None => name,
    };
    let mut chars = name.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Paths, numbers and assignments: the part of a command that changes from call to call.
fn looks_like_value(word: &str) -> bool {
    word.starts_with('.')
        || word.contains(['/', '=', ':', '@', '%', '#'])
        || word.chars().all(|c| c.is_ascii_digit() || c == '.' || c == ',')
}

/// The "Always allow" prefix of a command: its words up to the first quoted, expanded,
/// path-like or value argument, flags included. `None` for anything [`allow_matches`] would
/// not approve, and for a bare interpreter.
pub(crate) fn command_prefix(command: &str) -> Option<String> {
    let lexed = lex(command);
    if lexed.compound || lexed.exotic || lexed.segments.len() != 1 {
        return None;
    }
    let words = &lexed.segments[0];
    let first = words.first()?;
    if first.quoted || first.subst || first.glob || first.text.is_empty() || is_assignment(&first.text) {
        return None;
    }
    if allow_blocked(words) {
        return None;
    }
    let mut keep = vec![first.text.as_str()];
    for w in &words[1..] {
        if w.quoted || w.subst || w.glob || looks_like_value(&w.text) {
            break;
        }
        keep.push(&w.text);
    }
    if is_interpreter(basename(&first.text)) && keep.len() == 1 {
        return None;
    }
    Some(keep.join(" "))
}

/// Whether an Allow rule with this non-empty prefix covers `command`: one simple command,
/// nothing exotic, not a wrapper, and its leading words are the prefix exactly.
pub(crate) fn allow_matches(pattern: &str, command: &str) -> bool {
    let want: Vec<&str> = pattern.split_whitespace().collect();
    let lexed = lex(command);
    if lexed.compound || lexed.exotic || lexed.segments.len() != 1 {
        return false;
    }
    let words = &lexed.segments[0];
    if words.len() < want.len() || allow_blocked(words) {
        return false;
    }
    words.iter().zip(&want).all(|(w, p)| !w.subst && !w.glob && w.text == *p)
}

/// Whether a Deny or Ask rule with this non-empty prefix applies to `command`: some command
/// in it, at any depth, starts with the prefix; or the command cannot be decoded; or it hands
/// code to something opaque and the prefix's first word appears anywhere in it.
pub(crate) fn restrictive_matches(pattern: &str, command: &str) -> bool {
    let mut want: Vec<String> = pattern.split_whitespace().map(str::to_string).collect();
    let Some(head) = want.first_mut() else { return true };
    *head = basename(head).to_string();
    let mut scan = Scan::default();
    scan.text(command, 0);
    if scan.undecodable {
        return true;
    }
    if scan.commands.iter().any(|c| c.len() >= want.len() && c.iter().zip(&want).all(|(a, b)| a == b)) {
        return true;
    }
    scan.opaque
        && command
            .split(|c: char| !(c.is_alphanumeric() || matches!(c, '_' | '-' | '.' | '/' | '+')))
            .any(|w| basename(w) == want[0])
}

/// Deeper than this, a command is treated as undecodable.
const MAX_DEPTH: usize = 8;
/// Arguments of a wrapper looked at as possible commands.
const MAX_SUFFIXES: usize = 64;

/// Every command a shell line may run, as far as it can be read.
#[derive(Default)]
struct Scan {
    /// Word lists, the first word without its path.
    commands: Vec<Vec<String>>,
    /// Something could hide any command: exotic syntax, an expanded command word.
    undecodable: bool,
    /// Code is handed to something not read here (ssh, docker, `python -c`).
    opaque: bool,
}

impl Scan {
    fn text(&mut self, text: &str, depth: usize) {
        if depth > MAX_DEPTH {
            self.undecodable = true;
            return;
        }
        let lexed = lex(text);
        self.undecodable |= lexed.exotic;
        for inner in &lexed.inner {
            self.text(inner, depth + 1);
        }
        for seg in &lexed.segments {
            self.simple(seg, depth);
        }
    }

    fn simple(&mut self, words: &[Word], depth: usize) {
        if depth > MAX_DEPTH {
            self.undecodable = true;
            return;
        }
        let skip = |w: &Word| is_assignment(&w.text) || (!w.quoted && KEYWORDS.contains(&w.text.as_str()));
        let Some(start) = words.iter().position(|w| !skip(w)) else { return };
        let words = &words[start..];
        let first = &words[0];
        if first.subst || first.glob {
            self.undecodable = true;
            return;
        }
        let name = basename(&first.text).to_string();
        let args = &words[1..];
        self.push(&name, args);
        if is_interpreter(&name) && has_code_flag(args) {
            self.opaque = true;
        }
        if is_wrapper(&name) || matches!(name.as_str(), "nix" | "docker" | "podman") {
            self.unwrap(&name, args, depth);
        }
    }

    fn push(&mut self, name: &str, args: &[Word]) {
        let mut cmd = vec![name.to_string()];
        cmd.extend(args.iter().map(|w| w.text.clone()));
        self.commands.push(cmd);
    }

    /// Looks for the command a wrapper runs: any of its arguments may start it.
    fn unwrap(&mut self, name: &str, args: &[Word], depth: usize) {
        let code_after = |flags: &[&str]| args.iter().position(|a| flags.contains(&a.text.as_str())).and_then(|i| args.get(i + 1));
        if SHELLS.contains(&name) {
            let flag = args.iter().position(|a| {
                a.text == "--command" || a.text.strip_prefix('-').is_some_and(|s| !s.starts_with('-') && s.contains('c'))
            });
            if let Some(code) = flag.and_then(|i| args.get(i + 1)) {
                self.text(&code.text, depth + 1);
            }
        }
        match name {
            "eval" | "watch" | "trap" => {
                let joined = args.iter().map(|a| a.text.as_str()).collect::<Vec<_>>().join(" ");
                self.text(&joined, depth + 1);
                for arg in args {
                    self.text(&arg.text, depth + 1);
                }
            }
            "nix-shell" => {
                if let Some(code) = code_after(&["--run", "--command"]) {
                    self.text(&code.text, depth + 1);
                }
            }
            "perl" | "ruby" | "ssh" | "docker" | "podman" | "parallel" | "su" | "script" => self.opaque = true,
            _ => {}
        }
        if args.iter().any(|a| a.subst) {
            self.undecodable = true;
        }
        for (k, arg) in args.iter().enumerate().take(MAX_SUFFIXES) {
            self.push(basename(&arg.text), &args[k + 1..]);
        }
        let nested = args.iter().position(|a| is_wrapper(basename(&a.text)) || is_interpreter(basename(&a.text)));
        if let Some(k) = nested {
            self.simple(&args[k..], depth + 1);
        }
    }
}
