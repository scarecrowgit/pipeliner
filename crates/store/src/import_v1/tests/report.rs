use serde_json::json;

use super::{Fixture, old_issue, old_session, user_text};

#[test]
fn the_report_reads_as_a_short_summary() {
    let fx = Fixture::new();
    fx.write_json(
        "sessions/1763560000001-0001.json",
        &old_session("1763560000001-0001", json!([user_text("hi")])),
    );
    fx.write("sessions/1763560000002-0002.json", "oops");
    fx.write_json(
        "issues.json",
        &json!({"next": 1, "items": [old_issue("HAR-1", "done", None)]}),
    );

    let text = fx.import().to_string();
    assert!(text.starts_with("Import from the old app\n"), "{text}");
    assert!(
        text.contains("sessions     1 imported, 0 already there, 1 failed"),
        "{text}"
    );
    assert!(text.contains("messages     1 imported"), "{text}");
    assert!(text.contains("issues       1 imported"), "{text}");
    assert!(
        text.contains("Problems (1):\n  session sessions/1763560000002-0002.json: "),
        "{text}"
    );

    let again = fx.import().to_string();
    assert!(
        again.contains("sessions     0 imported, 1 already there, 1 failed"),
        "{again}"
    );
    let dry = fx.dry_import().to_string();
    assert!(
        dry.starts_with("Import from the old app (dry run, nothing was written)\n"),
        "{dry}"
    );
    assert!(dry.contains("0 to import, 1 already there"), "{dry}");
}

#[test]
fn an_import_of_an_empty_directory_does_nothing_and_sets_the_marker() {
    let fx = Fixture::new();
    let report = fx.import();
    assert_eq!((report.imported_total(), report.failed_total()), (0, 0));
    assert!(fx.store.marker(crate::import_v1::IMPORT_MARKER).unwrap().is_some());
    // Not even the data directory is needed.
    std::fs::remove_dir_all(fx.data()).unwrap();
    assert_eq!(fx.import().imported_total(), 0);
}
