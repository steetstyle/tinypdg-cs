use tiny_pdg_cs::cfg::builder::build_cfg;
use tiny_pdg_cs::graph::dot::cfg_to_dot;

fn read_fixture(parts: &[&str]) -> String {
    let base = std::env::current_dir().unwrap();
    let mut path = base.join("tests").join("fixtures");
    for p in parts {
        path = path.join(p);
    }
    std::fs::read_to_string(path).expect("fixture not found")
}

#[test]
fn golden_if_else() {
    let src = read_fixture(&["control_flow", "if_else.cs"]);
    let g = build_cfg(&src).unwrap();
    insta::assert_snapshot!("if_else", cfg_to_dot(&g));
}

#[test]
fn golden_switch() {
    let src = read_fixture(&["control_flow", "switch.cs"]);
    let g = build_cfg(&src).unwrap();
    insta::assert_snapshot!("switch", cfg_to_dot(&g));
}

#[test]
fn golden_loops() {
    let src = read_fixture(&["control_flow", "loops.cs"]);
    let g = build_cfg(&src).unwrap();
    insta::assert_snapshot!("loops", cfg_to_dot(&g));
}

#[test]
fn golden_try_catch() {
    let src = read_fixture(&["exceptions", "try_catch.cs"]);
    let g = build_cfg(&src).unwrap();
    insta::assert_snapshot!("try_catch", cfg_to_dot(&g));
}

#[test]
fn golden_try_finally() {
    let src = read_fixture(&["exceptions", "try_finally.cs"]);
    let g = build_cfg(&src).unwrap();
    insta::assert_snapshot!("try_finally", cfg_to_dot(&g));
}
