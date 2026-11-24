//! The old app's linear pipelines come over on import, keep their ids, and are never written twice.

use model::{PipelineId, RunnableRef};
use store::import_v1::PipelineOutcome;

use super::Fx;
use crate::api::CatalogApi;

fn old(workdir: &str) -> String {
    format!(
        r#"
name = "Checks"
[[workdirs]]
id = "code"
name = "Code"
path = "{workdir}"

[[steps]]
id = "tests"
name = "Tests"
kind = "command"
workdir = "code"
command = "cargo test"

[[steps]]
kind = "report"
"#
    )
}

fn outcomes(report: &store::import_v1::ImportReport) -> Vec<(String, PipelineOutcome)> {
    report.pipelines.iter().map(|p| (p.id.clone(), p.outcome.clone())).collect()
}

#[test]
fn old_pipelines_are_converted_into_the_library_once() {
    let fx = Fx::new();
    let dir = fx.paths.v1_pipelines_dir();
    std::fs::create_dir_all(&dir).unwrap();
    let work = fx.root().join("work");
    std::fs::create_dir_all(&work).unwrap();
    std::fs::write(dir.join("mockup-gpui-build.toml"), old(&work.display().to_string())).unwrap();
    std::fs::write(dir.join("no-folder.toml"), old("")).unwrap();
    std::fs::write(dir.join("broken.toml"), "steps = 3").unwrap();
    std::fs::write(dir.join("notes.txt"), "not a pipeline").unwrap();

    let dry = fx.engine.import_v1(true).unwrap();
    assert!(dry.dry_run);
    assert_eq!(dry.pipelines.len(), 3, "{dry}");
    assert!(fx.engine.pipeline(&PipelineId::new("mockup-gpui-build")).is_none(), "a dry run writes nothing");

    let report = fx.engine.import_v1(false).unwrap();
    let got = outcomes(&report);
    assert!(matches!(&got[0], (id, PipelineOutcome::Failed { .. }) if id == "broken"), "{got:#?}");
    let (id, PipelineOutcome::Converted { problems }) = &got[1] else { panic!("{got:#?}") };
    assert_eq!(id, "mockup-gpui-build");
    assert!(problems.iter().all(|p| !p.starts_with("error")), "{problems:?}");
    let (id, PipelineOutcome::Converted { problems }) = &got[2] else { panic!("{got:#?}") };
    assert_eq!(id, "no-folder");
    assert!(problems.iter().any(|p| p.starts_with("error") && p.contains("Code")), "a draft with its problems: {problems:?}");
    assert!(report.failed_total() >= 1);
    let text = report.to_string();
    assert!(text.contains("pipelines") && text.contains("2 converted (1 with problems)"), "{text}");

    let graph = fx.engine.pipeline(&PipelineId::new("mockup-gpui-build")).expect("in the library");
    assert_eq!(graph.workdirs[0].path, work);
    assert!(fx.engine.runnables().iter().any(|r| r.reference == RunnableRef::pipeline("no-folder")));
    assert!(dir.join("mockup-gpui-build.toml").exists(), "old files stay");

    let again = fx.engine.import_v1(false).unwrap();
    let got = outcomes(&again);
    assert_eq!(got[1], ("mockup-gpui-build".to_string(), PipelineOutcome::AlreadyThere));
    assert_eq!(got[2], ("no-folder".to_string(), PipelineOutcome::AlreadyThere));
}

#[test]
fn the_self_edit_template_is_offered_with_its_folders_to_set() {
    let fx = Fx::new();
    assert!(fx.engine.templates().iter().any(|t| t.id == "self-edit"));
    let made = fx.engine.create_from_template("self-edit").unwrap();
    let graph = fx.engine.pipeline(&PipelineId::new(made.id())).expect("saved");
    let problems: Vec<String> = fx.engine.pipeline_problems(&graph).into_iter().map(|p| p.message).collect();
    assert!(problems.iter().any(|m| m == "Workdir Code has no folder"), "{problems:#?}");
    assert!(problems.iter().any(|m| m == "Workdir Mockups has no folder"), "{problems:#?}");
}
