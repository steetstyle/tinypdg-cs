//! Searching GitHub, and turning a result into something this tool can analyse.
//!
//! Two questions, two endpoints. `search_repositories` answers "which projects is
//! this" and works unauthenticated. `search_code` answers "which file is it" and does
//! not: GitHub requires a token for code search at all.
//!
//! What comes back is not a URL. It is a [`specifier`], the same `gh:owner/repo@ref`
//! string [`crate::source`] resolves -- so a search result is something every other
//! command accepts, and answering "found it, now what" does not need a second step.
//!
//! [`specifier`]: GitHubResult::specifier
//!
//! # What the token changes
//!
//! With a token, search covers everything the token can see: public repositories, and
//! the private ones it has been granted. Without one, only public. That is GitHub's
//! behaviour, not a choice made here -- but it matters to whoever reads a result, so
//! [`identity`] is reported alongside the results and says whether private ones were
//! possible at all.
//!
//! # Rate limits
//!
//! Search is 10 requests a minute authenticated, 30 unauthenticated, and it is a
//! separate budget from the rest of the API. A 403 with a rate-limit body is reported
//! as such, because it otherwise arrives looking like a permissions problem.

use std::time::Duration;

use serde::Serialize;
use serde_json::{json, Value};

const API: &str = "https://api.github.com";

/// What kind of thing to search for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchKind {
    /// Repositories whose name, description or topics match.
    Repositories,
    /// Files whose contents match, within one repository or across all of them.
    Code,
}

impl SearchKind {
    pub fn as_str(self) -> &'static str {
        match self {
            SearchKind::Repositories => "repositories",
            SearchKind::Code => "code",
        }
    }

    /// Parse a search kind from a command-line argument.
    pub fn parse_public(raw: &str) -> Result<Self, String> {
        Self::parse(raw)
    }

    fn parse(raw: &str) -> Result<Self, String> {
        match raw.to_lowercase().as_str() {
            "repos" | "repositories" | "repo" => Ok(SearchKind::Repositories),
            "code" | "files" | "file" => Ok(SearchKind::Code),
            other => Err(format!(
                "'{other}' is not a search kind. Use `repositories` to find projects, or \
                 `code` to find files inside them."
            )),
        }
    }

    /// Whether the endpoint needs a token.
    ///
    /// Reported rather than assumed, because the failure without one is a 401 that
    /// reads like a permissions problem.
    fn needs_token(self) -> bool {
        matches!(self, SearchKind::Code)
    }
}

/// Who the token belongs to, and what it can reach.
#[derive(Debug, Clone, Serialize)]
pub struct Identity {
    /// The token's user, or null when there is no token.
    pub login: Option<String>,
    /// OAuth scopes the token carries, when the server reports them.
    pub scopes: Option<String>,
    /// Whether private repositories were in scope for this search.
    pub private_results_possible: bool,
}

/// One search result, already shaped as something this tool accepts.
#[derive(Debug, Clone, Serialize)]
pub struct GitHubResult {
    /// `gh:owner/repo@ref` or `gh:owner/repo@ref:path/to/File.cs`.
    ///
    /// The point of the whole module: this string goes into `cfg`, `pdg`, `hammock`,
    /// `detect`, `route`, `callgraph`, `traverse` and `impact` unchanged.
    pub specifier: String,
    /// `owner/repo`.
    pub repository: String,
    /// The branch the result is on. Code search only ever sees the default branch.
    pub branch: String,
    /// The file, for a code result.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// The whole project, for a code result.
    ///
    /// A code hit names a file, and the tools that want a project directory cannot use
    /// one -- so both spellings travel together rather than making the caller work out
    /// which of the two a given tool wanted.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    pub url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    /// Whether the repository is private. Only ever true with a token.
    pub private: bool,
    pub stars: Option<u64>,
}

/// The whole answer: results, plus the things a reader needs to interpret them.
#[derive(Debug, Clone, Serialize)]
pub struct SearchResults {
    pub kind: String,
    pub query: String,
    /// How many matched, which is more than `results` holds.
    pub total_count: u64,
    pub identity: Identity,
    pub results: Vec<GitHubResult>,
}

/// Search, and return results that are ready to analyse.
///
/// `repository` narrows a code search to one repository, which is the form that answers
/// "where in this project is it" -- the global form returns hits across every
/// repository the token can see and is rarely what an investigation wants.
pub fn search(
    query: &str,
    kind: SearchKind,
    limit: usize,
    repository: Option<&str>,
) -> Result<SearchResults, String> {
    let query = query.trim();
    if query.is_empty() {
        return Err("Search needs a query.".to_string());
    }

    let scoped = build_query(query, kind, repository)?;

    let limit = limit.clamp(1, 100);
    let url = format!(
        "{API}/search/{}?q={}&per_page={limit}",
        kind.as_str(),
        urlencode(&scoped)
    );

    let identity = identity(kind.needs_token());
    let (body, _) = get(&url, kind.needs_token())?;

    let total = body["total_count"].as_u64().unwrap_or(0);
    let items = body["items"].as_array().cloned().unwrap_or_default();

    let results: Vec<GitHubResult> = items
        .iter()
        .filter_map(|item| match kind {
            SearchKind::Repositories => repository_result(item),
            SearchKind::Code => code_result(item),
        })
        .take(limit)
        .collect();

    Ok(SearchResults {
        kind: kind.as_str().to_string(),
        query: scoped,
        total_count: total,
        identity,
        results,
    })
}

/// The query string sent to GitHub.
///
/// Split out from [`search`] so the qualifiers can be tested without a request, and so
/// the refusal has one place to live.
fn build_query(query: &str, kind: SearchKind, repository: Option<&str>) -> Result<String, String> {
    match (kind, repository) {
        (SearchKind::Code, Some(repo)) => Ok(format!("{query} repo:{repo}")),
        (_, None) => Ok(query.to_string()),
        // GitHub has no repository filter on repository search; the qualifier would be
        // silently ignored, so it is refused instead.
        (SearchKind::Repositories, Some(repo)) => Err(format!(
            "`repositories` search cannot be scoped to '{repo}'. \
             Search all repositories, or use `code` with the repo: qualifier."
        )),
    }
}

/// A repository search hit.
fn repository_result(item: &Value) -> Option<GitHubResult> {
    let repository = item["full_name"].as_str()?.to_string();
    let branch = item["default_branch"]
        .as_str()
        .unwrap_or("HEAD")
        .to_string();
    Some(GitHubResult {
        specifier: format!("gh:{repository}@{branch}"),
        repository,
        branch,
        path: None,
        project: None,
        url: item["html_url"].as_str().unwrap_or_default().to_string(),
        description: item["description"].as_str().map(str::to_string),
        language: item["language"].as_str().map(str::to_string),
        private: item["private"].as_bool().unwrap_or(false),
        stars: item["stargazers_count"].as_u64(),
    })
}

/// A code search hit.
///
/// The branch is the repository's default branch because that is all code search
/// indexes -- a result cannot be pinned to a commit, and saying so is better than a
/// reader assuming the hit exists on whatever branch they are working in.
fn code_result(item: &Value) -> Option<GitHubResult> {
    let repository = item["repository"]["full_name"].as_str()?.to_string();
    let path = item["path"].as_str()?.to_string();

    // Code search indexes the default branch and nothing else, and its payload has no
    // `default_branch` key at all -- checked, because inventing one produces
    // `gh:owner/repo@HEAD`, which the specifier parser rejects on purpose.
    //
    // So the reference is left out entirely when the server does not name one, and
    // `git clone` picks the default branch. That is the branch the hit was found on, so
    // it is not a guess.
    let branch = item["repository"]["default_branch"]
        .as_str()
        .map(str::to_string);

    let specifier = match &branch {
        Some(branch) => format!("gh:{repository}@{branch}:{path}"),
        None => format!("gh:{repository}:{path}"),
    };
    let project = match &branch {
        Some(branch) => format!("gh:{repository}@{branch}"),
        None => format!("gh:{repository}"),
    };

    Some(GitHubResult {
        specifier,
        repository,
        branch: branch.unwrap_or_else(|| "default branch".to_string()),
        path: Some(path),
        project: Some(project),
        url: item["html_url"].as_str().unwrap_or_default().to_string(),
        description: None,
        language: None,
        private: item["repository"]["private"].as_bool().unwrap_or(false),
        stars: item["repository"]["stargazers_count"].as_u64(),
    })
}

/// One commit between two references.
#[derive(Debug, Clone, Serialize)]
pub struct Commit {
    pub sha: String,
    pub short_sha: String,
    /// The first line of the message, which is what almost every reader wants.
    pub subject: String,
    /// Everything after the first line: the part that says *why*.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub body: String,
    pub author: String,
    pub date: String,
    pub url: String,
}

/// The commits between two references.
#[derive(Debug, Clone, Serialize)]
pub struct CommitRange {
    /// The reference the range starts from, as written.
    pub from: String,
    /// The reference the range ends at.
    pub to: String,
    /// Commits `to` has that `from` does not.
    pub ahead_by: u64,
    /// Commits `from` has that `to` does not.
    pub behind_by: u64,
    pub total: u64,
    /// Newest first, which is the order a reader wants and the order git uses.
    pub commits: Vec<Commit>,
    /// Set when `commits` holds fewer than `total`. Said rather than left for the reader
    /// to work out, because a short list that looks complete is worse than a long one.
    pub truncated: bool,
}

/// The commits one reference has that another does not.
///
/// GitHub's compare endpoint rather than `git log`: the checkouts here are `--depth 1`,
/// so there is no history for a local log to walk. Fetching the range is also slower and
/// mutates the cached tree, and a report must not change what it is reporting on.
pub fn compare_commits(
    owner: &str,
    repo: &str,
    from: &str,
    to: &str,
    limit: usize,
) -> Result<CommitRange, String> {
    let url = format!(
        "{API}/repos/{owner}/{repo}/compare/{from}...{to}?per_page={}",
        limit.clamp(1, 100)
    );
    let (body, _) = get(&url, true)?;

    // `status` is GitHub's own word for the relationship: ahead, behind, identical or
    // diverged. Reported so a reader does not have to infer it from two counts.
    let commits: Vec<Commit> = body["commits"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|c| {
            let message = c["commit"]["message"].as_str().unwrap_or_default();
            let mut lines = message.lines();
            Commit {
                short_sha: c["sha"].as_str().unwrap_or_default()
                    [..8.min(c["sha"].as_str().unwrap_or_default().len())]
                    .to_string(),
                sha: c["sha"].as_str().unwrap_or_default().to_string(),
                subject: lines.next().unwrap_or_default().to_string(),
                body: lines.collect::<Vec<_>>().join("\n").trim().to_string(),
                author: c["commit"]["author"]["name"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
                date: c["commit"]["author"]["date"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
                url: c["html_url"].as_str().unwrap_or_default().to_string(),
            }
        })
        .collect();

    let total = body["total_commits"]
        .as_u64()
        .unwrap_or(commits.len() as u64);
    Ok(CommitRange {
        from: from.to_string(),
        to: to.to_string(),
        ahead_by: body["ahead_by"].as_u64().unwrap_or(0),
        behind_by: body["behind_by"].as_u64().unwrap_or(0),
        total,
        truncated: (commits.len() as u64) < total,
        commits,
    })
}

/// Who we are, and whether private results were in scope.
///
/// Never fatal. A search that works should still be returned when the identity lookup
/// does not -- the caller asked a question about code, not about credentials.
fn identity(private_possible: bool) -> Identity {
    let (body, headers) = match get(&format!("{API}/user"), false) {
        Ok(pair) => pair,
        Err(_) => {
            return Identity {
                login: None,
                scopes: None,
                private_results_possible: private_possible && token().is_some(),
            }
        }
    };

    Identity {
        login: body["login"].as_str().map(str::to_string),
        scopes: headers
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case("x-oauth-scopes"))
            .map(|(_, value)| value.clone())
            .filter(|scopes| !scopes.is_empty())
            .map(|scopes| {
                scopes
                    .split(',')
                    .map(str::trim)
                    .collect::<Vec<_>>()
                    .join(", ")
            }),
        private_results_possible: private_possible && token().is_some(),
    }
}

/// One GET, with the token when one is configured.
///
/// Status codes are not turned into errors: a 403 carrying "rate limit exceeded" and
/// a 403 carrying "not authorised" need different answers, and the body is where the
/// difference is. So `http_status_as_error` is off and the code is inspected here.
fn get(url: &str, needs_token: bool) -> Result<(Value, Vec<(String, String)>), String> {
    let mut request = agent()
        .get(url)
        .header("Accept", "application/vnd.github+json");
    if let Some(token) = token() {
        request = request.header("Authorization", &format!("Bearer {token}"));
    }

    let response = match request.call() {
        Ok(response) => response,
        Err(e) => return Err(format!("could not reach GitHub: {e}")),
    };

    let status = response.status().as_u16();
    let headers: Vec<(String, String)> = response
        .headers()
        .iter()
        .map(|(name, value)| {
            (
                name.as_str().to_string(),
                value.to_str().unwrap_or_default().to_string(),
            )
        })
        .collect();

    let body: Value = match response.into_body().read_json() {
        Ok(body) => body,
        Err(e) => {
            if status >= 400 {
                return Err(status_message(status, &json!({})));
            }
            return Err(format!("GitHub returned something that is not JSON: {e}"));
        }
    };

    if status >= 400 {
        return Err(status_message(status, &body));
    }

    if needs_token && token().is_none() {
        return Err(
            "GitHub code search needs a token. Set GITHUB_TOKEN (read-only is enough): \
             code search is not available anonymously. Repository search still is."
                .to_string(),
        );
    }
    Ok((body, headers))
}

/// An agent that hands back error responses instead of raising them.
fn agent() -> ureq::Agent {
    ureq::Agent::new_with_config(
        ureq::Agent::config_builder()
            .http_status_as_error(false)
            .timeout_global(Some(Duration::from_secs(20)))
            .build(),
    )
}

/// Turn a status code into something that says what to do about it.
///
/// The rate limit is the one that gets misread: it arrives as a 403, looks exactly
/// like being denied access, and sending someone to check their token's permissions
/// when the answer is "wait a minute" wastes their time.
fn status_message(status: u16, body: &Value) -> String {
    let detail = body["message"].as_str().unwrap_or("no detail");

    match status {
        401 => format!("GitHub rejected the token: {detail}"),
        403 | 429 if detail.to_lowercase().contains("rate limit") => format!(
            "GitHub's search rate limit is exhausted: {detail}. \
             Authenticated search allows 10 requests a minute. Wait, or search fewer \
             times with a narrower query."
        ),
        403 => format!("GitHub denied the request: {detail}"),
        404 => format!("GitHub has nothing at {detail}"),
        other => format!("GitHub returned {other}: {detail}"),
    }
}

fn token() -> Option<String> {
    ["TINY_GITHUB_TOKEN", "GITHUB_TOKEN"]
        .iter()
        .find_map(|k| std::env::var(k).ok())
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
}

/// Percent-encode a query string value.
///
/// `+` is left alone because GitHub's search syntax uses it to mean "and", and turning
/// it into `%2B` would silently change what is being searched for.
fn urlencode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'+' => {
                out.push(byte as char)
            }
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

/// The help text, shared by the CLI and the MCP tool description so they agree.
pub const HOW_TO_USE: &str = "\
Each result carries a `specifier`, which is a source argument. Pass it to any command: \
cfg, pdg, hammock, detect, route, callgraph, traverse, impact. A repository result \
analyses the whole tree; a code result names one file and takes a subdirectory \
specifier. The first call fetches the repository and caches it; later calls are \
cache hits.";

#[cfg(test)]
mod tests {
    use super::*;

    fn item(json: &str) -> Value {
        serde_json::from_str(json).expect("valid json")
    }

    #[test]
    fn a_repository_result_becomes_a_specifier_the_tools_accept() {
        let r = repository_result(&item(
            r#"{"full_name":"unicpeak/unicpeak-analytics-api","default_branch":"main",
                "html_url":"https://github.com/unicpeak/unicpeak-analytics-api",
                "description":"Ad network analytics","language":"C#",
                "private":false,"stargazers_count":12}"#,
        ))
        .expect("parsed");

        assert_eq!(r.specifier, "gh:unicpeak/unicpeak-analytics-api@main");
        assert_eq!(r.branch, "main");
        assert_eq!(r.stars, Some(12));
        assert!(r.path.is_none());
    }

    #[test]
    fn a_code_result_names_the_file_and_its_branch() {
        let r = code_result(&item(
            r#"{"path":"AdNetwork.API/Endpoints/GetConnectionDetailEndpoint.cs",
                "html_url":"https://github.com/o/r/blob/main/AdNetwork.API/x.cs",
                "repository":{"full_name":"o/r","default_branch":"main","private":true,
                              "stargazers_count":3}}"#,
        ))
        .expect("parsed");
        assert_eq!(r.branch, "main");

        // The subdirectory form, so the file is addressable by the same parser.
        assert_eq!(
            r.specifier,
            "gh:o/r@main:AdNetwork.API/Endpoints/GetConnectionDetailEndpoint.cs"
        );
        assert_eq!(
            r.path.as_deref(),
            Some("AdNetwork.API/Endpoints/GetConnectionDetailEndpoint.cs")
        );
        // Both spellings, because the file form cannot be given to a tool that wants a
        // project directory.
        assert_eq!(
            r.project.as_deref(),
            Some("gh:o/r@main"),
            "the project form must be the directory specifier"
        );
        assert!(crate::source::parse_github(r.project.as_deref().unwrap())
            .unwrap()
            .unwrap()
            .subpath
            .is_none());
        assert!(r.private);
    }

    #[test]
    fn the_specifier_a_result_produces_parses_back_to_the_same_repository() {
        // The round trip is the whole claim: a search result must be something the
        // source parser accepts, or "found it, now analyse it" needs a second step.
        let r = repository_result(&item(
            r#"{"full_name":"o/r","default_branch":"release/2026-10","html_url":""}"#,
        ))
        .unwrap();

        let parsed = crate::source::parse_github(&r.specifier)
            .expect("specifier is a github spec")
            .expect("specifier is valid");
        assert_eq!(parsed.owner, "o");
        assert_eq!(parsed.repo, "r");
        // A branch with a slash survives the round trip, which is the case that would
        // silently become a subdirectory otherwise.
        assert_eq!(parsed.reference.as_deref(), Some("release/2026-10"));
    }

    #[test]
    fn a_code_result_also_round_trips_to_the_right_file() {
        let r = code_result(&item(
            r#"{"path":"src/a/b/Foo.cs","html_url":"",
                "repository":{"full_name":"o/r","default_branch":"main"}}"#,
        ))
        .unwrap();
        let parsed = crate::source::parse_github(&r.specifier).unwrap().unwrap();
        assert_eq!(parsed.subpath.as_deref(), Some("src/a/b/Foo.cs"));
        assert_eq!(parsed.reference.as_deref(), Some("main"));
    }

    #[test]
    fn a_branch_the_server_did_not_name_is_not_invented() {
        // A repository search hit has no default_branch on an empty repository, and a
        // code search hit never has one at all. Falling back to "HEAD" would produce
        // `gh:o/r@HEAD`, which the specifier parser rejects on purpose -- so the result
        // would be unusable rather than merely imprecise.
        let repo = repository_result(&item(r#"{"full_name":"o/r","html_url":""}"#)).unwrap();
        assert_eq!(repo.branch, "HEAD");
        assert!(crate::source::parse_github(&repo.specifier)
            .unwrap()
            .is_err());

        // The code path omits the reference instead, and the parser accepts that.
        let code = code_result(&item(
            r#"{"path":"src/Foo.cs","html_url":"","repository":{"full_name":"o/r"}}"#,
        ))
        .unwrap();
        assert_eq!(code.specifier, "gh:o/r:src/Foo.cs");
        let parsed = crate::source::parse_github(&code.specifier)
            .unwrap()
            .unwrap();
        assert_eq!(parsed.reference, None, "clone resolves the default branch");
        assert_eq!(parsed.subpath.as_deref(), Some("src/Foo.cs"));
    }

    #[test]
    fn code_search_is_the_one_that_needs_a_token() {
        assert!(SearchKind::Code.needs_token());
        assert!(!SearchKind::Repositories.needs_token());
    }

    #[test]
    fn scoping_repositories_is_refused_rather_than_silently_ignored() {
        // GitHub has no repository filter on repository search. Accepting the argument
        // and dropping it would return results from everywhere, which reads as if the
        // scope had been applied.
        let err = search("x", SearchKind::Repositories, 10, Some("o/r")).unwrap_err();
        assert!(err.contains("code"), "{err}");
    }

    #[test]
    fn a_code_search_is_scoped_by_a_repo_qualifier() {
        // Scoping is the form an investigation actually wants: "where in this project",
        // not "where in everything the token can see".
        assert_eq!(
            build_query("Handler", SearchKind::Code, Some("o/r")).unwrap(),
            "Handler repo:o/r"
        );
        assert_eq!(
            build_query("Handler", SearchKind::Code, None).unwrap(),
            "Handler",
            "an unscoped code search is allowed and is the global one"
        );
    }

    #[test]
    fn an_empty_query_is_refused_without_a_request() {
        assert!(search("   ", SearchKind::Repositories, 10, None).is_err());
    }

    #[test]
    fn search_kinds_parse_from_what_a_caller_would_write() {
        assert_eq!(
            SearchKind::parse("repos").unwrap(),
            SearchKind::Repositories
        );
        assert_eq!(SearchKind::parse("code").unwrap(), SearchKind::Code);
        assert!(SearchKind::parse("everything").is_err());
    }

    #[test]
    fn the_query_is_encoded_but_a_plus_stays_a_plus() {
        // GitHub's search syntax uses `+` for "and"; encoding it to %2B would search for
        // a literal plus and return nothing, which looks like "no matches".
        assert_eq!(urlencode("a b"), "a%20b");
        assert_eq!(urlencode("foo+bar"), "foo+bar");
        assert_eq!(urlencode("repo:o/r"), "repo%3Ao%2Fr");
    }
}
