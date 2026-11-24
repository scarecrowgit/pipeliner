use crate::runners::{Pattern, parse_tests};
use model::TestResult;

fn caps(pattern: &str, line: &str) -> Option<Vec<(String, String)>> {
    Pattern::new(pattern).expect("compiles").captures(line)
}

#[test]
fn the_small_regex_finds_named_groups() {
    let found = caps(r"(?P<passed>\d+)/(?<total>\d+) ok", "suite: 12/14 ok, 2 skipped").expect("match");
    assert_eq!(found, vec![("passed".to_string(), "12".to_string()), ("total".to_string(), "14".to_string())]);
    assert!(caps(r"^\d+$", "12a").is_none());
    assert!(caps(r"^(ab|cd)+e?$", "abcdab").is_some());
    assert!(caps(r"[^x]{3}", "xxaxx").is_none());
    assert!(caps(r"a.{2,}b", "a12b").is_some());
    assert!(caps(r"\s*Tests:\s+(?P<passed>\d+) passed", "  Tests:  4 passed").is_some());
    assert!(Pattern::new("(unclosed").is_err());
    assert!(Pattern::new("*").is_err());
}

#[test]
fn tests_output_is_summed_over_lines() {
    let out = "running\ntest result: ok. 5 passed; 0 failed; 1 ignored\ntest result: FAILED. 1 passed; 2 failed; 0 ignored\n";
    assert_eq!(parse_tests("cargo-test", out), Ok(Some(TestResult { passed: 6, total: 8 })));
    assert_eq!(parse_tests("cargo-test", "nothing"), Ok(None));
    assert_eq!(parse_tests("", out), Ok(None));
    assert_eq!(parse_tests(r"(?P<passed>\d+) of (?P<total>\d+)", "3 of 4\n1 of 1"), Ok(Some(TestResult { passed: 4, total: 5 })));
    assert!(parse_tests("(", "x").is_err());
}
