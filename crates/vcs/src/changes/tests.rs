use std::path::{Path, PathBuf};

use model::DiffLineKind::{Add, Context, Del};

use super::*;
use crate::testutil::{Sandbox, commit_file, git, write};
use crate::worktree;

fn cancel() -> CancelToken {
    CancelToken::new()
}

/// A repo on `main` and a worktree on `chat/work` cut from it.
fn with_worktree(sandbox: &Sandbox) -> (PathBuf, PathBuf) {
    let repo = sandbox.repo("proj");
    let dir = sandbox.path().join("wt");
    worktree::create(&repo, &dir, "chat/work", "main", &cancel()).expect("worktree");
    (repo, dir)
}

fn change(path: &str, added: u32, removed: u32) -> FileChange {
    FileChange { path: path.into(), added, removed }
}

// ---------------------------------------------------------------- changed files

#[test]
fn changed_files_cover_committed_staged_unstaged_and_untracked() {
    let sandbox = Sandbox::new();
    let (repo, wt) = with_worktree(&sandbox);
    commit_file(&repo, "keep.txt", "1\n2\n3\n4\n5\n6\n7\n8\n");
    git(&wt, &["merge", "-q", "main"]);

    commit_file(&wt, "committed.txt", "a\nb\nc\n");
    write(&wt, "keep.txt", "1\n2\n3\n4\n5\n6\n7\nEIGHT\n9\n");
    write(&wt, "staged.txt", "s\n");
    git(&wt, &["add", "staged.txt"]);
    write(&wt, "untracked.txt", "u1\nu2\nno newline at end");
    write(&wt, "dir/deep/new.txt", "x\n");
    write(&wt, "blob.bin", "\0\0\0binary");

    let files = changed_files(&wt, "main", &cancel()).expect("changes");
    assert_eq!(
        files,
        [
            change("blob.bin", 0, 0),
            change("committed.txt", 3, 0),
            change("dir/deep/new.txt", 1, 0),
            change("keep.txt", 2, 1),
            change("staged.txt", 1, 0),
            change("untracked.txt", 3, 0),
        ]
    );
}

#[test]
fn changed_files_are_empty_on_a_clean_branch_and_ignore_ignored_files() {
    let sandbox = Sandbox::new();
    let (_repo, wt) = with_worktree(&sandbox);
    assert_eq!(changed_files(&wt, "main", &cancel()).expect("changes"), []);
    commit_file(&wt, ".gitignore", "target/\n");
    write(&wt, "target/out.o", "junk");
    assert_eq!(changed_files(&wt, "main", &cancel()).expect("changes"), [change(".gitignore", 1, 0)]);
}

#[test]
fn renamed_files_are_one_entry_under_the_new_name() {
    let sandbox = Sandbox::new();
    let repo = sandbox.repo("rename");
    let body = "line\n".repeat(30);
    commit_file(&repo, "old.txt", &body);
    git(&repo, &["checkout", "-q", "-b", "chat/r"]);
    git(&repo, &["mv", "old.txt", "new.txt"]);
    write(&repo, "new.txt", &format!("{body}extra\n"));
    let files = changed_files(&repo, "main", &cancel()).expect("changes");
    assert_eq!(files, [change("new.txt", 1, 0)]);
}

#[test]
fn changes_are_measured_from_the_merge_base_not_the_tip_of_base() {
    let sandbox = Sandbox::new();
    let (repo, wt) = with_worktree(&sandbox);
    commit_file(&wt, "mine.txt", "mine\n");
    // The base moves on; its new file is not "removed" by the branch.
    commit_file(&repo, "theirs.txt", "theirs\n");
    assert_eq!(changed_files(&wt, "main", &cancel()).expect("changes"), [change("mine.txt", 1, 0)]);
}

#[test]
fn a_missing_base_is_a_readable_error_and_origin_is_tried() {
    let sandbox = Sandbox::new();
    let (repo, wt) = with_worktree(&sandbox);
    let e = changed_files(&wt, "nope", &cancel()).expect_err("no base").to_string();
    assert!(e.contains("base `nope` does not exist"), "{e}");
    assert!(changed_files(&wt, "", &cancel()).is_err());

    git(&repo, &["update-ref", "refs/remotes/origin/develop", "main"]);
    commit_file(&wt, "x.txt", "x\n");
    assert_eq!(changed_files(&wt, "develop", &cancel()).expect("via origin"), [change("x.txt", 1, 0)]);
}

// ---------------------------------------------------------------- file diff

#[test]
fn file_diff_of_a_modified_file_has_numbered_lines() {
    let sandbox = Sandbox::new();
    let (_repo, wt) = with_worktree(&sandbox);
    commit_file(&wt, "a.txt", "one\ntwo\nthree\n");
    write(&wt, "a.txt", "one\n2\nthree\nfour\n");
    let diff = file_diff(&wt, "main", "a.txt", &cancel()).expect("diff");
    assert_eq!(diff.path, "a.txt");
    assert!(diff.created, "a.txt did not exist on main");
    assert_eq!((diff.added, diff.removed), (4, 0));

    commit_file(&wt, "b.txt", "one\ntwo\nthree\n");
    git(&wt, &["checkout", "-q", "-B", "chat/work"]);
    // Against the branch's own earlier state: use a second base branch.
    git(&wt, &["branch", "snapshot"]);
    write(&wt, "b.txt", "one\n2\nthree\nfour\n");
    let diff = file_diff(&wt, "snapshot", "b.txt", &cancel()).expect("diff");
    assert!(!diff.created);
    assert_eq!((diff.added, diff.removed), (2, 1));
    let kinds: Vec<_> = diff.hunks[0].lines.iter().map(|l| (l.kind, l.old_no, l.new_no, l.text.as_str())).collect();
    assert_eq!(
        kinds,
        [
            (Context, Some(1), Some(1), "one"),
            (Del, Some(2), None, "two"),
            (Add, None, Some(2), "2"),
            (Context, Some(3), Some(3), "three"),
            (Add, None, Some(4), "four"),
        ]
    );
}

#[test]
fn file_diff_of_an_untracked_file_is_a_new_file() {
    let sandbox = Sandbox::new();
    let (_repo, wt) = with_worktree(&sandbox);
    write(&wt, "src/new file.rs", "fn main() {}\n// two\n");
    let diff = file_diff(&wt, "main", "src/new file.rs", &cancel()).expect("diff");
    assert_eq!(diff.path, "src/new file.rs");
    assert!(diff.created);
    assert_eq!((diff.added, diff.removed), (2, 0));
    assert_eq!(diff.hunks.len(), 1);
    assert_eq!(diff.hunks[0].lines[0].new_no, Some(1));
}

#[test]
fn file_diff_of_a_renamed_file_shows_only_the_edit() {
    let sandbox = Sandbox::new();
    let repo = sandbox.repo("rename-diff");
    let body: String = (1..=30).map(|n| format!("line {n}\n")).collect();
    commit_file(&repo, "old.txt", &body);
    git(&repo, &["checkout", "-q", "-b", "chat/r"]);
    git(&repo, &["mv", "old.txt", "new.txt"]);
    write(&repo, "new.txt", &body.replace("line 15\n", "line fifteen\n"));
    let diff = file_diff(&repo, "main", "new.txt", &cancel()).expect("diff");
    assert_eq!(diff.path, "new.txt");
    assert!(!diff.created);
    assert_eq!((diff.added, diff.removed), (1, 1));
}

#[test]
fn file_diff_edge_cases() {
    let sandbox = Sandbox::new();
    let (_repo, wt) = with_worktree(&sandbox);
    // No change: an empty diff for the path.
    let diff = file_diff(&wt, "main", "README.md", &cancel()).expect("diff");
    assert_eq!(diff, FileDiff { path: "README.md".into(), ..FileDiff::default() });
    // Deleted file.
    std::fs::remove_file(wt.join("README.md")).unwrap();
    let diff = file_diff(&wt, "main", "README.md", &cancel()).expect("diff");
    assert_eq!((diff.added, diff.removed), (0, 1));
    // Binary file: no hunks.
    write(&wt, "b.bin", "\0\u{1}\u{2}");
    git(&wt, &["add", "b.bin"]);
    let diff = file_diff(&wt, "main", "b.bin", &cancel()).expect("diff");
    assert!(diff.hunks.is_empty() && diff.created);
    // Paths that leave the worktree are refused; glob characters are literal.
    assert!(file_diff(&wt, "main", "../x", &cancel()).is_err());
    assert!(file_diff(&wt, "main", "/etc/passwd", &cancel()).is_err());
    assert!(file_diff(&wt, "main", "", &cancel()).is_err());
    write(&wt, "star*.txt", "x\n");
    write(&wt, "starfish.txt", "y\n");
    let diff = file_diff(&wt, "main", "star*.txt", &cancel()).expect("diff");
    assert_eq!((diff.path.as_str(), diff.added), ("star*.txt", 1));
}

// ---------------------------------------------------------------- parse_unified

#[test]
fn parses_a_modified_file_with_two_hunks() {
    let text = "\
diff --git a/src/lib.rs b/src/lib.rs
index 111..222 100644
--- a/src/lib.rs
+++ b/src/lib.rs
@@ -1,4 +1,4 @@ fn first()
 a
-b
+B
 c
 d
@@ -20,3 +20,4 @@ fn second()
 x
 y
+z
 w
";
    let files = parse_unified(text);
    assert_eq!(files.len(), 1);
    let f = &files[0];
    assert_eq!((f.path.as_str(), f.added, f.removed, f.created), ("src/lib.rs", 2, 1, false));
    assert_eq!(f.hunks.len(), 2);
    let first = &f.hunks[0].lines;
    assert_eq!(first.len(), 5);
    assert_eq!((first[1].kind, first[1].old_no, first[1].new_no, first[1].text.as_str()), (Del, Some(2), None, "b"));
    assert_eq!((first[2].kind, first[2].old_no, first[2].new_no), (Add, None, Some(2)));
    assert_eq!((first[4].old_no, first[4].new_no), (Some(4), Some(4)));
    let second = &f.hunks[1].lines;
    assert_eq!((second[0].old_no, second[0].new_no), (Some(20), Some(20)));
    assert_eq!((second[2].kind, second[2].new_no), (Add, Some(22)));
    assert_eq!((second[3].old_no, second[3].new_no), (Some(22), Some(23)));
}

#[test]
fn parses_new_and_deleted_files() {
    let text = "\
diff --git a/new.txt b/new.txt
new file mode 100644
index 0000000..e69de29
--- /dev/null
+++ b/new.txt
@@ -0,0 +1,2 @@
+hello
+world
diff --git a/gone.txt b/gone.txt
deleted file mode 100644
index e69de29..0000000
--- a/gone.txt
+++ /dev/null
@@ -1,3 +0,0 @@
-one
-two
-three
";
    let files = parse_unified(text);
    assert_eq!(files.len(), 2);
    assert_eq!((files[0].path.as_str(), files[0].created, files[0].added, files[0].removed), ("new.txt", true, 2, 0));
    assert_eq!(files[0].hunks[0].lines[1].new_no, Some(2));
    assert_eq!((files[1].path.as_str(), files[1].created, files[1].added, files[1].removed), ("gone.txt", false, 0, 3));
    assert_eq!(files[1].hunks[0].lines[2].old_no, Some(3));
}

#[test]
fn parses_renames_with_and_without_edits() {
    let text = "\
diff --git a/old name.txt b/new name.txt
similarity index 100%
rename from old name.txt
rename to new name.txt
diff --git a/a.rs b/b.rs
similarity index 90%
rename from a.rs
rename to b.rs
index 1..2 100644
--- a/a.rs
+++ b/b.rs
@@ -1,2 +1,2 @@
 keep
-old
+new
";
    let files = parse_unified(text);
    assert_eq!(files.len(), 2);
    assert_eq!(files[0].path, "new name.txt");
    assert!(files[0].hunks.is_empty() && !files[0].created);
    assert_eq!((files[1].path.as_str(), files[1].added, files[1].removed), ("b.rs", 1, 1));
}

#[test]
fn no_newline_marker_is_not_a_line() {
    let text = "\
diff --git a/f b/f
index 1..2 100644
--- a/f
+++ b/f
@@ -1,2 +1,2 @@
 keep
-last
\\ No newline at end of file
+last!
\\ No newline at end of file
";
    let files = parse_unified(text);
    let lines = &files[0].hunks[0].lines;
    assert_eq!(lines.len(), 3);
    assert_eq!((lines[1].kind, lines[1].text.as_str(), lines[1].old_no), (Del, "last", Some(2)));
    assert_eq!((lines[2].kind, lines[2].text.as_str(), lines[2].new_no), (Add, "last!", Some(2)));
    assert_eq!((files[0].added, files[0].removed), (1, 1));
}

#[test]
fn parses_binary_and_mode_only_changes() {
    let text = "\
diff --git a/logo.png b/logo.png
new file mode 100644
index 0000000..abcdef1
Binary files /dev/null and b/logo.png differ
diff --git a/pic with space.png b/pic with space.png
index 1..2 100644
Binary files a/pic with space.png and b/pic with space.png differ
diff --git a/run.sh b/run.sh
old mode 100644
new mode 100755
";
    let files = parse_unified(text);
    let names: Vec<_> = files.iter().map(|f| f.path.as_str()).collect();
    assert_eq!(names, ["logo.png", "pic with space.png", "run.sh"]);
    assert!(files.iter().all(|f| f.hunks.is_empty() && f.added == 0 && f.removed == 0));
    assert!(files[0].created && !files[1].created);
}

#[test]
fn hunk_counts_protect_lines_that_look_like_headers() {
    let text = "\
diff --git a/x.md b/x.md
index 1..2 100644
--- a/x.md
+++ b/x.md
@@ -1,3 +1,2 @@
 title
--- a/not/a/header
-+++ b/not/either
+++ b/neither
";
    let files = parse_unified(text);
    assert_eq!(files.len(), 1, "{files:?}");
    let lines = &files[0].hunks[0].lines;
    assert_eq!(lines.len(), 4, "{lines:?}");
    assert_eq!(lines[1].text, "-- a/not/a/header");
    assert_eq!((files[0].added, files[0].removed), (1, 2));
}

#[test]
fn parses_quoted_paths_plain_unified_diffs_and_empty_input() {
    let text = "\
diff --git \"a/caf\\303\\251.txt\" \"b/caf\\303\\251.txt\"
index 1..2 100644
--- \"a/caf\\303\\251.txt\"
+++ \"b/caf\\303\\251.txt\"
@@ -1 +1 @@
-a
+b
";
    let files = parse_unified(text);
    assert_eq!(files[0].path, "café.txt");

    let plain = "--- a/one.txt\t2024-01-01\n+++ b/one.txt\t2024-01-02\n@@ -1 +1 @@\n-x\n+y\n--- a/two.txt\n+++ b/two.txt\n@@ -5,0 +6 @@\n+z\n";
    let files = parse_unified(plain);
    assert_eq!(files.iter().map(|f| f.path.as_str()).collect::<Vec<_>>(), ["one.txt", "two.txt"]);
    assert_eq!(files[1].hunks[0].lines[0].new_no, Some(6));

    assert!(parse_unified("").is_empty());
    assert!(parse_unified("just some text\nwithout a diff\n").is_empty());
}

#[test]
fn a_blank_context_line_without_its_space_is_kept() {
    let text = "diff --git a/f b/f\n--- a/f\n+++ b/f\n@@ -1,3 +1,3 @@\n a\n\n-c\n+d\n";
    let lines = &parse_unified(text)[0].hunks[0].lines;
    assert_eq!(lines.len(), 4);
    assert_eq!((lines[1].kind, lines[1].text.as_str(), lines[1].new_no), (Context, "", Some(2)));
}

// ---------------------------------------------------------------- commit_all and push

#[test]
fn commit_all_commits_everything_or_reports_nothing() {
    let sandbox = Sandbox::new();
    let (_repo, wt) = with_worktree(&sandbox);
    assert_eq!(commit_all(&wt, "nothing", &cancel()).expect("commit"), None);
    write(&wt, "a.txt", "a\n");
    write(&wt, "sub/b.txt", "b\n");
    let head = commit_all(&wt, "HAR-4: add files\n\nBody line", &cancel()).expect("commit").expect("some");
    assert_eq!(head.len(), 40);
    assert_eq!(git(&wt, &["rev-parse", "HEAD"]).trim(), head);
    assert_eq!(git(&wt, &["log", "-1", "--format=%B"]).trim(), "HAR-4: add files\n\nBody line");
    assert!(git(&wt, &["status", "--porcelain"]).trim().is_empty());
    assert_eq!(commit_all(&wt, "again", &cancel()).expect("commit"), None);
    assert!(commit_all(&wt, "  ", &cancel()).is_err());
}

#[test]
fn commit_all_commits_without_a_configured_identity() {
    let sandbox = Sandbox::new();
    let repo = sandbox.path().join("anon");
    std::fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    write(&repo, "a", "a\n");
    // Whether or not the machine has a global identity, this must not fail.
    assert!(commit_all(&repo, "first", &cancel()).expect("commit").is_some());
}

#[test]
fn a_failing_hook_is_reported_not_hung() {
    use std::os::unix::fs::PermissionsExt;
    let sandbox = Sandbox::new();
    let (_repo, wt) = with_worktree(&sandbox);
    let hooks = sandbox.path().join("proj/.git/hooks");
    std::fs::create_dir_all(&hooks).unwrap();
    let hook = hooks.join("pre-commit");
    std::fs::write(&hook, "#!/bin/sh\necho lint failed >&2\nexit 1\n").unwrap();
    std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
    write(&wt, "a.txt", "a\n");
    let e = commit_all(&wt, "blocked", &cancel()).expect_err("hook fails").to_string();
    assert!(e.contains("lint failed"), "{e}");
}

#[test]
fn push_sets_the_upstream_and_reports_missing_remotes() {
    let sandbox = Sandbox::new();
    let (repo, wt) = with_worktree(&sandbox);
    commit_file(&wt, "a.txt", "a\n");
    let e = push(&wt, "chat/work", &cancel()).expect_err("no remote").to_string();
    assert!(e.contains("no remote"), "{e}");

    let remote = sandbox.bare("remote.git");
    git(&repo, &["remote", "add", "origin", remote.to_str().unwrap()]);
    push(&wt, "chat/work", &cancel()).expect("push");
    assert!(git(&remote, &["branch", "--list", "chat/work"]).contains("chat/work"));
    assert_eq!(git(&wt, &["config", "--get", "branch.chat/work.remote"]).trim(), "origin");
    assert!(push(&wt, "-x", &cancel()).is_err());
    assert!(push(&wt, "missing-branch", &cancel()).is_err());
}

// ---------------------------------------------------------------- merge

fn head_of(dir: &Path, name: &str) -> String {
    git(dir, &["rev-parse", name]).trim().to_string()
}

#[test]
fn merges_into_a_base_that_is_not_checked_out() {
    let sandbox = Sandbox::new();
    let repo = sandbox.repo("merge-free");
    git(&repo, &["checkout", "-q", "-b", "chat/a"]);
    commit_file(&repo, "a.txt", "a\n");
    git(&repo, &["checkout", "-q", "-b", "elsewhere", "main"]);
    let before = head_of(&repo, "main");
    let outcome = merge_into_base(&repo, "chat/a", "main", &cancel()).expect("merge");
    let MergeOutcome::Merged { head } = outcome else { panic!("expected a merge, got {outcome:?}") };
    assert_eq!(head_of(&repo, "main"), head);
    assert_eq!(git(&repo, &["rev-list", "--parents", "-n", "1", "main"]).split_whitespace().count(), 3);
    assert!(git(&repo, &["log", "-1", "--format=%s", "main"]).contains("Merge branch 'chat/a' into main"));
    assert_eq!(git(&repo, &["rev-parse", "main^1"]).trim(), before);
    // The checked-out branch and its files were not touched.
    assert_eq!(git(&repo, &["rev-parse", "--abbrev-ref", "HEAD"]).trim(), "elsewhere");
    assert!(!repo.join("a.txt").exists());
    // Merging again changes nothing.
    assert_eq!(merge_into_base(&repo, "chat/a", "main", &cancel()).expect("merge"), MergeOutcome::UpToDate);
}

#[test]
fn merges_into_a_clean_checked_out_base_and_updates_its_files() {
    let sandbox = Sandbox::new();
    let (repo, wt) = with_worktree(&sandbox);
    commit_file(&wt, "feature.txt", "f\n");
    let outcome = merge_into_base(&repo, "chat/work", "main", &cancel()).expect("merge");
    assert!(matches!(outcome, MergeOutcome::Merged { .. }));
    assert_eq!(std::fs::read_to_string(repo.join("feature.txt")).unwrap(), "f\n");
    assert!(git(&repo, &["status", "--porcelain"]).trim().is_empty());
    // Also when called from the worktree.
    commit_file(&wt, "second.txt", "s\n");
    let outcome = merge_into_base(&wt, "chat/work", "main", &cancel()).expect("merge");
    assert!(matches!(outcome, MergeOutcome::Merged { .. }));
    assert!(repo.join("second.txt").is_file());
}

#[test]
fn a_dirty_main_checkout_keeps_its_changes() {
    let sandbox = Sandbox::new();
    let (repo, wt) = with_worktree(&sandbox);
    commit_file(&repo, "mine.txt", "base\n");
    git(&wt, &["merge", "-q", "main"]);
    commit_file(&wt, "feature.txt", "f\n");
    // Uncommitted work in the main checkout, in files the merge does not touch.
    write(&repo, "mine.txt", "local edit\n");
    write(&repo, "scratch.txt", "untracked\n");
    let outcome = merge_into_base(&repo, "chat/work", "main", &cancel()).expect("merge");
    assert!(matches!(outcome, MergeOutcome::Merged { .. }));
    assert_eq!(std::fs::read_to_string(repo.join("mine.txt")).unwrap(), "local edit\n");
    assert_eq!(std::fs::read_to_string(repo.join("scratch.txt")).unwrap(), "untracked\n");
    assert_eq!(std::fs::read_to_string(repo.join("feature.txt")).unwrap(), "f\n");
    let status = git(&repo, &["status", "--porcelain"]);
    assert!(status.contains(" M mine.txt") && status.contains("?? scratch.txt"), "{status}");
    assert!(!status.contains("feature.txt"), "{status}");
}

#[test]
fn local_changes_in_files_the_merge_touches_stop_it_with_nothing_changed() {
    let sandbox = Sandbox::new();
    let (repo, wt) = with_worktree(&sandbox);
    commit_file(&wt, "README.md", "hello\nfrom the branch\n");
    write(&repo, "README.md", "local edit\n");
    let before = head_of(&repo, "main");
    let e = merge_into_base(&repo, "chat/work", "main", &cancel()).expect_err("blocked").to_string();
    assert!(e.contains("nothing was merged"), "{e}");
    assert_eq!(head_of(&repo, "main"), before);
    assert_eq!(std::fs::read_to_string(repo.join("README.md")).unwrap(), "local edit\n");
}

#[test]
fn a_conflict_leaves_the_tree_exactly_as_it_was() {
    let sandbox = Sandbox::new();
    let (repo, wt) = with_worktree(&sandbox);
    commit_file(&repo, "f.txt", "a\nb\nc\n");
    git(&wt, &["merge", "-q", "main"]);
    commit_file(&wt, "f.txt", "a\nBRANCH\nc\n");
    commit_file(&wt, "only-branch.txt", "x\n");
    commit_file(&repo, "f.txt", "a\nMAIN\nc\n");
    write(&repo, "scratch.txt", "untracked\n");
    let (main_before, branch_before) = (head_of(&repo, "main"), head_of(&repo, "chat/work"));

    let outcome = merge_into_base(&repo, "chat/work", "main", &cancel()).expect("merge");
    assert_eq!(outcome, MergeOutcome::Conflict { files: vec!["f.txt".to_string()] });
    assert_eq!(head_of(&repo, "main"), main_before);
    assert_eq!(head_of(&repo, "chat/work"), branch_before);
    assert_eq!(std::fs::read_to_string(repo.join("f.txt")).unwrap(), "a\nMAIN\nc\n");
    assert!(!repo.join("only-branch.txt").exists());
    assert_eq!(git(&repo, &["status", "--porcelain"]).trim(), "?? scratch.txt");
    assert!(!repo.join(".git/MERGE_HEAD").exists());
    assert!(git(&wt, &["status", "--porcelain"]).trim().is_empty());
}

#[test]
fn merge_argument_errors_are_readable() {
    let sandbox = Sandbox::new();
    let (repo, _wt) = with_worktree(&sandbox);
    let err = |branch: &str, base: &str| merge_into_base(&repo, branch, base, &cancel()).expect_err("fails").to_string();
    assert!(err("chat/work", "chat/work").contains("into itself"));
    assert!(err("chat/work", "nope").contains("not a local branch"));
    assert!(err("nope", "main").contains("does not exist"));
    assert!(err("", "main").contains("needs a branch"));
    git(&repo, &["checkout", "-q", "--orphan", "island"]);
    git(&repo, &["rm", "-rfq", "."]);
    git(&repo, &["commit", "-q", "--allow-empty", "-m", "island"]);
    git(&repo, &["checkout", "-q", "main"]);
    let e = err("island", "main");
    assert!(e.contains("merge-tree"), "{e}");
}
