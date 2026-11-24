use std::cmp::Ordering;

use super::*;

#[test]
fn parses_keys() {
    assert_eq!(parse("HAR-12"), Some(("HAR".to_string(), 12)));
    assert_eq!(parse(" pay-1427 "), Some(("PAY".to_string(), 1427)));
    assert_eq!(parse("A-0"), Some(("A".to_string(), 0)));
}

#[test]
fn rejects_what_is_not_a_key() {
    for bad in ["", "HAR", "HAR-", "-12", "HAR-x", "HAR-1-2", "H4R-1", "HAR-+3", "HAR--3", "HAR-99999999999", "Я-1"] {
        assert_eq!(parse(bad), None, "{bad}");
    }
}

#[test]
fn natural_order() {
    assert_eq!(compare("HAR-9", "HAR-10"), Ordering::Less);
    assert_eq!(compare("HAR-10", "HAR-9"), Ordering::Greater);
    assert_eq!(compare("HAR-10", "HAR-10"), Ordering::Equal);
    assert_eq!(compare("HAR-99", "PAY-1"), Ordering::Less);
    let mut keys = vec!["HAR-10", "junk", "HAR-2", "PAY-1", "HAR-1", "abc"];
    keys.sort_by(|a, b| compare(a, b));
    assert_eq!(keys, ["HAR-1", "HAR-2", "HAR-10", "PAY-1", "abc", "junk"]);
}

#[test]
fn next_number_follows_the_highest() {
    assert_eq!(next_number("HAR", ["HAR-1", "HAR-12", "PAY-40", "x"]), 13);
    assert_eq!(next_number("har", ["HAR-3"]), 4);
    assert_eq!(next_number("NEW", ["HAR-3"]), 1);
    assert_eq!(format("HAR", 7), "HAR-7");
}

#[test]
fn prefixes() {
    assert_eq!(prefix_for("Payments"), "PAY");
    assert_eq!(prefix_for("Harness"), "HAR");
    assert_eq!(prefix_for("pipeliner"), "PIP");
    assert_eq!(prefix_for("Checkout web app"), "CWA");
    assert_eq!(prefix_for("a b c d e f"), "ABCD");
    assert_eq!(prefix_for("checkout-web"), "CW");
    assert_eq!(prefix_for("Harness 2"), "HAR");
    assert_eq!(prefix_for("Go"), "GO");
}

#[test]
fn prefixes_are_never_empty() {
    assert_eq!(prefix_for(""), "PRJ");
    assert_eq!(prefix_for("   "), "PRJ");
    assert_eq!(prefix_for("12345"), "PRJ");
    assert_eq!(prefix_for("Платежи"), "PRJ");
    assert_eq!(prefix_for("X"), "XX");
    assert_eq!(prefix_for("Q 7"), "QX");
    for name in ["", "é", "日本", "A", "a-b", "Payments API gateway v2"] {
        let p = prefix_for(name);
        assert!((2..=4).contains(&p.len()), "{name}: {p}");
        assert!(p.chars().all(|c| c.is_ascii_uppercase()));
        assert!(parse(&format(&p, 1)).is_some());
    }
}

#[test]
fn typed_prefixes_are_letters_and_unique() {
    assert_eq!(validate_prefix(" har ", ["PAY"]), Ok("HAR".to_string()));
    assert_eq!(validate_prefix("", ["PAY"]), Err(PrefixProblem::Empty));
    assert_eq!(validate_prefix("HA-1", ["PAY"]), Err(PrefixProblem::NotLetters));
    assert_eq!(validate_prefix("ÄB", ["PAY"]), Err(PrefixProblem::NotLetters));
    assert_eq!(validate_prefix("ABCDEFGHI", ["PAY"]), Err(PrefixProblem::NotLetters));
    assert_eq!(validate_prefix("pay", ["PAY"]), Err(PrefixProblem::Taken));
}
