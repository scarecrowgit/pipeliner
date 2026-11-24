use super::*;

#[test]
fn tokens_tiers() {
    assert_eq!(tokens(0), "0");
    assert_eq!(tokens(1), "1");
    assert_eq!(tokens(950), "950");
    assert_eq!(tokens(999), "999");
    assert_eq!(tokens(1_000), "1k");
    assert_eq!(tokens(1_049), "1k");
    assert_eq!(tokens(1_050), "1.1k");
    assert_eq!(tokens(3_200), "3.2k");
    assert_eq!(tokens(48_200), "48.2k");
    assert_eq!(tokens(200_000), "200k");
    assert_eq!(tokens(999_949), "999.9k");
    assert_eq!(tokens(999_950), "1M");
    assert_eq!(tokens(1_000_000), "1M");
    assert_eq!(tokens(1_250_000), "1.3M");
    assert_eq!(tokens(12_340_000), "12.3M");
    assert_eq!(tokens(999_950_000), "1B");
    assert_eq!(tokens(2_500_000_000), "2.5B");
}

#[test]
fn tokens_huge_does_not_overflow() {
    assert!(tokens(u64::MAX).ends_with('B'));
    assert!(tokens(u64::MAX - 1).ends_with('B'));
}

#[test]
fn cost_tiers() {
    assert_eq!(cost(0.0), "$0.00");
    assert_eq!(cost(0.0001), "<$0.01");
    assert_eq!(cost(0.004), "<$0.01");
    assert_eq!(cost(0.0049), "<$0.01");
    assert_eq!(cost(0.005), "$0.01");
    assert_eq!(cost(0.01), "$0.01");
    assert_eq!(cost(0.21), "$0.21");
    assert_eq!(cost(1.0), "$1.00");
    assert_eq!(cost(612.4), "$612.40");
    assert_eq!(cost(999.999), "$1,000.00");
    assert_eq!(cost(1234.5), "$1,234.50");
    assert_eq!(cost(1_234_567.891), "$1,234,567.89");
}

#[test]
fn cost_odd_inputs_do_not_panic() {
    assert_eq!(cost(-3.2), "$0.00");
    assert_eq!(cost(-0.0), "$0.00");
    assert_eq!(cost(f64::NAN), "$0.00");
    assert_eq!(cost(f64::INFINITY), "$0.00");
    assert_eq!(cost(f64::NEG_INFINITY), "$0.00");
    assert!(cost(f64::MAX).starts_with('$'));
    assert!(cost(1e30).starts_with('$'));
}

#[test]
fn cost_short_axis_labels() {
    assert_eq!(cost_short(0.0), "$0");
    assert_eq!(cost_short(10.0), "$10");
    assert_eq!(cost_short(3.5), "$3.50");
    assert_eq!(cost_short(0.25), "$0.25");
    assert_eq!(cost_short(1000.0), "$1,000");
    assert_eq!(cost_short(1000.5), "$1,000.50");
    assert_eq!(cost_short(-1.0), "$0");
    assert_eq!(cost_short(f64::NAN), "$0");
    assert!(cost_short(f64::MAX).starts_with('$'));
}

#[test]
fn duration_tiers() {
    assert_eq!(duration(0), "0s");
    assert_eq!(duration(42), "42s");
    assert_eq!(duration(59), "59s");
    assert_eq!(duration(60), "1:00");
    assert_eq!(duration(102), "1:42");
    assert_eq!(duration(252), "4:12");
    assert_eq!(duration(3_599), "59:59");
    assert_eq!(duration(3_600), "1h 0m");
    assert_eq!(duration(3_700), "1h 1m");
    assert_eq!(duration(86_399), "23h 59m");
    assert_eq!(duration(86_400), "1d 0h");
    assert_eq!(duration(90_000), "1d 1h");
    assert!(duration(u64::MAX).contains('d'));
}

#[test]
fn duration_clock_tiers() {
    assert_eq!(duration_clock(0), "0:00");
    assert_eq!(duration_clock(7), "0:07");
    assert_eq!(duration_clock(30), "0:30");
    assert_eq!(duration_clock(60), "1:00");
    assert_eq!(duration_clock(90), "1:30");
    assert_eq!(duration_clock(3_599), "59:59");
    assert_eq!(duration_clock(3_600), "1:00:00");
    assert_eq!(duration_clock(3_727), "1:02:07");
    assert_eq!(duration_clock(90_187), "25:03:07");
    assert!(duration_clock(u64::MAX).contains(':'));
}

#[test]
fn duration_words_tiers() {
    assert_eq!(duration_words(0), "0s");
    assert_eq!(duration_words(6), "6s");
    assert_eq!(duration_words(60), "1m");
    assert_eq!(duration_words(252), "4m 12s");
    assert_eq!(duration_words(3_599), "59m 59s");
    assert_eq!(duration_words(3_600), "1h");
    assert_eq!(duration_words(3_700), "1h 1m");
    assert_eq!(duration_words(7_200), "2h");
    assert_eq!(duration_words(86_400), "1d");
    assert_eq!(duration_words(90_000), "1d 1h");
    assert!(duration_words(u64::MAX).contains('d'));
}

#[test]
fn count_separators() {
    assert_eq!(count(0), "0");
    assert_eq!(count(999), "999");
    assert_eq!(count(1_000), "1,000");
    assert_eq!(count(2_418), "2,418");
    assert_eq!(count(12_345), "12,345");
    assert_eq!(count(123_456), "123,456");
    assert_eq!(count(1_234_567), "1,234,567");
    assert_eq!(count(u64::MAX), "18,446,744,073,709,551,615");
}

#[test]
fn bytes_tiers() {
    assert_eq!(bytes(0), "0 B");
    assert_eq!(bytes(12), "12 B");
    assert_eq!(bytes(999), "999 B");
    assert_eq!(bytes(1_000), "1 KB");
    assert_eq!(bytes(1_500), "1.5 KB");
    assert_eq!(bytes(9_949), "9.9 KB");
    assert_eq!(bytes(12_450), "12 KB");
    assert_eq!(bytes(412_000), "412 KB");
    assert_eq!(bytes(999_499), "999 KB");
    assert_eq!(bytes(999_999), "1 MB");
    assert_eq!(bytes(1_100_000), "1.1 MB");
    assert_eq!(bytes(250_000_000), "250 MB");
    assert_eq!(bytes(3_400_000_000), "3.4 GB");
    assert_eq!(bytes(2_000_000_000_000), "2 TB");
    assert!(bytes(u64::MAX).ends_with(" TB"));
}

#[test]
fn percent_rounds_and_clamps() {
    assert_eq!(percent(0.0, 100.0), 0);
    assert_eq!(percent(1.0, 3.0), 33);
    assert_eq!(percent(2.0, 3.0), 67);
    assert_eq!(percent(50.0, 100.0), 50);
    assert_eq!(percent(100.0, 100.0), 100);
    assert_eq!(percent(250.0, 100.0), 100);
    assert_eq!(percent(-5.0, 100.0), 0);
}

#[test]
fn percent_odd_inputs() {
    assert_eq!(percent(5.0, 0.0), 0);
    assert_eq!(percent(5.0, -1.0), 0);
    assert_eq!(percent(f64::NAN, 10.0), 0);
    assert_eq!(percent(5.0, f64::NAN), 0);
    assert_eq!(percent(f64::INFINITY, 10.0), 100);
    assert_eq!(percent(f64::INFINITY, f64::INFINITY), 0);
    assert_eq!(percent(f64::MAX, f64::MIN_POSITIVE), 100);
}

#[test]
fn plural_picks_form() {
    assert_eq!(plural(0, "file", "files"), "0 files");
    assert_eq!(plural(1, "file", "files"), "1 file");
    assert_eq!(plural(3, "file", "files"), "3 files");
    assert_eq!(plural(2_418, "run", "runs"), "2,418 runs");
    assert_eq!(plural(2, "entry", "entries"), "2 entries");
}

#[test]
fn first_line_skips_blank() {
    assert_eq!(first_line(""), "");
    assert_eq!(first_line("   \n\t\n"), "");
    assert_eq!(first_line("one"), "one");
    assert_eq!(first_line("\n\n  two words  \nthird"), "two words");
    assert_eq!(first_line("a\r\nb"), "a");
}

#[test]
fn truncate_is_char_safe() {
    assert_eq!(truncate_chars("", 5), "");
    assert_eq!(truncate_chars("short", 5), "short");
    assert_eq!(truncate_chars("short", 10), "short");
    assert_eq!(truncate_chars("abcdef", 5), "abcd…");
    assert_eq!(truncate_chars("abcdef", 1), "…");
    assert_eq!(truncate_chars("abcdef", 0), "");
    assert_eq!(truncate_chars("", 0), "");
    assert_eq!(truncate_chars("привет мир", 7), "привет…");
    assert_eq!(truncate_chars("héllo wörld", 4), "hél…");
    assert_eq!(truncate_chars("日本語のテキスト", 3), "日本…");
    assert_eq!(truncate_chars("日本語", 3), "日本語");
    assert_eq!(truncate_chars("a😀b😀c", 4).chars().count(), 4);
}
