//! Cloning a repository.

use std::path::Path;

use anyhow::{Context as _, Result, anyhow, bail};
use base::cancel::CancelToken;

use crate::exec::{NETWORK, Tool, redact_credentials};
use crate::pr::gh_available;

/// Clones `url` into the new folder `dest`.
///
/// `dest` must not exist or must be empty; its parent folders are created. A GitHub
/// `owner/name` shorthand is cloned with `gh` when it is installed (so private repositories
/// use its login) and over https otherwise. Never asks for a password: a repository that
/// needs one fails with git's message.
pub fn clone(url: &str, dest: &Path, cancel: &CancelToken) -> Result<()> {
    let url = url.trim();
    if url.is_empty() {
        bail!("nothing to clone");
    }
    if url.starts_with('-') {
        bail!("`{}` is not a repository", redact_credentials(url));
    }
    let existed = dest.exists();
    if existed && !(dest.is_dir() && std::fs::read_dir(dest).is_ok_and(|mut e| e.next().is_none())) {
        bail!("{} already exists", dest.display());
    }
    let dest = std::path::absolute(dest).unwrap_or_else(|_| dest.to_path_buf());
    let parent = dest.parent().ok_or_else(|| anyhow!("{} has no parent folder", dest.display()))?;
    std::fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;

    let result = run_clone(url, &dest, parent, cancel);
    if result.is_err() && !existed {
        // A failed clone can leave an empty folder behind.
        let _ = std::fs::remove_dir_all(&dest);
    }
    result
}

fn run_clone(url: &str, dest: &Path, parent: &Path, cancel: &CancelToken) -> Result<()> {
    let mut gh_error = None;
    let shorthand = github_shorthand(url);
    if let Some(full) = &shorthand
        && gh_available()
    {
        match Tool::gh(Some(parent))
            .args(["repo", "clone", full.as_str()])
            .arg(dest)
            .timeout(NETWORK)
            .check(cancel)
        {
            Ok(_) => return Ok(()),
            Err(e) => gh_error = Some(e),
        }
        if cancel.is_cancelled() {
            return gh_error.map_or(Ok(()), Err);
        }
        let _ = std::fs::remove_dir_all(dest);
    }
    let source = match &shorthand {
        Some(full) => format!("https://github.com/{full}.git"),
        None => url.to_string(),
    };
    let cloned = Tool::git(parent)
        .args(["clone", "--"])
        .arg(&source)
        .arg(dest)
        .timeout(NETWORK)
        .check(cancel);
    match (cloned, gh_error) {
        (Ok(_), _) => Ok(()),
        (Err(e), Some(g)) => Err(anyhow!("{e} (after {g})")),
        (Err(e), None) => Err(e),
    }
}

/// `owner/name` when `text` is a GitHub shorthand and not an existing local path.
fn github_shorthand(text: &str) -> Option<String> {
    if text.contains("://") || text.contains('@') || text.contains(':') || Path::new(text).exists() {
        return None;
    }
    let (owner, name) = text.split_once('/')?;
    let valid = |s: &str| {
        !s.is_empty()
            && !s.starts_with(['.', '-'])
            && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    };
    let name = name.strip_suffix(".git").unwrap_or(name);
    (valid(owner) && valid(name)).then(|| format!("{owner}/{name}"))
}

/// The folder name a clone of `url` gets: the last path segment without `.git`, for https,
/// ssh, scp-style and local paths. `repo` when the URL names nothing.
pub fn repo_name_from_url(url: &str) -> String {
    let mut text = url.trim().trim_end_matches(['/', '\\']);
    text = text.strip_suffix(".git").unwrap_or(text).trim_end_matches(['/', '\\']);
    let last = text.rsplit(['/', ':', '\\']).next().unwrap_or_default();
    if last.is_empty() || last == "." || last == ".." || last.starts_with('-') {
        "repo".to_string()
    } else {
        last.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{Sandbox, git};

    #[test]
    fn repo_names() {
        assert_eq!(repo_name_from_url("https://github.com/acme/payments-api.git"), "payments-api");
        assert_eq!(repo_name_from_url("https://github.com/acme/payments-api"), "payments-api");
        assert_eq!(repo_name_from_url("https://github.com/acme/payments-api/"), "payments-api");
        assert_eq!(repo_name_from_url("git@github.com:acme/api.git"), "api");
        assert_eq!(repo_name_from_url("ssh://git@host:2222/team/sub/x.git"), "x");
        assert_eq!(repo_name_from_url("acme/api"), "api");
        assert_eq!(repo_name_from_url("/srv/git/local.git/"), "local");
        assert_eq!(repo_name_from_url("  https://h/x.git  "), "x");
        assert_eq!(repo_name_from_url(""), "repo");
        assert_eq!(repo_name_from_url("/"), "repo");
    }

    #[test]
    fn shorthand_is_recognised_only_for_owner_name() {
        assert_eq!(github_shorthand("acme/api").as_deref(), Some("acme/api"));
        assert_eq!(github_shorthand("acme/api.git").as_deref(), Some("acme/api"));
        assert_eq!(github_shorthand("https://github.com/acme/api"), None);
        assert_eq!(github_shorthand("git@github.com:acme/api.git"), None);
        assert_eq!(github_shorthand("a/b/c"), None);
        assert_eq!(github_shorthand("-x/y"), None);
        assert_eq!(github_shorthand("/abs/path"), None);
    }

    #[test]
    fn clones_a_local_repository_and_refuses_to_overwrite() {
        let sandbox = Sandbox::new();
        let source = sandbox.repo("source");
        let dest = sandbox.path().join("deep/er/copy");
        clone(source.to_str().unwrap(), &dest, &CancelToken::new()).expect("clone");
        assert!(dest.join("README.md").is_file());
        assert!(git(&dest, &["rev-parse", "--abbrev-ref", "HEAD"]).starts_with("main"));

        let err = clone(source.to_str().unwrap(), &dest, &CancelToken::new()).expect_err("exists");
        assert!(err.to_string().contains("already exists"), "{err}");
    }

    #[test]
    fn a_failed_clone_is_readable_and_leaves_nothing_behind() {
        let sandbox = Sandbox::new();
        let dest = sandbox.path().join("nope");
        let err = clone("/definitely/not/a/repo", &dest, &CancelToken::new()).expect_err("fails");
        let text = err.to_string();
        assert!(text.contains("git clone"), "{text}");
        assert!(!dest.exists());
        assert!(clone("", &dest, &CancelToken::new()).is_err());
        assert!(clone("--upload-pack=x", &dest, &CancelToken::new()).is_err());
    }
}
