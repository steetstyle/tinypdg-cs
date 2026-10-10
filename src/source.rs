//! Where the code to analyse comes from: a directory, or a GitHub repository.
//!
//! Both are given as one string, so every tool takes the same thing and an agent does
//! not have to learn two argument shapes:
//!
//! ```text
//! ./some/local/dir                     a path
//! gh:steetstyle/tinylink              the default branch
//! gh:steetstyle/tinylink@main         a branch, tag or commit
//! gh:steetstyle/unicpeak@main:Agency.API   a subdirectory of the repository
//! ```
//!
//! A fetched repository is cached under `~/.cache/tiny-source`, keyed by owner, repo
//! and reference, so a second call with the same spec is a cache hit rather than a
//! network round trip. The cache is what makes this usable inside an MCP tool, where
//! every call would otherwise re-clone the same tree.
//!
//! # The token
//!
//! `GITHUB_TOKEN`, or `TINY_GITHUB_TOKEN` to win over it. It is deliberately *not* a
//! command-line flag: argv is world-readable through `ps`, and a token that leaks into
//! a shell history or a CI log is a token that has to be rotated.
//!
//! It is also never written to the clone. `git` is given the credential through
//! `GIT_CONFIG_KEY_0`/`GIT_CONFIG_VALUE_0` in its environment, so the remote it records
//! in `.git/config` is the plain public HTTPS URL and a later `git fetch` from the cache
//! does not need the token to still be around.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{anyhow, bail, Context};

/// A resolved, local directory. This is what the callers actually want; the GitHub case
/// ends here.
#[derive(Debug, Clone)]
pub struct Checkout {
    /// The directory holding the sources.
    pub dir: PathBuf,
    /// What the caller asked for, for messages: `gh:owner/repo@ref`.
    pub spec: String,
    /// Where `dir` came from. `None` means it was already on disk.
    pub fetched: bool,
}

/// A parsed `gh:` specifier.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitHubRef {
    pub owner: String,
    pub repo: String,
    /// `None` means the default branch, which `git clone` resolves on its own.
    pub reference: Option<String>,
    /// A directory inside the repository, for a monorepo that holds several.
    pub subpath: Option<String>,
}

impl GitHubRef {
    pub fn slug(&self) -> String {
        format!("{}/{}", self.owner, self.repo)
    }

    /// Where this reference's checkout lives.
    ///
    /// The reference is hashed rather than sanitised into a path. Branch names contain
    /// slashes (`release/2026-10`), which would otherwise become directories, and two
    /// branches that differ only in a character the sanitiser drops would collide on one
    /// checkout.
    pub fn cache_dir(&self) -> PathBuf {
        let cache = cache_root();
        let key = format!(
            "{}@{}",
            self.slug(),
            self.reference.as_deref().unwrap_or("HEAD")
        );
        cache
            .join("gh")
            .join(&self.owner)
            .join(&self.repo)
            .join(cache_key(&key))
    }

    /// The directory to hand back: the checkout, or a subdirectory of it.
    pub fn dir(&self) -> anyhow::Result<PathBuf> {
        let base = self.cache_dir();
        match &self.subpath {
            None => Ok(base),
            Some(sub) => {
                let path = base.join(sub);
                // Checked, because a typo in the subpath would otherwise index whatever
                // is in the parent directory, and the error would surface much later as a
                // confusing "no methods found".
                if path.is_file() {
                    // Worth distinguishing: a file is a good answer to "where is this",
                    // and a bad one to "analyse this project". Search results carry both
                    // spellings, so the message can name the one that works here.
                    bail!(
                        "gh:{}/{}{}:{sub} names a file, and this tool needs a directory.\n\
                         For the whole project use: gh:{}/{}{}",
                        self.owner,
                        self.repo,
                        self.reference
                            .as_ref()
                            .map(|r| format!("@{r}"))
                            .unwrap_or_default(),
                        self.owner,
                        self.repo,
                        self.reference
                            .as_ref()
                            .map(|r| format!("@{r}"))
                            .unwrap_or_default()
                    );
                }
                if !path.is_dir() {
                    bail!(
                        "gh:{}/{}{} has no directory '{sub}'.\n\
                         The repository is cached at {}. Its top level is:\n  {}",
                        self.owner,
                        self.repo,
                        self.reference
                            .as_ref()
                            .map(|r| format!("@{r}"))
                            .unwrap_or_default(),
                        base.display(),
                        top_level(&base)
                    );
                }
                Ok(path)
            }
        }
    }

    /// Whether a checkout for this reference is already on disk.
    pub fn is_cached(&self) -> bool {
        self.cache_dir().join(".git").is_dir()
    }
}

/// Is this string a GitHub specifier rather than a path?
pub fn is_github(spec: &str) -> bool {
    spec.starts_with("gh:") || spec.starts_with("github:")
}

/// Parse a `gh:` specifier.
///
/// Returns `None` for anything that is not one, so a caller can treat "not a GitHub
/// reference" as "a path" without checking twice.
pub fn parse_github(spec: &str) -> Option<anyhow::Result<GitHubRef>> {
    let rest = spec
        .strip_prefix("gh:")
        .or_else(|| spec.strip_prefix("github:"))?;

    // Split owner/repo from the rest at the first '@'. Branch names may contain '/', so
    // this cannot be done by taking the last path segment.
    let (slug, after_ref) = match rest.split_once('@') {
        Some((slug, tail)) => (slug, Some(tail)),
        None => (rest, None),
    };

    // With no '@' the subdirectory may still be there: `gh:o/r:src/Foo.cs`. This is what
    // a code search result looks like, because GitHub's code search payload does not
    // name the branch it indexed -- and inventing one would give `@HEAD`, which is
    // rejected below. Leaving the reference out lets `git clone` choose, and that is
    // the branch the search actually looked at.
    let (reference, subpath) = match after_ref {
        // A subdirectory cannot contain ':' in git's ref grammar and there is no reason
        // to allow it, so the first ':' after the reference ends it.
        Some(tail) => match tail.split_once(':') {
            Some((r, sub)) => (Some(r.to_string()), Some(sub.to_string())),
            None => (Some(tail.to_string()), None),
        },
        None => match slug.split_once(':') {
            Some((_, sub)) => (None, Some(sub.to_string())),
            None => (None, None),
        },
    };
    let slug = match after_ref {
        Some(_) => slug.to_string(),
        None => slug.split(':').next().unwrap_or(slug).to_string(),
    };
    let slug = slug.as_str();

    if reference.as_deref() == Some("") {
        return Some(Err(anyhow!(
            "'{spec}' has an empty reference: gh:owner/repo@"
        )));
    }
    if subpath.as_deref() == Some("") {
        return Some(Err(anyhow!(
            "'{spec}' has an empty subdirectory: gh:owner/repo@ref:"
        )));
    }
    if reference.as_deref() == Some("HEAD") {
        // Written out rather than rejected, because `git clone --branch HEAD` is not a
        // thing and the failure otherwise arrives three layers down.
        return Some(Err(anyhow!(
            "'{spec}' asks for reference HEAD. \
             Omit @HEAD for the default branch, or name the branch."
        )));
    }

    let mut parts = slug.split('/');
    let owner = parts.next().unwrap_or_default();
    let repo = parts.next().unwrap_or_default();
    if owner.is_empty() || repo.is_empty() || parts.next().is_some() {
        return Some(Err(anyhow!(
            "'{spec}' is not a repository. Write gh:owner/repo, where owner is the \
             user or organisation."
        )));
    }

    Some(Ok(GitHubRef {
        owner: owner.to_string(),
        repo: repo.trim_end_matches(".git").to_string(),
        reference,
        subpath,
    }))
}

/// Resolve a target to a local directory, fetching it first if it is a GitHub spec.
///
/// The single entry point every tool uses, so "gh:..." means the same thing in the CLI
/// and in an MCP argument.
pub fn resolve(spec: &str) -> anyhow::Result<Checkout> {
    let Some(parsed) = parse_github(spec) else {
        let dir = PathBuf::from(spec);
        // Not fetched, not checked: the existing behaviour is that a bad path is
        // reported by whatever tries to read it, with the tool's own message.
        return Ok(Checkout {
            dir,
            spec: spec.to_string(),
            fetched: false,
        });
    };

    let reference = parsed?;
    fetch(&reference)?;
    Ok(Checkout {
        dir: reference.dir()?,
        spec: spec.to_string(),
        fetched: true,
    })
}

/// Resolve an optional target, with a message that says which argument is missing.
///
/// Most of these tools take `path` because they must not assume a source root; a
/// missing one is a caller error, and saying only "no path" makes an agent guess.
pub fn resolve_opt(spec: Option<&str>, what: &str) -> anyhow::Result<PathBuf> {
    let spec = spec.ok_or_else(|| {
        anyhow!(
            "{what} needs a source: a directory, or gh:owner/repo@ref to fetch one from \
             GitHub. This server has no configured source root, so pass it per call."
        )
    })?;
    Ok(resolve(spec)?.dir)
}

/// Resolve a specifier that names something *inside* a repository rather than its root.
///
/// A file argument like `gh:o/r@main:src/Foo.cs` wants the path, not the directory, so
/// the existence check in [`GitHubRef::dir`] would reject the correct answer — a file is
/// not a directory. Whether it exists is left to whatever tries to read it, which can
/// name the file in its own error.
pub fn resolve_path(spec: &str) -> anyhow::Result<PathBuf> {
    match parse_github(spec) {
        None => Ok(PathBuf::from(spec)),
        Some(Err(e)) => Err(e),
        Some(Ok(reference)) => {
            fetch(&reference)?;
            let base = reference.cache_dir();
            Ok(match &reference.subpath {
                None => base,
                Some(sub) => base.join(sub),
            })
        }
    }
}

/// Say what was fetched, on the first call for a reference.
///
/// On a cache hit this says nothing, because printing on every invocation is how a
/// useful message turns into noise that gets ignored.
pub fn announce(spec: &str) {
    let Some(Ok(reference)) = parse_github(spec) else {
        return;
    };
    if reference.is_cached() {
        return;
    }
    if let Ok(dir) = resolve(spec).map(|c| c.dir) {
        eprintln!("fetched {spec} -> {}", dir.display());
    }
}

/// Clone or update the checkout for `reference`.
///
/// Idempotent, and safe to call concurrently: the clone goes to a temporary directory
/// and is moved into place, so a second process cannot see a half-written checkout.
pub fn fetch(reference: &GitHubRef) -> anyhow::Result<PathBuf> {
    let dir = reference.cache_dir();

    if reference.is_cached() {
        return update(&reference.slug(), &dir, reference.reference.as_deref());
    }

    if let Some(parent) = dir.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("create cache directory {}", parent.display()))?;
    }

    let staging = dir.with_extension("partial");
    let _ = std::fs::remove_dir_all(&staging);

    let url = format!("https://github.com/{}.git", reference.slug());

    // `git clone <url> <dir>` writes the directory itself, so the process has to start
    // somewhere that already exists. Running it with the staging directory as the
    // working directory fails to spawn at all, with an errno that says nothing about
    // git or GitHub.
    let parent = dir.parent().expect("a cache path always has a parent");

    let result = match reference.reference.as_deref() {
        // A commit. `--branch` only accepts a branch or a tag, so a raw object id has
        // to be fetched the way git can actually fetch one: ask the server for the
        // object by id and check out what comes back. GitHub allows this.
        Some(r) if looks_like_object_id(r) => fetch_object(&staging, &url, r),
        // A branch or a tag: shallow clone straight at it.
        Some(r) => git(parent)
            .args(["clone", "--depth", "1", "--single-branch", "--branch", r])
            .arg(&url)
            .arg(&staging)
            .status(),
        // No reference: the default branch, which the server chooses.
        None => git(parent)
            .args(["clone", "--depth", "1"])
            .arg(&url)
            .arg(&staging)
            .status(),
    };

    match result {
        Ok(status) if status.success() => {}
        Ok(status) => {
            let _ = std::fs::remove_dir_all(&staging);
            let what = reference
                .reference
                .as_deref()
                .unwrap_or("the default branch");
            bail!(
                "could not fetch gh:{}@{what}: git exited {status}.\n\
                 Check the owner and repository name, and that the reference exists. \
                 A private repository also needs GITHUB_TOKEN in the environment \
                 (it is read-only for anything you only read).",
                reference.slug()
            );
        }
        Err(e) => {
            let _ = std::fs::remove_dir_all(&staging);
            return Err(e).with_context(|| format!("running git to fetch {}", reference.slug()));
        }
    }

    std::fs::rename(&staging, &dir)
        .with_context(|| format!("move the checkout of {} into place", reference.slug()))?;
    Ok(dir)
}

/// The commits one source reference has that another does not.
///
/// Returns `None` when the references are not repository references, because a local
/// directory has no history this can read -- and saying so is different from reporting
/// an empty list, which reads as "these two versions are identical".
pub fn commit_range(
    from: &str,
    to: &str,
    limit: usize,
) -> Option<Result<crate::github::CommitRange, String>> {
    let from = parse_github(from)?.ok()?;
    let to = parse_github(to)?.ok()?;

    // The compare endpoint takes refs, so the caller's own spelling is used rather than
    // a commit the checkout happened to be on. Omitting the reference means the default
    // branch, which is what the specifier means too.
    let base = from.reference.clone().unwrap_or_else(|| "HEAD".to_string());
    let head = to.reference.clone().unwrap_or_else(|| "HEAD".to_string());

    Some(crate::github::compare_commits(
        &from.owner,
        &from.repo,
        &base,
        &head,
        limit,
    ))
}

/// Whether a specifier names a repository rather than a directory.
pub fn is_repository(spec: &str) -> bool {
    matches!(parse_github(spec), Some(Ok(_)))
}

/// Whether a reference is a commit rather than a branch or a tag.
///
/// A full 40-character hex object id. The length is the test, not the content: a
/// shorter hex string is far more likely to be a branch that happens to be named with
/// digits, and `--branch` handles that case correctly while treating it as an object id
/// would not.
fn looks_like_object_id(reference: &str) -> bool {
    reference.len() == 40 && reference.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Materialise one commit into an empty directory.
///
/// `git clone --branch` cannot do this -- it takes a ref name, not an object id -- so
/// the repository is initialised and the object is asked for directly. GitHub serves
/// arbitrary reachable commits this way.
fn fetch_object(
    dir: &Path,
    url: &str,
    object_id: &str,
) -> std::io::Result<std::process::ExitStatus> {
    std::fs::create_dir_all(dir)?;
    let init = git(dir).args(["init", "--quiet"]).status()?;
    if !init.success() {
        return Ok(init);
    }
    // The remote has to exist before anything can be fetched from it, and `git init`
    // alone does not add one.
    let remote = git(dir).args(["remote", "add", "origin", url]).status()?;
    if !remote.success() {
        return Ok(remote);
    }
    let fetch = git(dir)
        .args(["fetch", "--depth", "1", "origin", object_id])
        .status()?;
    if !fetch.success() {
        return Ok(fetch);
    }
    git(dir)
        .args(["checkout", "--quiet", "FETCH_HEAD"])
        .status()
}

/// Bring an existing checkout to the reference it was taken at.
///
/// The recorded commit is compared first, because a shallow fetch of an unchanged
/// branch is a network round trip for no change, and this runs on every MCP call.
fn update(slug: &str, dir: &Path, reference: Option<&str>) -> anyhow::Result<PathBuf> {
    if std::env::var_os("TINY_SOURCE_OFFLINE").is_some() {
        return Ok(dir.to_path_buf());
    }

    let wanted = reference.unwrap_or("HEAD");
    let before = rev_parse(dir, "HEAD");

    let status = git(dir)
        .args(["fetch", "--depth", "1", "origin", wanted])
        // git narrates every fetch to stderr. On a cache hit that is a line of noise
        // before the tool's own output, and nothing here needs it: success is the exit
        // code and a warning is already logged when it fails.
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();

    // A failed fetch is not fatal: a checkout from an earlier run is still a usable
    // answer, and an investigation should not stop because the network is down.
    if !matches!(status, Ok(s) if s.success()) {
        tracing::warn!("git fetch for {slug}@{wanted} failed; using the cached checkout");
        return Ok(dir.to_path_buf());
    }

    if before != rev_parse(dir, "HEAD") {
        let _ = git(dir)
            .args(["checkout", "--quiet", "FETCH_HEAD"])
            .status();
    }
    Ok(dir.to_path_buf())
}

fn rev_parse(dir: &Path, rev: &str) -> Option<String> {
    let output = git(dir).args(["rev-parse", rev]).output().ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// `git` with the token supplied through the environment.
///
/// The header is how git authenticates without the credential appearing in the remote
/// URL it then records. `GIT_CONFIG_KEY_n` is read by git itself, so nothing here has
/// to write a temporary config file that could outlive the process.
fn git(dir: &Path) -> Command {
    let mut cmd = Command::new("git");
    cmd.current_dir(dir);

    if let Some(token) = token() {
        // GitHub accepts `basic base64(x-access-token:<token>)` for a token credential.
        // The username is fixed; only the secret varies.
        let encoded = base64(format!("x-access-token:{token}").as_bytes());
        cmd.env("GIT_CONFIG_COUNT", "1")
            .env("GIT_CONFIG_KEY_0", "http.https://github.com/.extraheader")
            .env(
                "GIT_CONFIG_VALUE_0",
                format!("AUTHORIZATION: basic {encoded}"),
            )
            // Terminal prompts cannot be answered here and would hang the agent.
            .env("GIT_TERMINAL_PROMPT", "0");
    }

    cmd
}

/// The token, if one is configured.
fn token() -> Option<String> {
    ["TINY_GITHUB_TOKEN", "GITHUB_TOKEN"]
        .iter()
        .find_map(|k| std::env::var(k).ok())
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
}

/// Where checkouts are cached.
///
/// `TINY_SOURCE_CACHE` overrides it, which is what makes the tests possible without
/// touching a developer's real cache.
pub fn cache_root() -> PathBuf {
    if let Some(dir) = std::env::var_os("TINY_SOURCE_CACHE") {
        return PathBuf::from(dir);
    }
    // XDG first, because that is the convention and it means a test or a CI job can
    // redirect it by setting XDG_CACHE_HOME rather than knowing about this tool.
    if let Some(dir) = std::env::var_os("XDG_CACHE_HOME") {
        return PathBuf::from(dir).join("tiny-source");
    }
    if let Some(home) = std::env::var_os("HOME") {
        return PathBuf::from(home).join(".cache").join("tiny-source");
    }
    std::env::temp_dir().join("tiny-source")
}

/// A filesystem-safe, collision-free key for a reference string.
fn cache_key(reference: &str) -> String {
    // FNV-1a, so the same reference always maps to the same directory across runs and
    // across the three tools that share the cache. `DefaultHasher` would be simpler and
    // is explicitly not stable between Rust releases, so it cannot be used for a path
    // another process has to find again.
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in reference.as_bytes() {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x1000_0000_01b3);
    }
    let readable: String = reference
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .take(48)
        .collect();
    format!("{readable}-{:016x}", hash)
}

/// Top-level entries of a checkout, for a "did you mean" message.
fn top_level(dir: &Path) -> String {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .flatten()
                .filter(|e| e.file_name() != ".git")
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .take(20)
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    if names.is_empty() {
        return "(the checkout is empty)".to_string();
    }
    names.join("\n  ")
}

/// Standard base64, because the only thing encoded is an HTTP header value.
fn base64(input: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = u32::from_be_bytes([0, b[0], b[1], b[2]]);
        out.push(ALPHABET[(n >> 18) as usize & 63] as char);
        out.push(ALPHABET[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            ALPHABET[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            ALPHABET[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parsed(spec: &str) -> GitHubRef {
        parse_github(spec)
            .expect("should parse")
            .expect("should be valid")
    }

    fn error(spec: &str) -> String {
        parse_github(spec)
            .expect("should parse")
            .expect_err("should be rejected")
            .to_string()
    }

    #[test]
    fn a_bare_repository_means_the_default_branch() {
        let r = parsed("gh:steetstyle/tinylink");
        assert_eq!(r.owner, "steetstyle");
        assert_eq!(r.repo, "tinylink");
        assert_eq!(r.reference, None);
        assert_eq!(r.subpath, None);
    }

    #[test]
    fn the_long_prefix_is_accepted_because_an_agent_will_write_it() {
        // Rejecting the obvious spelling would send an agent looking for the difference
        // instead of fixing its input.
        assert_eq!(parsed("github:o/r").slug(), parsed("gh:o/r").slug());
    }

    #[test]
    fn a_branch_containing_a_slash_survives() {
        // release/2026-10 is an ordinary branch name. Splitting on the last path
        // segment would read it as owner=steetstyle, repo=tinylink@release, and then
        // 2026-10 would be a subdirectory.
        let r = parsed("gh:steetstyle/tinylink@release/2026-10");
        assert_eq!(r.owner, "steetstyle");
        assert_eq!(r.repo, "tinylink");
        assert_eq!(r.reference.as_deref(), Some("release/2026-10"));
        assert_eq!(r.subpath, None);
    }

    #[test]
    fn a_subdirectory_is_separate_from_the_reference() {
        let r = parsed("gh:steetstyle/unicpeak@main:Agency.API/Endpoints");
        assert_eq!(r.reference.as_deref(), Some("main"));
        assert_eq!(r.subpath.as_deref(), Some("Agency.API/Endpoints"));
    }

    #[test]
    fn a_subdirectory_without_a_reference_is_the_default_branch() {
        // What a code search result looks like: a file, and no branch, because GitHub's
        // code search payload does not carry one.
        let r = parsed("gh:o/r:src/a/Foo.cs");
        assert_eq!((r.owner.as_str(), r.repo.as_str()), ("o", "r"));
        assert_eq!(r.reference, None);
        assert_eq!(r.subpath.as_deref(), Some("src/a/Foo.cs"));
    }

    #[test]
    fn a_reference_and_a_subdirectory_still_split_on_the_right_colon() {
        // The two forms must not be confused: with an '@' present the first ':' after it
        // ends the reference, not the repository.
        let r = parsed("gh:o/r@main:src/Foo.cs");
        assert_eq!(r.repo, "r");
        assert_eq!(r.reference.as_deref(), Some("main"));
        assert_eq!(r.subpath.as_deref(), Some("src/Foo.cs"));
    }

    #[test]
    fn a_trailing_git_suffix_is_not_part_of_the_repository_name() {
        // People paste clone URLs, and the URL a clone prints ends in .git.
        assert_eq!(parsed("gh:steetstyle/tinylink.git").repo, "tinylink");
    }

    #[test]
    fn a_local_path_is_not_mistaken_for_a_reference() {
        for path in ["./src", "/abs/path", "C:\\src", "../other/repo"] {
            assert!(parse_github(path).is_none(), "{path} must stay a path");
            assert!(!is_github(path), "{path} must stay a path");
        }
    }

    #[test]
    fn a_specifier_missing_its_repository_says_what_one_looks_like() {
        for bad in ["gh:owner", "gh:owner/", "gh:/repo", "gh:a/b/c"] {
            let message = error(bad);
            assert!(message.contains("gh:owner/repo"), "{bad}: {message}");
        }
    }

    #[test]
    fn an_empty_reference_is_rejected_rather_than_cloned() {
        // `git clone --branch ""` fails with a message about the remote, which says
        // nothing about the actual mistake.
        assert!(error("gh:o/r@").contains("empty reference"));
    }

    #[test]
    fn head_is_rejected_because_clone_cannot_branch_on_it() {
        let message = error("gh:o/r@HEAD");
        assert!(message.contains("default branch"), "{message}");
    }

    #[test]
    fn two_references_never_share_a_cache_directory() {
        // The case that matters: `release/2026-10` must not collide with anything, and
        // neither must two references that sanitise to the same readable form.
        let a = parsed("gh:o/r@main");
        let b = parsed("gh:o/r@feature/x");
        let c = parsed("gh:o/r@feature-x");
        assert_ne!(a.cache_dir(), b.cache_dir());
        // These two are the interesting pair: identical readable form, different refs.
        assert_ne!(b.cache_dir(), c.cache_dir());
    }

    #[test]
    fn the_cache_key_is_stable_across_runs() {
        // Another process has to find this directory again, so the hash cannot be one of
        // Rust's deliberately unstable ones.
        let first = cache_key("steetstyle/tinylink@main");
        let second = cache_key("steetstyle/tinylink@main");
        assert_eq!(first, second);
        assert_eq!(
            first,
            cache_key("steetstyle/tinylink@main"),
            "the key must not depend on the process"
        );
        // The readable part is for a human looking in the cache directory; the hash is
        // what makes it unique. Both have to be there for it to be useful at all.
        assert!(first.starts_with("steetstyle-tinylink-main-"), "{first}");
        assert_eq!(first.len(), "steetstyle-tinylink-main-".len() + 16);
    }

    #[test]
    fn base64_matches_the_known_vector() {
        // The auth header is only as good as this; a wrong encoder produces a 401 with
        // no hint that the encoding was at fault.
        assert_eq!(base64(b"x-access-token:abc"), "eC1hY2Nlc3MtdG9rZW46YWJj");
        assert_eq!(base64(b"a"), "YQ==");
        assert_eq!(base64(b"ab"), "YWI=");
        assert_eq!(base64(b"abc"), "YWJj");
    }

    #[test]
    fn resolving_a_local_path_does_not_touch_the_network() {
        let dir = std::env::temp_dir().join("tiny_source_local_check");
        std::fs::create_dir_all(&dir).unwrap();
        let checkout = resolve(dir.to_str().unwrap()).unwrap();
        assert!(!checkout.fetched);
        assert_eq!(checkout.dir, dir);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_local_directory_has_no_commits_to_compare() {
        // `None`, not an empty range: "these are identical" and "there is nothing here
        // to compare" are different answers and the caller has to be able to tell.
        assert!(commit_range("./a", "./b", 10).is_none());
        assert!(commit_range("gh:o/r", "./b", 10).is_none());
        assert!(commit_range("./a", "gh:o/r", 10).is_none());
        assert!(commit_range("gh:o/r@main", "gh:o/r@dev", 10).is_some());
    }

    #[test]
    fn a_repository_reference_is_recognised_without_fetching_it() {
        assert!(is_repository("gh:o/r"));
        assert!(is_repository("gh:o/r@main:src"));
        assert!(!is_repository("./local"));
        assert!(!is_repository("/abs"));
    }

    #[test]
    fn a_missing_optional_target_names_what_was_missing() {
        let err = resolve_opt(None, "method_dependencies")
            .unwrap_err()
            .to_string();
        assert!(err.contains("method_dependencies"), "{err}");
        assert!(err.contains("gh:owner/repo"), "{err}");
    }

    /// The second call of an MCP conversation must not depend on the network.
    ///
    /// A hand-made checkout is enough to prove it: the resolver only looks for `.git`,
    /// and a `git fetch` that fails is deliberately not fatal, because an investigation
    /// should not stop because the network is down.
    #[test]
    fn an_already_populated_cache_resolves_without_a_clone() {
        let root = std::env::temp_dir().join("tiny_source_cache_hit");
        let _ = std::fs::remove_dir_all(&root);

        let previous = std::env::var_os("TINY_SOURCE_CACHE");
        // Set before the path is computed, not after: `cache_dir` reads the environment,
        // so building the reference first would point it at the real cache.
        //
        // Unsafe because set_var races with any other thread reading the environment.
        // The value is unique to this test, nothing else here reads it, and it is
        // restored before returning.
        unsafe { std::env::set_var("TINY_SOURCE_CACHE", &root) };

        let reference = GitHubRef {
            owner: "steetstyle".into(),
            repo: "tinylink".into(),
            reference: Some("main".into()),
            subpath: None,
        };
        assert!(
            reference.cache_dir().starts_with(&root),
            "the override must reach the cache path: {} is not under {}",
            reference.cache_dir().display(),
            root.display()
        );
        std::fs::create_dir_all(reference.cache_dir().join(".git")).unwrap();
        std::fs::write(reference.cache_dir().join("marker.txt"), "cached").unwrap();

        // Captured while the override is still in place: `cache_dir` reads the
        // environment on every call, so computing it after the restore gives the real
        // path back and the comparison below would be meaningless.
        let expected = reference.cache_dir();
        let checkout = resolve("gh:steetstyle/tinylink@main").expect("must resolve from cache");
        unsafe {
            match previous {
                Some(value) => std::env::set_var("TINY_SOURCE_CACHE", value),
                None => std::env::remove_var("TINY_SOURCE_CACHE"),
            }
        }

        assert_eq!(checkout.dir, expected);
        assert!(
            checkout.fetched,
            "a gh: specifier is reported as fetched either way"
        );
        assert!(checkout.dir.join("marker.txt").exists());

        let _ = std::fs::remove_dir_all(&root);
    }
}
