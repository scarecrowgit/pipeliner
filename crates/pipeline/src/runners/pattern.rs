//! Reading pass/fail counts from a tests command's output: `cargo-test`, or a regular
//! expression with `passed` and `total` (or `failed`) groups.
//!
//! The expression language is a small backtracking subset of the usual one, enough for test
//! summaries: literals, `.`, `\d \w \s` and their capitals, other escapes, `[a-z]` classes with
//! `^`, groups `( )`, `(?: )`, `(?P<name> )` and `(?<name> )`, `|`, `^ $`, and the
//! quantifiers `* + ? {n} {n,} {n,m}` (greedy). It is matched against each line.

use model::TestResult;

/// A compiled expression.
#[derive(Clone, Debug)]
pub struct Pattern {
    root: Node,
    names: Vec<Option<String>>,
}

#[derive(Clone, Debug)]
enum Node {
    Char(char),
    Any,
    Class { ranges: Vec<(char, char)>, negated: bool },
    Start,
    End,
    Group(Box<Node>, Option<usize>),
    Concat(Vec<Node>),
    Alt(Vec<Node>),
    Repeat { node: Box<Node>, min: usize, max: Option<usize> },
}

type Caps = Vec<Option<(usize, usize)>>;

/// Steps a match may take before it gives up, so a bad expression cannot hang a run.
const STEP_LIMIT: usize = 200_000;

impl Pattern {
    /// Compiles `text`, or says what is wrong with it.
    pub fn new(text: &str) -> Result<Pattern, String> {
        let chars: Vec<char> = text.chars().collect();
        let mut parser = Parser { chars: &chars, pos: 0, names: Vec::new() };
        let root = parser.alt()?;
        if parser.pos < chars.len() {
            return Err(format!("unexpected `{}` at {}", chars[parser.pos], parser.pos));
        }
        Ok(Pattern { root, names: parser.names })
    }

    /// The text of named groups at the first match in `line`.
    pub fn captures(&self, line: &str) -> Option<Vec<(String, String)>> {
        let input: Vec<char> = line.chars().collect();
        for start in 0..=input.len() {
            let mut caps: Caps = vec![None; self.names.len()];
            let m = Matcher { input: &input, steps: std::cell::Cell::new(0) };
            if m.node(&self.root, start, &mut caps, &mut |_, _| true) {
                let out = self
                    .names
                    .iter()
                    .zip(&caps)
                    .filter_map(|(name, cap)| {
                        let (name, (a, b)) = (name.as_ref()?, (*cap)?);
                        Some((name.clone(), input[a..b].iter().collect()))
                    })
                    .collect();
                return Some(out);
            }
            if m.steps.get() > STEP_LIMIT {
                return None;
            }
        }
        None
    }
}

struct Parser<'a> {
    chars: &'a [char],
    pos: usize,
    names: Vec<Option<String>>,
}

impl Parser<'_> {
    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }

    fn eat(&mut self, c: char) -> bool {
        if self.peek() == Some(c) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn alt(&mut self) -> Result<Node, String> {
        let mut options = vec![self.concat()?];
        while self.eat('|') {
            options.push(self.concat()?);
        }
        Ok(if options.len() == 1 { options.remove(0) } else { Node::Alt(options) })
    }

    fn concat(&mut self) -> Result<Node, String> {
        let mut items = Vec::new();
        while let Some(c) = self.peek() {
            if c == '|' || c == ')' {
                break;
            }
            let atom = self.atom()?;
            items.push(self.quantified(atom)?);
        }
        Ok(Node::Concat(items))
    }

    fn quantified(&mut self, atom: Node) -> Result<Node, String> {
        let (min, max) = match self.peek() {
            Some('*') => (0, None),
            Some('+') => (1, None),
            Some('?') => (0, Some(1)),
            Some('{') => {
                let start = self.pos;
                self.pos += 1;
                let min = self.number();
                let max = if self.eat(',') { if self.peek() == Some('}') { None } else { self.number() } } else { min };
                if !self.eat('}') || min.is_none() {
                    self.pos = start;
                    return Ok(atom);
                }
                self.pos -= 1;
                (min.unwrap_or(0), max)
            }
            _ => return Ok(atom),
        };
        self.pos += 1;
        // A lazy marker is accepted and treated as greedy.
        self.eat('?');
        Ok(Node::Repeat { node: Box::new(atom), min, max })
    }

    fn number(&mut self) -> Option<usize> {
        let start = self.pos;
        while self.peek().is_some_and(|c| c.is_ascii_digit()) {
            self.pos += 1;
        }
        self.chars[start..self.pos].iter().collect::<String>().parse().ok()
    }

    fn atom(&mut self) -> Result<Node, String> {
        let c = self.peek().ok_or("unexpected end")?;
        self.pos += 1;
        Ok(match c {
            '.' => Node::Any,
            '^' => Node::Start,
            '$' => Node::End,
            '(' => {
                let mut index = Some(self.names.len());
                if self.eat('?') {
                    if self.eat(':') {
                        index = None;
                    } else {
                        self.eat('P');
                        if !self.eat('<') {
                            return Err("unsupported group".into());
                        }
                        let start = self.pos;
                        while self.peek().is_some_and(|c| c != '>') {
                            self.pos += 1;
                        }
                        let name: String = self.chars[start..self.pos].iter().collect();
                        if !self.eat('>') || name.is_empty() {
                            return Err("bad group name".into());
                        }
                        self.names.push(Some(name));
                    }
                } else {
                    self.names.push(None);
                }
                let inner = self.alt()?;
                if !self.eat(')') {
                    return Err("missing `)`".into());
                }
                Node::Group(Box::new(inner), index)
            }
            '[' => self.class()?,
            '\\' => self.escape()?,
            '*' | '+' | '?' => return Err(format!("nothing to repeat before `{c}`")),
            ')' => return Err("unmatched `)`".into()),
            other => Node::Char(other),
        })
    }

    fn escape(&mut self) -> Result<Node, String> {
        let c = self.peek().ok_or("trailing `\\`")?;
        self.pos += 1;
        let class = |ranges: Vec<(char, char)>, negated| Node::Class { ranges, negated };
        let digit = vec![('0', '9')];
        let word = vec![('a', 'z'), ('A', 'Z'), ('0', '9'), ('_', '_')];
        let space = vec![(' ', ' '), ('\t', '\t'), ('\r', '\r'), ('\n', '\n')];
        Ok(match c {
            'd' => class(digit, false),
            'D' => class(digit, true),
            'w' => class(word, false),
            'W' => class(word, true),
            's' => class(space, false),
            'S' => class(space, true),
            't' => Node::Char('\t'),
            other => Node::Char(other),
        })
    }

    fn class(&mut self) -> Result<Node, String> {
        let negated = self.eat('^');
        let mut ranges = Vec::new();
        let mut first = true;
        loop {
            let c = self.peek().ok_or("missing `]`")?;
            self.pos += 1;
            if c == ']' && !first {
                break;
            }
            first = false;
            let lo = if c == '\\' {
                match self.escape()? {
                    Node::Char(ch) => ch,
                    Node::Class { ranges: more, .. } => {
                        ranges.extend(more);
                        continue;
                    }
                    _ => continue,
                }
            } else {
                c
            };
            if self.peek() == Some('-') && self.chars.get(self.pos + 1).is_some_and(|&n| n != ']') {
                self.pos += 1;
                let hi = self.peek().ok_or("missing `]`")?;
                self.pos += 1;
                ranges.push((lo, hi));
            } else {
                ranges.push((lo, lo));
            }
        }
        Ok(Node::Class { ranges, negated })
    }
}

struct Matcher<'a> {
    input: &'a [char],
    steps: std::cell::Cell<usize>,
}

impl Matcher<'_> {
    fn node(&self, node: &Node, pos: usize, caps: &mut Caps, k: &mut dyn FnMut(usize, &mut Caps) -> bool) -> bool {
        self.steps.set(self.steps.get() + 1);
        if self.steps.get() > STEP_LIMIT {
            return false;
        }
        let one = |ok: bool| ok.then_some(pos + 1);
        let at = self.input.get(pos).copied();
        match node {
            Node::Char(c) => one(at == Some(*c)).is_some_and(|next| k(next, caps)),
            Node::Any => one(at.is_some()).is_some_and(|next| k(next, caps)),
            Node::Class { ranges, negated } => {
                let ok = at.is_some_and(|ch| ranges.iter().any(|&(lo, hi)| lo <= ch && ch <= hi) != *negated);
                one(ok).is_some_and(|next| k(next, caps))
            }
            Node::Start => pos == 0 && k(pos, caps),
            Node::End => pos == self.input.len() && k(pos, caps),
            Node::Group(inner, index) => {
                let index = *index;
                self.node(inner, pos, caps, &mut |end, caps: &mut Caps| {
                    let before = index.and_then(|i| caps[i]);
                    if let Some(i) = index {
                        caps[i] = Some((pos, end));
                    }
                    if k(end, caps) {
                        return true;
                    }
                    if let Some(i) = index {
                        caps[i] = before;
                    }
                    false
                })
            }
            Node::Concat(items) => self.seq(items, pos, caps, k),
            Node::Alt(options) => options.iter().any(|option| self.node(option, pos, caps, k)),
            Node::Repeat { node, min, max } => self.repeat(node, *min, *max, 0, pos, caps, k),
        }
    }

    fn seq(&self, items: &[Node], pos: usize, caps: &mut Caps, k: &mut dyn FnMut(usize, &mut Caps) -> bool) -> bool {
        match items.split_first() {
            None => k(pos, caps),
            Some((first, rest)) => {
                self.node(first, pos, caps, &mut |next, caps: &mut Caps| self.seq(rest, next, caps, k))
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn repeat(
        &self,
        node: &Node,
        min: usize,
        max: Option<usize>,
        count: usize,
        pos: usize,
        caps: &mut Caps,
        k: &mut dyn FnMut(usize, &mut Caps) -> bool,
    ) -> bool {
        if max.is_none_or(|max| count < max) {
            // An empty step would repeat forever.
            let more = self.node(node, pos, caps, &mut |next, caps: &mut Caps| {
                next > pos && self.repeat(node, min, max, count + 1, next, caps, k)
            });
            if more {
                return true;
            }
        }
        count >= min && k(pos, caps)
    }
}

/// Counts in `output` read as `parse` says: `cargo-test`, or an expression with `passed` and
/// `total` (or `failed`) groups, summed over every line that matches. `Ok(None)` when nothing
/// in the output matched; `Err` when the expression does not compile.
pub fn parse_tests(parse: &str, output: &str) -> Result<Option<TestResult>, String> {
    let parse = parse.trim();
    if parse.is_empty() {
        return Ok(None);
    }
    if parse == "cargo-test" {
        return Ok(cargo_test(output));
    }
    let pattern = Pattern::new(parse).map_err(|e| format!("the tests pattern does not compile: {e}"))?;
    let (mut passed, mut total, mut any) = (0u32, 0u32, false);
    for line in output.lines() {
        let Some(caps) = pattern.captures(line) else { continue };
        let get = |name: &str| caps.iter().find(|(n, _)| n == name).and_then(|(_, v)| v.trim().parse::<u32>().ok());
        let Some(p) = get("passed") else { continue };
        let t = get("total").or_else(|| get("failed").map(|f| p + f)).unwrap_or(p);
        passed += p;
        total += t.max(p);
        any = true;
    }
    Ok(any.then_some(TestResult { passed, total }))
}

/// Sums the `test result:` lines of `cargo test`.
fn cargo_test(output: &str) -> Option<TestResult> {
    let mut found = None;
    for line in output.lines().filter(|l| l.contains("test result:")) {
        let count = |word: &str| -> u32 {
            line.split(';')
                .find_map(|part| part.trim().strip_suffix(word).and_then(|n| n.trim().rsplit(' ').next()?.parse().ok()))
                .unwrap_or(0)
        };
        let (passed, failed) = (count("passed"), count("failed"));
        let total: &mut TestResult = found.get_or_insert(TestResult { passed: 0, total: 0 });
        total.passed += passed;
        total.total += passed + failed;
    }
    found
}
