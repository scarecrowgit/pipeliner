//! Tests of the catalog as a whole: locations, writing, transfer, templates, the self-edit graph.

use std::fs;
use std::path::{Path, PathBuf};

use base::paths::Paths;
use model::*;

use crate::write::{self, *};
use crate::{Catalog, Instance, Place, SelfEditModels, TemplateContext, instantiate, self_edit_pipeline, templates, transfer};

struct Fixture {
    _tmp: tempfile::TempDir,
    paths: Paths,
    repo: PathBuf,
}

fn fixture() -> Fixture {
    let tmp = tempfile::tempdir().unwrap();
    let paths = Paths::under(tmp.path());
    let repo = tmp.path().join("repo");
    fs::create_dir_all(&repo).unwrap();
    Fixture { _tmp: tmp, paths, repo }
}

fn put(path: &Path, text: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

fn md(description: &str) -> String {
    format!("---\nname: X\ndescription: {description}\n---\nRole.\n")
}

fn agent_named(id: &str, name: &str) -> Runnable {
    Runnable {
        reference: RunnableRef::agent(id),
        kind: RunnableKind::Agent,
        name: name.into(),
        description: "d".into(),
        origin: Origin::Library,
        uses: Uses::default(),
        paused: false,
        archived: false,
        agent: Some(AgentDef { role: "Do it.".into(), ..Default::default() }),
        path: None,
    }
}

impl Fixture {
    fn load(&self) -> Catalog {
        Catalog::load(&self.paths, Some(&self.repo))
    }
    fn description(&self, id: &str) -> String {
        self.load().get(&RunnableRef::agent(id)).unwrap().description.clone()
    }
}

#[test]
fn built_ins_are_there_without_files() {
    let f = fixture();
    let cat = Catalog::load(&f.paths, None);
    let build = cat.get(&RunnableRef::agent("build")).unwrap();
    assert_eq!(build.origin, Origin::Builtin);
    assert_eq!(cat.agent_def(&build.reference).unwrap().tools.access, ToolAccess::Full);
    let plan = cat.agent_def(&RunnableRef::agent("plan")).unwrap();
    assert_eq!(plan.tools.access, ToolAccess::ReadOnly);
    assert!(plan.role.contains("never change anything"));
    assert!(cat.problems().is_empty());
}

#[test]
fn later_locations_override_earlier_ones() {
    let f = fixture();
    let lib = Place::Library.dir(&f.paths, RunnableKind::Agent);
    let proj = Place::Project(f.repo.clone()).dir(&f.paths, RunnableKind::Agent);
    put(&f.paths.home.join(".claude/agents/x.md"), &md("claude home"));
    assert_eq!(f.description("x"), "claude home");
    put(&f.repo.join(".claude/agents/x.md"), &md("claude repo"));
    assert_eq!(f.description("x"), "claude repo");
    put(&lib.join("x.md"), &md("library"));
    assert_eq!(f.description("x"), "library");
    put(&proj.join("x.md"), &md("project"));
    let cat = f.load();
    assert_eq!(cat.get(&RunnableRef::agent("x")).unwrap().origin, Origin::Project { repo: f.repo.clone() });
    assert_eq!(f.description("x"), "project");
    // Without a repo only the user's locations count.
    let no_repo = Catalog::load(&f.paths, None);
    assert_eq!(no_repo.get(&RunnableRef::agent("x")).unwrap().description, "library");
    // A file in agents/ can replace a built-in.
    put(&lib.join("plan.md"), &md("my plan"));
    assert_eq!(f.description("plan"), "my plan");
}

#[test]
fn a_broken_file_is_a_problem_not_a_failure() {
    let f = fixture();
    let lib = Place::Library.dir(&f.paths, RunnableKind::Agent);
    put(&lib.join("good.md"), &md("fine"));
    put(&lib.join("bad.md"), "no header at all");
    put(&lib.join("worse.md"), "---\nname: w\nharness:\n  access: root\n---\nx");
    put(&Place::Library.dir(&f.paths, RunnableKind::Pipeline).join("broken.toml"), "id = [");
    put(&f.paths.home.join(".claude/agents/README.md"), "# not an agent");
    let cat = f.load();
    assert!(cat.get(&RunnableRef::agent("good")).is_some());
    assert!(cat.get(&RunnableRef::agent("bad")).is_none());
    let mut files: Vec<_> = cat.problems().iter().map(|p| p.path.file_name().unwrap().to_string_lossy().into_owned()).collect();
    files.sort();
    assert_eq!(files, ["bad.md", "broken.toml", "worse.md"]);
    assert!(cat.problems().iter().any(|p| p.message.contains("harness.access")));
}

#[test]
fn claude_code_agents_are_imported_read_only() {
    let f = fixture();
    put(
        &f.paths.home.join(".claude/agents/code-reviewer.md"),
        "---\nname: code-reviewer\ndescription: Reviews code\ntools: Read, Grep, Bash(git diff:*)\nmodel: sonnet\n---\nBe strict.\n",
    );
    let cat = f.load();
    let r = cat.get(&RunnableRef::agent("code-reviewer")).unwrap();
    assert!(matches!(r.origin, Origin::ClaudeCode { .. }));
    let def = r.agent.as_ref().unwrap();
    assert_eq!(def.tools.access, ToolAccess::ReadAndRun);
    assert!(def.model.is_empty());
    assert_eq!(def.role, "Be strict.");
    // Not writable, not deletable, not renameable.
    let err = save_agent(&f.paths, &Place::Library, r).unwrap_err().to_string();
    assert!(err.contains("Claude Code") && err.contains("duplicate"), "{err}");
    assert!(write::delete(&cat, &r.reference).is_err());
    assert!(write::rename(&cat, &r.reference, "Other").is_err());
    assert!(set_paused(&cat, &r.reference, true).is_err());
}

#[test]
fn built_ins_cannot_be_written_but_can_be_duplicated() {
    let f = fixture();
    let cat = f.load();
    let build = cat.get(&RunnableRef::agent("build")).unwrap();
    let err = save_agent(&f.paths, &Place::Library, build).unwrap_err().to_string();
    assert!(err.contains("built in"), "{err}");
    assert!(write::delete(&cat, &build.reference).is_err());
    let copy = duplicate(&cat, &f.paths, &Place::Library, &build.reference).unwrap();
    assert_eq!(copy, RunnableRef::agent("build-copy"));
    let cat = f.load();
    let copy = cat.get(&copy).unwrap();
    assert_eq!(copy.name, "Build copy");
    assert_eq!(copy.origin, Origin::Library);
    assert_eq!(cat.agent_def(&copy.reference).unwrap().role, cat.agent_def(&build.reference).unwrap().role);
}

#[test]
fn duplicate_picks_free_names() {
    let f = fixture();
    save_agent(&f.paths, &Place::Library, &agent_named("fixer", "Fixer")).unwrap();
    let a = duplicate(&f.load(), &f.paths, &Place::Library, &RunnableRef::agent("fixer")).unwrap();
    let b = duplicate(&f.load(), &f.paths, &Place::Library, &RunnableRef::agent("fixer")).unwrap();
    let c = duplicate(&f.load(), &f.paths, &Place::Project(f.repo.clone()), &RunnableRef::agent("fixer")).unwrap();
    assert_eq!([a.as_str(), b.as_str(), c.as_str()], ["agent:fixer-copy", "agent:fixer-copy-2", "agent:fixer-copy-3"]);
    let cat = f.load();
    assert_eq!(cat.get(&b).unwrap().name, "Fixer copy 2");
    assert_eq!(cat.get(&c).unwrap().origin, Origin::Project { repo: f.repo.clone() });
}

#[test]
fn rename_follows_the_slug_and_fails_on_a_clash() {
    let f = fixture();
    save_agent(&f.paths, &Place::Library, &agent_named("fixer", "Fixer")).unwrap();
    save_agent(&f.paths, &Place::Library, &agent_named("tester", "Tester")).unwrap();
    let cat = f.load();
    let err = write::rename(&cat, &RunnableRef::agent("fixer"), "tester").unwrap_err().to_string();
    assert!(err.contains("already taken"), "{err}");
    let err = write::rename(&cat, &RunnableRef::agent("fixer"), "Build").unwrap_err().to_string();
    assert!(err.contains("already taken"), "{err}");
    assert!(write::rename(&cat, &RunnableRef::agent("fixer"), "  ").is_err());
    assert!(f.load().get(&RunnableRef::agent("fixer")).is_some());

    let new = write::rename(&cat, &RunnableRef::agent("fixer"), "Bug fixer").unwrap();
    assert_eq!(new, RunnableRef::agent("bug-fixer"));
    let cat = f.load();
    assert!(cat.get(&RunnableRef::agent("fixer")).is_none());
    assert_eq!(cat.get(&new).unwrap().name, "Bug fixer");
    // Same id, new capitalisation: only the name changes.
    let same = write::rename(&cat, &new, "Bug Fixer").unwrap();
    assert_eq!(same, new);
    assert_eq!(f.load().get(&new).unwrap().name, "Bug Fixer");
}

#[test]
fn pause_archive_and_delete() {
    let f = fixture();
    save_agent(&f.paths, &Place::Library, &agent_named("fixer", "Fixer")).unwrap();
    let r = RunnableRef::agent("fixer");
    set_paused(&f.load(), &r, true).unwrap();
    set_archived(&f.load(), &r, true).unwrap();
    let cat = f.load();
    assert!(cat.get(&r).unwrap().paused && cat.get(&r).unwrap().archived);
    set_paused(&cat, &r, false).unwrap();
    let cat = f.load();
    assert!(!cat.get(&r).unwrap().paused && cat.get(&r).unwrap().archived);
    write::delete(&cat, &r).unwrap();
    assert!(f.load().get(&r).is_none());
}

#[test]
fn pipelines_save_load_and_keep_their_switches() {
    let f = fixture();
    let Instance::Pipeline(graph) = instantiate(&f.load(), "agent-pr", &TemplateContext { repo: Some(f.repo.clone()), ..Default::default() }).unwrap() else {
        panic!("pipeline expected")
    };
    let path = save_pipeline(&f.paths, &Place::Library, &graph).unwrap();
    assert!(path.ends_with("library/pipelines/agent-pull-request.toml"));
    let cat = f.load();
    let id = graph.id.clone();
    assert_eq!(cat.pipeline(&id), Some(&graph));
    assert!(cat.pipeline_text(&id).unwrap().contains("type = \"source\""));
    let r = RunnableRef::pipeline(id.as_str());
    assert_eq!(cat.get(&r).unwrap().name, "Agent → pull request");

    set_paused(&cat, &r, true).unwrap();
    set_archived(&f.load(), &r, true).unwrap();
    assert!(f.load().get(&r).unwrap().paused);
    // Saving again keeps the switches; the graph is unchanged.
    save_pipeline(&f.paths, &Place::Library, &graph).unwrap();
    let cat = f.load();
    assert!(cat.get(&r).unwrap().paused && cat.get(&r).unwrap().archived);
    assert_eq!(cat.pipeline(&id), Some(&graph));
    set_paused(&cat, &r, false).unwrap();
    let cat = f.load();
    assert!(!cat.get(&r).unwrap().paused && cat.get(&r).unwrap().archived);
    // No temp files are left behind.
    let leftovers = fs::read_dir(path.parent().unwrap()).unwrap().flatten().filter(|e| e.file_name().to_string_lossy().ends_with(".tmp")).count();
    assert_eq!(leftovers, 0);

    // Rename and duplicate move the whole thing, flags included on rename.
    let new = write::rename(&cat, &r, "Ship it").unwrap();
    assert_eq!(new, RunnableRef::pipeline("ship-it"));
    let cat = f.load();
    assert!(cat.get(&r).is_none() && cat.get(&new).unwrap().archived);
    assert_eq!(cat.pipeline(&PipelineId::new("ship-it")).unwrap().name, "Ship it");
    let copy = duplicate(&cat, &f.paths, &Place::Library, &new).unwrap();
    assert_eq!(copy, RunnableRef::pipeline("ship-it-copy"));
    assert!(write::rename(&f.load(), &copy, "Ship it").is_err());
}

#[test]
fn files_are_ordered_orchestrators_pipelines_agents() {
    let f = fixture();
    save_agent(&f.paths, &Place::Library, &agent_named("zed", "Zed")).unwrap();
    let mut lead = agent_named("lead", "Lead");
    lead.kind = RunnableKind::Orchestrator;
    lead.reference = RunnableRef::orchestrator("lead");
    save_agent(&f.paths, &Place::Library, &lead).unwrap();
    let Instance::Pipeline(graph) = instantiate(&f.load(), "agent-pr", &TemplateContext::default()).unwrap() else { panic!() };
    save_pipeline(&f.paths, &Place::Library, &graph).unwrap();
    let order: Vec<String> = f.load().runnables().iter().map(|r| r.reference.to_string()).collect();
    assert_eq!(order, ["orchestrator:lead", "pipeline:agent-pull-request", "agent:build", "agent:plan", "agent:zed"]);
}

#[test]
fn export_and_import_detect_the_type_from_content() {
    let f = fixture();
    let cat = f.load();
    let out = f.repo.join("export");
    fs::create_dir_all(&out).unwrap();
    // Agent exported under a misleading extension is still an agent.
    let agent_file = out.join("planner.txt");
    transfer::export(&cat, &RunnableRef::agent("plan"), &agent_file).unwrap();
    let r = transfer::import(&cat, &f.paths, &Place::Library, &agent_file).unwrap();
    assert_eq!(r, RunnableRef::agent("planner"));
    let cat = f.load();
    assert_eq!(cat.agent_def(&r).unwrap().tools.access, ToolAccess::ReadOnly);
    assert_eq!(cat.get(&r).unwrap().name, "Plan");
    // Importing the same file again gets a free id.
    let again = transfer::import(&cat, &f.paths, &Place::Library, &agent_file).unwrap();
    assert_eq!(again, RunnableRef::agent("planner-2"));

    // A pipeline in a `.md` file.
    let Instance::Pipeline(graph) = instantiate(&cat, "agent-x3-review-pr", &TemplateContext::default()).unwrap() else { panic!() };
    save_pipeline(&f.paths, &Place::Library, &graph).unwrap();
    let cat = f.load();
    let pipe_file = out.join("pipeline.md");
    transfer::export(&cat, &RunnableRef::pipeline(graph.id.as_str()), &pipe_file).unwrap();
    let imported = transfer::import(&cat, &f.paths, &Place::Project(f.repo.clone()), &pipe_file).unwrap();
    // The id is taken in the library, so the import gets a free one.
    assert_eq!(imported, RunnableRef::pipeline("agent-review-pull-request-2"));
    let cat = f.load();
    assert_eq!(cat.get(&imported).unwrap().origin, Origin::Project { repo: f.repo.clone() });
    let copy = cat.pipeline(&PipelineId::new(imported.id())).unwrap();
    assert_eq!((&copy.nodes, &copy.edges), (&graph.nodes, &graph.edges));

    let junk = out.join("junk.json");
    fs::write(&junk, "{\"a\": 1}").unwrap();
    assert!(transfer::import(&cat, &f.paths, &Place::Library, &junk).unwrap_err().to_string().contains("neither"));
    // A Claude Code file imports with its tools mapped.
    let cc = out.join("helper.md");
    fs::write(&cc, "---\nname: helper\ntools: Read, Edit\nmodel: opus\n---\nHelp.").unwrap();
    let r = transfer::import(&cat, &f.paths, &Place::Library, &cc).unwrap();
    let cat = f.load();
    assert_eq!(cat.agent_def(&r).unwrap().tools.deny, ["bash", "write_file"]);
}

#[test]
fn skills_are_listed_and_loaded_through_the_catalog() {
    let f = fixture();
    put(&f.paths.skills_dir().join("pdf/SKILL.md"), "---\nname: pdf\ndescription: PDFs\n---\nUse `scripts/x.py`.");
    put(&f.repo.join(".harness/skills/pdf/SKILL.md"), "---\nname: pdf\ndescription: Project PDFs\n---\nProject body.");
    let cat = f.load();
    assert_eq!(cat.skills().len(), 1);
    assert_eq!(cat.skills()[0].source, SkillSource::Project);
    let text = cat.skill_text("pdf").unwrap();
    assert!(text.contains("Project body.") && text.contains(&format!("{}", f.repo.join(".harness/skills/pdf").display())));
    assert!(cat.skill_text("missing").is_none());
}

#[test]
fn templates_make_agents_and_pipelines_with_free_ids() {
    let f = fixture();
    let list = templates();
    assert_eq!(list.iter().filter(|t| t.kind == RunnableKind::Agent).count(), 3);
    assert_eq!(list.iter().filter(|t| t.kind == RunnableKind::Pipeline).count(), 3);
    let ctx = TemplateContext { repo: Some(f.repo.clone()), mockups_dir: Some(f.repo.join("mockups")), ..Default::default() };
    for template in &list {
        let cat = f.load();
        match instantiate(&cat, &template.id, &ctx).unwrap() {
            Instance::Agent(r) => {
                assert_eq!(template.kind, RunnableKind::Agent);
                assert_eq!(r.name, template.name);
                save_agent(&f.paths, &Place::Library, &r).unwrap();
            }
            Instance::Pipeline(g) => {
                assert_eq!(g.name, template.name);
                check_graph(&g);
                save_pipeline(&f.paths, &Place::Library, &g).unwrap();
            }
        }
    }
    let cat = f.load();
    assert!(cat.problems().is_empty(), "{:?}", cat.problems());
    assert_eq!(cat.agent_def(&RunnableRef::agent("reviewer")).unwrap().tools.access, ToolAccess::ReadOnly);
    assert_eq!(cat.agent_def(&RunnableRef::agent("explorer")).unwrap().tools.access, ToolAccess::ReadOnly);
    assert_eq!(cat.agent_def(&RunnableRef::agent("test-writer")).unwrap().tools.access, ToolAccess::Full);
    // A second Reviewer does not overwrite the first.
    let Instance::Agent(second) = instantiate(&cat, "reviewer", &ctx).unwrap() else { panic!() };
    assert_eq!(second.reference, RunnableRef::agent("reviewer-2"));
    assert!(instantiate(&cat, "nope", &ctx).is_err());
    // The x3 template has three variants and a review.
    let x3 = cat.pipeline(&PipelineId::new("agent-review-pull-request")).unwrap();
    assert!(x3.nodes.iter().any(|n| matches!(&n.kind, NodeKind::Agent(a) if a.variants == 3)));
    assert!(x3.nodes.iter().any(|n| matches!(n.kind, NodeKind::Review(_))));
}

/// Every edge joins existing nodes through a port the source node has.
fn check_graph(graph: &GraphDef) {
    for e in &graph.edges {
        let from = graph.node(&e.from).unwrap_or_else(|| panic!("edge from missing node {}", e.from));
        assert!(graph.node(&e.to).is_some(), "edge to missing node {}", e.to);
        let ports: Vec<String> = match &from.kind {
            NodeKind::Router(r) => r.rules.iter().map(|r| r.id.clone()).chain([graph::PORT_ELSE.to_string()]).collect(),
            NodeKind::Review(_) => vec![graph::PORT_APPROVED.into(), graph::PORT_OUT.into()],
            _ => vec![graph::PORT_OUT.into()],
        };
        assert!(ports.contains(&e.port), "{} has no port {}", e.from, e.port);
        match e.kind {
            EdgeKind::Context => assert!(matches!(from.kind, NodeKind::File(_))),
            EdgeKind::Spawn => assert!(matches!(graph.node(&e.to).unwrap().kind, NodeKind::Subagent(_))),
            EdgeKind::Back => {
                let NodeKind::Review(_) = from.kind else { panic!("back edge from a non-review") };
            }
            EdgeKind::Route => {}
        }
        for w in [&from.kind, &graph.node(&e.to).unwrap().kind] {
            if let NodeKind::Agent(a) | NodeKind::Orchestrator(a) = w {
                assert!(a.workdir.is_empty() || graph.workdir(&a.workdir).is_some());
            }
        }
    }
    let ids: std::collections::HashSet<_> = graph.nodes.iter().map(|n| &n.id).collect();
    assert_eq!(ids.len(), graph.nodes.len(), "node ids are unique");
}

#[test]
fn the_self_edit_pipeline_is_a_sound_graph() {
    let models = SelfEditModels::default();
    let graph = self_edit_pipeline(Path::new("/work/pipeliner"), Path::new("/work/mockups"), &models);
    check_graph(&graph);
    assert_eq!(graph.nodes.len(), 10);
    assert_eq!(graph.edges.len(), 12);

    let text = toml::to_string_pretty(&graph).unwrap();
    let back: GraphDef = toml::from_str(&text).unwrap();
    assert_eq!(back, graph);

    let mockups = graph.workdir("mockups").unwrap();
    assert!(mockups.worktree && mockups.branch_prefix == "design/");
    let code = graph.workdir("code").unwrap();
    assert!(code.worktree && code.setup == "nix-shell shell.nix --run 'cargo build' </dev/null");
    assert!(code.env["CARGO_TARGET_DIR"].ends_with(".harness/cargo-target/pipeliner"));

    let get = |id: &str| &graph.node(&id.into()).unwrap().kind;
    let NodeKind::Agent(designer) = get("designer") else { panic!() };
    assert_eq!(designer.skills.as_ref().unwrap().forced, ["ui-ux-pro-max"]);
    assert!(designer.role.contains("screens-v2/_build/build.py") && designer.role.contains("Never edit the generated HTML"));
    let NodeKind::Review(review) = get("approval") else { panic!() };
    assert_eq!(review.send_back_to, Some("designer".into()));
    assert!(review.skip_if_empty);
    let NodeKind::Orchestrator(build) = get("build") else { panic!() };
    assert_eq!(build.tools.as_ref().unwrap().access, ToolAccess::ReadAndRun);
    assert!(build.spawn.as_ref().unwrap().enabled);
    let NodeKind::Subagent(coder) = get("coder") else { panic!() };
    assert_eq!(coder.tools.access, ToolAccess::Full);
    let NodeKind::File(file) = get("mockups-file") else { panic!() };
    assert_eq!((file.path.as_str(), file.mode), ("screens-v2/**", FileMode::Read));
    let NodeKind::Router(router) = get("triage") else { panic!() };
    assert_eq!(router.rules[0].ask.as_ref().unwrap().workdir, "mockups");
    assert_eq!(router.rules[0].ask.as_ref().unwrap().model, models.triage);
    let build_id = NodeId::new("build");
    let to_build: Vec<_> = graph.edges_to(&build_id).map(|e| (e.from.as_str(), e.port.as_str(), e.kind)).collect();
    assert!(to_build.contains(&("triage", "else", EdgeKind::Route)));
    assert!(to_build.contains(&("mockups-file", "out", EdgeKind::Context)));
    // Nodes sit on a 190 px grid and do not overlap.
    for n in &graph.nodes {
        assert_eq!((n.pos.x - 20.0) % 190.0, 0.0, "{}", n.id);
    }
}
