//! What gets embedded, and what does not.
//!
//! Test code is excluded by default, and that is a deliberate choice rather than a
//! preference. Measured on a real repository: 384 of 1,667 C# files are tests, and
//! without this they dominate the results — the query "charge a customer's credit
//! balance" returned `AddCreditCommandTests.Should_InsertCreditTransaction_WhenTopUp`
//! from four different models, because a test method's name is a *sentence describing
//! the behaviour* ("should insert credit transaction when top up") and that is a better
//! match for a task sentence than any production method's name is. The retrieval was
//! working; it was retrieving the tests, which say what should happen rather than what
//! does.
//!
//! Excluding them makes the answer about the code that runs. `include_tests` brings
//! them back, for the case where the task is about the tests.
//!
//! Detection uses three signals, because any one of them alone misses something in some
//! repository:
//!
//! * **path** — a `tests` or `*.Tests` directory. On this repository it finds all 384,
//!   and the namespace signal finds none the path misses.
//! * **namespace** — the case where a project keeps tests beside production code, which
//!   is common and which the path cannot see.
//! * **type name** — `*Tests`, `*Test`, `*Fixture`. Catches a test class that lives in
//!   a production folder, which is where a repository drifts to over time.
//!
//! What this does *not* do is look for `[Fact]` and `[Theory]` attributes. It would be
//! the most direct signal and it is not implemented, because the parser does not carry
//! attributes through today. Saying so is better than implying better coverage than
//! there is.

use serde::{Deserialize, Serialize};

/// The default test-path pattern, applied to the path with forward slashes.
const TEST_PATH_SEGMENTS: [&str; 6] = ["test", "tests", "testing", "spec", "specs", "fixtures"];

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Filter {
    /// Keep test code. Off by default, for the reason on this module.
    #[serde(default)]
    pub include_tests: bool,
    /// Keep only paths containing one of these. Empty means no restriction.
    #[serde(default)]
    pub include: Vec<String>,
    /// Drop paths containing any of these. Applied after `include`.
    #[serde(default)]
    pub exclude: Vec<String>,
}

impl Filter {
    /// Keep everything.
    pub fn all() -> Self {
        Self {
            include_tests: true,
            ..Default::default()
        }
    }

    /// Drop paths containing any of `patterns`.
    pub fn excluding(patterns: &[&str]) -> Self {
        Self {
            exclude: patterns.iter().map(|p| p.to_string()).collect(),
            ..Default::default()
        }
    }

    /// Why a path was dropped, or `None` to keep it.
    ///
    /// Returning the reason rather than a bare bool is what lets the indexer report
    /// "excluded 1,204 symbols: test code" instead of quietly producing a smaller
    /// index, which reads exactly like a repository that has no tests.
    pub fn rejection(&self, file: &str, namespace: &str, type_name: &str) -> Option<String> {
        let path = file.replace('\\', "/");

        if !self.include.is_empty() && !self.include.iter().any(|p| path.contains(p.as_str())) {
            return Some(format!("outside --include ({})", self.include.join(", ")));
        }
        for pattern in &self.exclude {
            if path.contains(pattern.as_str()) {
                return Some(format!("matches --exclude {pattern}"));
            }
        }
        if !self.include_tests && is_test(&path, namespace, type_name) {
            return Some("test code".to_string());
        }
        None
    }

    /// Keep or drop, without the reason.
    pub fn keeps(&self, file: &str, namespace: &str, type_name: &str) -> bool {
        self.rejection(file, namespace, type_name).is_none()
    }
}

/// Whether one file or type is test code.
pub fn is_test(path: &str, namespace: &str, type_name: &str) -> bool {
    path_says_test(path) || namespace_says_test(namespace) || name_says_test(type_name)
}

fn path_says_test(path: &str) -> bool {
    let path = path.replace('\\', "/").to_lowercase();
    let segments: Vec<&str> = path.split('/').collect();

    // A path *segment* that is a test directory, rather than a substring anywhere: a
    // project called `Testbed` or `Contest.API` is not a test suite, and a substring
    // rule would swallow it.
    if segments.iter().any(|s| TEST_PATH_SEGMENTS.contains(s)) {
        return true;
    }
    // `Foo.Tests.Bar` as a directory, which is this repository's convention.
    segments.iter().any(|s| {
        s.split('.')
            .skip(1)
            .any(|part| TEST_PATH_SEGMENTS.contains(&part))
    })
}

fn namespace_says_test(namespace: &str) -> bool {
    let lower = namespace.to_lowercase();
    lower
        .split('.')
        .any(|part| TEST_PATH_SEGMENTS.contains(&part))
}

/// `AddCreditCommandTests`, `SomeFixture`, `FooTest`.
fn name_says_test(type_name: &str) -> bool {
    let lower = type_name.to_lowercase();
    // A type called exactly `Test` or `Tests` counts; so does one ending in it.
    lower == "test"
        || lower == "tests"
        || lower.ends_with("tests")
        || lower.ends_with("test")
        || lower.ends_with("fixture")
        || lower.ends_with("spec")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn production_code_is_kept() {
        let f = Filter::default();
        for path in [
            "src/Agency.API/Endpoints/Members/GetAgencyMembersEndpoint.cs",
            "src/Billing.Database/Models/CreditTransaction.cs",
        ] {
            assert!(
                f.keeps(path, "Billing.Database", "CreditTransaction"),
                "{path}"
            );
        }
    }

    /// The case that started this: a test method's name is a sentence, so retrieval
    /// finds it easily. It has to be dropped by default.
    #[test]
    fn test_code_is_dropped_by_default() {
        let f = Filter::default();
        assert!(!f.keeps(
            "src/Billing.Processing.Tests/Commands/AddCreditCommandTests.cs",
            "Billing.Processing.Tests",
            "AddCreditCommandTests"
        ));
    }

    /// `include_tests` is the way back, for a task that is about the tests.
    #[test]
    fn include_tests_keeps_them() {
        let f = Filter::all();
        assert!(f.keeps("src/Foo.Tests/BarTests.cs", "Foo.Tests", "BarTests"));
    }

    /// A segment, not a substring: these are production projects whose names merely
    /// contain the letters, and a substring rule would drop them.
    #[test]
    fn a_name_that_merely_contains_test_is_not_a_test() {
        let f = Filter::default();
        for path in [
            "src/Testbed.API/Handler.cs",
            "src/Contest.API/Handler.cs",
            "src/Latest.API/Handler.cs",
            "src/Protests.API/Handler.cs",
        ] {
            assert!(
                f.keeps(path, "Testbed.API", "Handler"),
                "{path} was dropped as a test"
            );
        }
    }

    /// A test class that lives in a production folder. Common after a repository drifts,
    /// and the path cannot see it.
    #[test]
    fn a_test_type_in_a_production_folder_is_still_dropped() {
        let f = Filter::default();
        assert!(!f.keeps(
            "src/Billing.Processing/Commands/AddCreditCommandTests.cs",
            "Billing.Processing.Commands",
            "AddCreditCommandTests"
        ));
    }

    /// The namespace-only case: production path, test namespace.
    #[test]
    fn a_test_namespace_in_a_production_folder_is_dropped() {
        let f = Filter::default();
        assert!(!f.keeps("src/Billing/CreditTests.cs", "Billing.Tests", "CreditTests"));
    }

    /// This repository's convention, `Foo.Tests.Bar` as a directory.
    #[test]
    fn a_dotted_test_directory_segment_is_dropped() {
        let f = Filter::default();
        assert!(!f.keeps(
            "src/Shared.Analytics.Tests/Attribution/Scorer.cs",
            "Shared.Analytics.Tests",
            "Scorer"
        ));
    }

    /// Windows separators, because a path is a path and this tool runs on both.
    #[test]
    fn backslash_paths_are_understood() {
        let f = Filter::default();
        assert!(!f.keeps(
            "src\\Billing.Processing.Tests\\Commands\\AddCreditCommandTests.cs",
            "",
            "AddCreditCommandTests"
        ));
    }

    #[test]
    fn include_narrows_to_matching_paths() {
        let f = Filter {
            include: vec!["Agency.API".into()],
            ..Default::default()
        };
        assert!(f.keeps("src/Agency.API/A.cs", "Agency.API", "A"));
        assert!(
            !f.keeps("src/Billing.API/B.cs", "Billing.API", "B"),
            "outside --include must be dropped"
        );
        assert_eq!(
            f.rejection("src/Billing.API/B.cs", "Billing.API", "B"),
            Some("outside --include (Agency.API)".to_string())
        );
    }

    #[test]
    fn exclude_wins_over_include() {
        let f = Filter {
            include: vec!["API".into()],
            exclude: vec!["AdNetwork".into()],
            ..Default::default()
        };
        assert!(f.keeps("src/Agency.API/A.cs", "Agency.API", "A"));
        assert_eq!(
            f.rejection("src/AdNetwork.API/B.cs", "AdNetwork.API", "B"),
            Some("matches --exclude AdNetwork".to_string())
        );
    }

    /// The rejection reason has to say why, or a smaller index reads like a repository
    /// that happens to have no tests.
    #[test]
    fn every_rejection_says_why() {
        let f = Filter::default();
        let reasons = [
            f.rejection("src/Foo.Tests/A.cs", "", "A"),
            f.rejection("src/A.cs", "", "ARepoTests"),
            (Filter {
                include: vec!["X".into()],
                ..Default::default()
            })
            .rejection("src/A.cs", "", "A"),
        ];
        for r in reasons.into_iter().flatten() {
            assert!(!r.is_empty(), "an empty reason tells nobody anything");
        }
    }

    /// The default has to mean what the docs say.
    #[test]
    fn the_default_drops_tests_and_keeps_everything_else() {
        let f = Filter::default();
        assert!(!f.include_tests);
        assert!(f.include.is_empty());
        assert!(f.exclude.is_empty());
        assert!(f.keeps("src/Agency.API/A.cs", "Agency.API", "A"));
        assert!(!f.keeps("src/Agency.API/ATests.cs", "Agency.API", "ATests"));
    }

    /// Not implemented on purpose, so it is stated in the code as well as the docs.
    #[test]
    fn the_name_test_is_honest_about_what_it_is() {
        // A type that looks nothing like a test but sits in a test directory is caught
        // by the path, and one that looks like a test in production is caught by the
        // name. Neither depends on attributes.
        assert!(is_test("src/Foo.Tests/Scorer.cs", "", "Scorer"));
        assert!(is_test("src/Foo/Scorer.cs", "", "ScorerTests"));
        assert!(!is_test("src/Foo/Scorer.cs", "", "Scorer"));
    }
}
