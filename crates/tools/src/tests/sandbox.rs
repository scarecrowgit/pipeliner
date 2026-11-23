use std::os::unix::fs::symlink;

use serde_json::json;
use tempfile::TempDir;

use super::Fixture;
use crate::sandbox::{SandboxError, normalize_subject, resolve};
use crate::{ReadFile, WriteFile};

#[test]
fn relative_and_inside_absolute_paths_resolve() {
    let f = Fixture::new();
    f.write("src/a.rs", "x");
    let croot = f.root().canonicalize().unwrap();
    let r = resolve(f.root(), "src/a.rs").unwrap();
    assert_eq!(r.rel, "src/a.rs");
    assert_eq!(r.abs, croot.join("src/a.rs"));
    let abs = f.path("src/a.rs");
    assert_eq!(resolve(f.root(), abs.to_str().unwrap()).unwrap().rel, "src/a.rs");
    assert_eq!(resolve(f.root(), "./src/../src/./a.rs").unwrap().rel, "src/a.rs");
    assert!(resolve(f.root(), ".").unwrap().is_root());
    assert!(resolve(f.root(), "").unwrap().is_root());
    assert!(resolve(f.root(), croot.to_str().unwrap()).unwrap().is_root());
}

#[test]
fn not_yet_existing_paths_resolve_inside() {
    let f = Fixture::new();
    let r = resolve(f.root(), "new/deep/file.txt").unwrap();
    assert_eq!(r.rel, "new/deep/file.txt");
    assert!(r.abs.starts_with(f.root().canonicalize().unwrap()));
}

#[test]
fn dot_dot_escapes_are_refused() {
    let f = Fixture::new();
    f.write("a/x", "");
    for p in ["..", "../x", "a/../../b", "a/../a/../../etc/passwd", "./../x"] {
        assert!(matches!(resolve(f.root(), p), Err(SandboxError::Escapes(_))), "{p}");
    }
    // Climbing and coming back down stays inside.
    assert_eq!(resolve(f.root(), "a/../a/x").unwrap().rel, "a/x");
}

#[test]
fn absolute_paths_outside_are_refused() {
    let f = Fixture::new();
    let other = TempDir::new().unwrap();
    for p in ["/etc/passwd", "/", other.path().to_str().unwrap()] {
        assert!(matches!(resolve(f.root(), p), Err(SandboxError::Outside(_))), "{p}");
    }
    let sneaky = format!("{}/../x", f.root().display());
    assert!(resolve(f.root(), &sneaky).is_err());
}

#[test]
fn symlink_to_outside_is_refused_for_existing_and_new_targets() {
    let f = Fixture::new();
    let outside = TempDir::new().unwrap();
    std::fs::write(outside.path().join("secret"), "s").unwrap();
    symlink(outside.path(), f.path("out")).unwrap();
    symlink(outside.path().join("secret"), f.path("secret_link")).unwrap();
    for p in ["out", "out/secret", "out/new_file", "out/new_dir/new_file", "secret_link"] {
        assert!(matches!(resolve(f.root(), p), Err(SandboxError::SymlinkOutside(_))), "{p}");
    }
    // Through the tools as well: nothing is read or written outside.
    let read = f.run(&ReadFile, json!({"path": "out/secret"}));
    assert!(read.is_error && read.content.contains("outside the workspace"), "{}", read.content);
    let write = f.run(&WriteFile, json!({"path": "out/planted", "content": "x"}));
    assert!(write.is_error);
    assert!(!outside.path().join("planted").exists());
}

#[test]
fn symlink_chains_are_followed_to_the_end() {
    let f = Fixture::new();
    let outside = TempDir::new().unwrap();
    std::fs::create_dir(f.path("real")).unwrap();
    symlink(f.path("real"), f.path("hop1")).unwrap();
    symlink(f.path("hop1"), f.path("hop2")).unwrap();
    let r = resolve(f.root(), "hop2/file").unwrap();
    assert_eq!(r.abs, f.root().canonicalize().unwrap().join("real/file"));

    // A chain that starts inside and ends outside.
    symlink(outside.path(), f.path("real/away")).unwrap();
    symlink(f.path("real/away"), f.path("hop3")).unwrap();
    assert!(matches!(resolve(f.root(), "hop3/x"), Err(SandboxError::SymlinkOutside(_))));
    // A relative link that climbs out.
    symlink("../../..", f.path("real/up")).unwrap();
    assert!(matches!(resolve(f.root(), "real/up/etc"), Err(SandboxError::SymlinkOutside(_))));
}

#[test]
fn dangling_symlinks_are_refused() {
    let f = Fixture::new();
    symlink("/nonexistent/target/file", f.path("dangling")).unwrap();
    symlink("missing_inside", f.path("dangling_in")).unwrap();
    assert!(matches!(resolve(f.root(), "dangling"), Err(SandboxError::Dangling(_))));
    assert!(matches!(resolve(f.root(), "dangling_in"), Err(SandboxError::Dangling(_))));
    let write = f.run(&WriteFile, json!({"path": "dangling", "content": "x"}));
    assert!(write.is_error);
    assert!(!std::path::Path::new("/nonexistent/target/file").exists());
}

#[test]
fn root_that_is_itself_a_symlink_works() {
    let real = TempDir::new().unwrap();
    let holder = TempDir::new().unwrap();
    let link = holder.path().join("root_link");
    symlink(real.path(), &link).unwrap();
    std::fs::write(real.path().join("a.txt"), "hi\n").unwrap();
    let r = resolve(&link, "a.txt").unwrap();
    assert_eq!(r.abs, real.path().canonicalize().unwrap().join("a.txt"));
    // Absolute paths in either spelling of the root are inside.
    assert_eq!(resolve(&link, link.join("a.txt").to_str().unwrap()).unwrap().rel, "a.txt");
    assert_eq!(resolve(&link, real.path().join("a.txt").to_str().unwrap()).unwrap().rel, "a.txt");
    // A link inside pointing at the real root is inside too.
    symlink(real.path(), real.path().join("self")).unwrap();
    assert_eq!(resolve(&link, "self/a.txt").unwrap().abs, r.abs);
}

#[test]
fn non_utf8_names_resolve_and_stay_checked() {
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;
    let holder = TempDir::new().unwrap();
    let root = holder.path().join(OsStr::from_bytes(b"ro\xffot"));
    std::fs::create_dir(&root).unwrap();
    std::fs::write(root.join("a.txt"), "x").unwrap();
    let r = resolve(&root, "a.txt").unwrap();
    assert!(r.abs.starts_with(root.canonicalize().unwrap()));
    // A non-UTF-8 symlink inside that points outside is refused when reached by a lossy name
    // or not at all; it is never followed.
    let outside = TempDir::new().unwrap();
    symlink(outside.path(), root.join(OsStr::from_bytes(b"l\xfe"))).unwrap();
    assert!(resolve(&root, "l\u{fffd}/x").is_ok_and(|r| r.abs.starts_with(root.canonicalize().unwrap())));
}

#[test]
fn missing_root_is_an_error() {
    let holder = TempDir::new().unwrap();
    let gone = holder.path().join("gone");
    assert!(matches!(resolve(&gone, "a"), Err(SandboxError::BadRoot(_))));
}

#[test]
fn subjects_normalize_lexically() {
    assert_eq!(normalize_subject("./src//a.rs"), "src/a.rs");
    assert_eq!(normalize_subject("src/../lib.rs"), "lib.rs");
    assert_eq!(normalize_subject(""), ".");
    assert_eq!(normalize_subject("../x"), "../x");
    assert_eq!(normalize_subject("/etc/passwd"), "/etc/passwd");
}
