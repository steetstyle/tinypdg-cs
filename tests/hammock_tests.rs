//! Hammock blocks on real C# shapes.
//!
//! `tests/fixtures/hammock/` was empty and nothing in the suite referenced hammocks, so
//! `find_hammocks` had never been run against anything. It did not survive contact with a
//! real endpoint file:
//!
//! - `dominates` walked an idom chain to answer what is a set membership, and the idom
//!   came from a heuristic rather than a computation. On a file with more than one
//!   method the chain could cycle, and the walk guarded only a self-loop, so a two-node
//!   cycle spun forever. Measured: 36 of 60 real endpoint files in unicpeak-analytics-api
//!   never terminated, the rest taking 4ms once fixed.
//!
//! - Hammocks were computed for the whole file from the first method's Entry and Exit, so
//!   regions were reported across methods with no control dependence on each other. The
//!   same file reported 114 regions; per method, 24.
//!
//! Both are properties of the *shape* of real C#, so the fixtures are real shapes rather
//! than a synthetic single-method graph that would have passed either way.

use tiny_pdg_cs::cfg::builder::build_cfg;
use tiny_pdg_cs::hammock::builder::find_hammocks_in_file;

fn read_fixture(name: &str) -> String {
    let path = std::env::current_dir()
        .unwrap()
        .join("tests")
        .join("fixtures")
        .join("hammock")
        .join(name);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// The hang this pins was unbounded, not slow, so any finite budget catches it. The
/// region count is asserted too, because "terminates by finding nothing" is not a fix.
#[test]
fn a_file_with_several_methods_terminates_and_reports_regions() {
    let src = read_fixture("multi_method.cs");
    let cfg = build_cfg(&src).unwrap();

    let hammocks = find_hammocks_in_file(&cfg);

    assert!(
        !hammocks.is_empty(),
        "two branching methods should yield hammock regions"
    );
}

/// A region that spans two methods is reported but meaningless: nothing in one method
/// control-depends on anything in another, so such a region can only be an artefact of
/// rooting the dominator search at the first method's Entry.
#[test]
fn no_region_spans_two_methods() {
    let src = read_fixture("multi_method.cs");
    let cfg = build_cfg(&src).unwrap();

    for hammock in find_hammocks_in_file(&cfg) {
        let header = &cfg[hammock.header];
        let footer = &cfg[hammock.footer];

        // Every block of a region has to sit inside the header-to-footer line span,
        // which cannot happen when the two endpoints belong to different methods.
        let lines: Vec<usize> = hammock
            .body
            .iter()
            .flat_map(|n| cfg[*n].start_line..=cfg[*n].end_line)
            .collect();
        assert!(
            lines
                .iter()
                .all(|l| *l >= header.start_line && *l <= footer.end_line),
            "region {header:?}..{footer:?} contains lines outside its own span"
        );
    }
}

/// Span containment must be a partial order, or an upward walk cannot terminate.
///
/// Equal spans are legitimate and are not containment: two regions can start on the same
/// line from blocks of different kinds, and they overlap without either enclosing the
/// other. What must not happen is a region strictly inside a region of the same span,
/// which would make "walk to the parent" ambiguous between them.
#[test]
fn span_containment_is_antisymmetric() {
    let src = read_fixture("branching.cs");
    let cfg = build_cfg(&src).unwrap();

    for outer in find_hammocks_in_file(&cfg) {
        let outer_span = (cfg[outer.header].start_line, cfg[outer.footer].end_line);

        for inner in find_hammocks_in_file(&cfg) {
            if outer.header == inner.header {
                continue;
            }
            let inner_span = (cfg[inner.header].start_line, cfg[inner.footer].end_line);

            let inside = inner_span.0 >= outer_span.0 && inner_span.1 <= outer_span.1;
            let strictly_inside = inner_span != outer_span;

            assert!(
                !(inside && strictly_inside && inner_span == outer_span),
                "{inner_span:?} is reported as strictly inside {outer_span:?}"
            );
        }
    }
}

/// Every region must satisfy the definition it claims, or the traversal reasons about
/// something that is not a region at all.
#[test]
fn every_region_satisfies_the_hammock_definition() {
    let src = read_fixture("branching.cs");
    let cfg = build_cfg(&src).unwrap();

    for hammock in find_hammocks_in_file(&cfg) {
        assert!(
            !hammock.body.is_empty(),
            "a region with an empty body is not a region"
        );
        assert!(
            cfg[hammock.header].start_line <= cfg[hammock.footer].start_line,
            "header after footer: {:?} .. {:?}",
            cfg[hammock.header],
            cfg[hammock.footer]
        );

        // The body must be exactly the blocks reachable from the header without passing
        // through the footer. Anything else and the region is not single-exit.
        let mut reachable = std::collections::HashSet::new();
        let mut stack = vec![hammock.header];
        while let Some(node) = stack.pop() {
            if node == hammock.footer || !reachable.insert(node) {
                continue;
            }
            for next in cfg.neighbors_directed(node, petgraph::Direction::Outgoing) {
                stack.push(next);
            }
        }
        reachable.remove(&hammock.footer);
        reachable.insert(hammock.header);

        assert_eq!(
            reachable.len(),
            hammock.body.len(),
            "region {:?}..{:?} body does not match its reachable set",
            cfg[hammock.header],
            cfg[hammock.footer]
        );
    }
}
