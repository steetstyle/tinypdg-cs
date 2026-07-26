fn fixture_path(parts: &[&str]) -> String {
    let base = std::env::current_dir().unwrap();
    let mut path = base.join("tests").join("fixtures");
    for p in parts {
        path = path.join(p);
    }
    path.to_string_lossy().to_string()
}

fn read_fixture(parts: &[&str]) -> String {
    std::fs::read_to_string(fixture_path(parts)).expect("fixture not found")
}

#[test]
fn test_parse_if_else_fixture() {
    let src = read_fixture(&["control_flow", "if_else.cs"]);
    let tree = tiny_pdg_cs::parse::parser::parse_source(&src).unwrap();
    let methods = tiny_pdg_cs::parse::visitor::find_methods(&tree);
    assert_eq!(methods.len(), 1);
}

#[test]
fn test_cfg_if_else_fixture() {
    let src = read_fixture(&["control_flow", "if_else.cs"]);
    let g = tiny_pdg_cs::cfg::builder::build_cfg(&src).unwrap();
    assert!(g.node_count() >= 4);
}

#[test]
fn test_cfg_switch_fixture() {
    let src = read_fixture(&["control_flow", "switch.cs"]);
    let g = tiny_pdg_cs::cfg::builder::build_cfg(&src).unwrap();
    assert!(g.node_count() >= 4);
}

#[test]
fn test_cfg_loops_fixture() {
    let src = read_fixture(&["control_flow", "loops.cs"]);
    let g = tiny_pdg_cs::cfg::builder::build_cfg(&src).unwrap();
    assert!(g.node_count() >= 3);
}

#[test]
fn test_cfg_try_catch_fixture() {
    let src = read_fixture(&["exceptions", "try_catch.cs"]);
    let g = tiny_pdg_cs::cfg::builder::build_cfg(&src).unwrap();
    assert!(g.node_count() >= 4);
}

#[test]
fn test_cfg_try_finally_fixture() {
    let src = read_fixture(&["exceptions", "try_finally.cs"]);
    let g = tiny_pdg_cs::cfg::builder::build_cfg(&src).unwrap();
    assert!(g.node_count() >= 4);
}
