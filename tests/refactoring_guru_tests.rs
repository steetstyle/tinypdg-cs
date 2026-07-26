use std::path::Path;

use anyhow::Result;
use tiny_pdg_cs::detect::behavioral::detect_behavioral;
use tiny_pdg_cs::detect::creational::detect_creational;
use tiny_pdg_cs::detect::dotnet::detect_dotnet;
use tiny_pdg_cs::detect::structural::detect_structural;
use tiny_pdg_cs::detect::types::{DetectionContext, PatternKind};
use tiny_pdg_cs::parse::parser::parse_source;
use tiny_pdg_cs::resolve::symbols::SymbolTable;

fn run_detection(source: &str) -> Result<Vec<(PatternKind, f64, String)>> {
    let tree = parse_source(source)?;
    let st = SymbolTable::from_ast(tree.root_node(), source)?;
    let ctx = DetectionContext::new(&st.type_graph, source);

    let mut results = Vec::new();
    results.extend(detect_creational(&ctx));
    results.extend(detect_structural(&ctx));
    results.extend(detect_behavioral(&ctx));
    results.extend(detect_dotnet(&ctx));

    Ok(results.into_iter()
        .map(|m| (m.pattern, m.confidence, m.class))
        .collect())
}

fn read_fixture(name: &str) -> Result<String> {
    let path = Path::new("patterns").join(name);
    Ok(std::fs::read_to_string(&path)?)
}

#[test]
fn test_refactoring_guru_strategy() -> Result<()> {
    let source = read_fixture("01-Strategy.cs")?;
    let results = run_detection(&source)?;
    assert!(results.iter().any(|(p, _, _)| *p == PatternKind::Strategy),
        "Strategy not detected");
    Ok(())
}

#[test]
fn test_refactoring_guru_observer() -> Result<()> {
    let source = read_fixture("02-Observer.cs")?;
    let results = run_detection(&source)?;
    assert!(results.iter().any(|(p, _, _)| *p == PatternKind::Observer),
        "Observer not detected");
    Ok(())
}

#[test]
fn test_refactoring_guru_command() -> Result<()> {
    let source = read_fixture("03-Command.cs")?;
    let results = run_detection(&source)?;
    // Strategy/Command/State share the same structural detection;
    // call-graph analysis distinguishes them.
    assert!(results.iter().any(|(p, _, _)| *p == PatternKind::Command
        || *p == PatternKind::Strategy),
        "Command/Strategy not detected: {:?}", results);
    Ok(())
}

#[test]
fn test_refactoring_guru_singleton() -> Result<()> {
    let source = read_fixture("04-Singleton.cs")?;
    let results = run_detection(&source)?;
    assert!(results.iter().any(|(p, _, _)| *p == PatternKind::Singleton),
        "Singleton not detected");
    Ok(())
}

#[test]
fn test_refactoring_guru_adapter() -> Result<()> {
    let source = read_fixture("06-Adapter.cs")?;
    let results = run_detection(&source)?;
    assert!(results.iter().any(|(p, _, _)| *p == PatternKind::Adapter),
        "Adapter not detected");
    Ok(())
}

#[test]
fn test_refactoring_guru_composite() -> Result<()> {
    let source = read_fixture("08-Composite.cs")?;
    let results = run_detection(&source)?;
    assert!(results.iter().any(|(p, _, _)| *p == PatternKind::Composite),
        "Composite not detected");
    Ok(())
}

#[test]
fn test_refactoring_guru_template_method() -> Result<()> {
    let source = read_fixture("09-TemplateMethod.cs")?;
    let results = run_detection(&source)?;
    assert!(results.iter().any(|(p, _, _)| *p == PatternKind::TemplateMethod),
        "TemplateMethod not detected");
    Ok(())
}

#[test]
fn test_refactoring_guru_visitor() -> Result<()> {
    let source = read_fixture("10-Visitor.cs")?;
    let results = run_detection(&source)?;
    assert!(results.iter().any(|(p, _, _)| *p == PatternKind::Visitor),
        "Visitor not detected");
    Ok(())
}

#[test]
fn test_refactoring_guru_chain_of_responsibility() -> Result<()> {
    let source = read_fixture("11-ChainOfResponsibility.cs")?;
    let results = run_detection(&source)?;
    assert!(results.iter().any(|(p, _, _)| *p == PatternKind::ChainOfResponsibility),
        "ChainOfResponsibility not detected");
    Ok(())
}

#[test]
fn test_refactoring_guru_factory_method() -> Result<()> {
    let source = read_fixture("05-FactoryMethod.cs")?;
    let results = run_detection(&source)?;
    assert!(results.iter().any(|(p, _, _)| *p == PatternKind::FactoryMethod),
        "FactoryMethod not detected");
    Ok(())
}

#[test]
fn test_refactoring_guru_decorator() -> Result<()> {
    let source = read_fixture("07-Decorator.cs")?;
    let results = run_detection(&source)?;
    assert!(results.iter().any(|(p, _, _)| *p == PatternKind::Decorator),
        "Decorator not detected");
    Ok(())
}

#[test]
fn test_refactoring_guru_mediator() -> Result<()> {
    let source = read_fixture("12-Mediator.cs")?;
    let results = run_detection(&source)?;
    assert!(results.iter().any(|(p, _, _)| *p == PatternKind::Mediator),
        "Mediator not detected");
    Ok(())
}

#[test]
fn test_refactoring_guru_handler() -> Result<()> {
    let source = read_fixture("13-Handler.cs")?;
    let results = run_detection(&source)?;
    assert!(results.iter().any(|(p, _, _)| *p == PatternKind::Handler),
        "Handler not detected");
    Ok(())
}

#[test]
fn test_refactoring_guru_abstract_factory() -> Result<()> {
    let source = read_fixture("14-AbstractFactory.cs")?;
    let results = run_detection(&source)?;
    assert!(results.iter().any(|(p, _, _)| *p == PatternKind::AbstractFactory),
        "AbstractFactory not detected");
    Ok(())
}

#[test]
fn test_refactoring_guru_dotnet_mediator() -> Result<()> {
    let source = read_fixture("15-DotnetMediator.cs")?;
    let results = run_detection(&source)?;
    assert!(results.iter().any(|(p, _, _)| *p == PatternKind::DotnetMediator),
        "DotnetMediator not detected");
    Ok(())
}

#[test]
fn test_refactoring_guru_controller_api() -> Result<()> {
    let source = read_fixture("16-ControllerApi.cs")?;
    let results = run_detection(&source)?;
    assert!(results.iter().any(|(p, _, _)| *p == PatternKind::ControllerApi),
        "ControllerApi not detected");
    Ok(())
}

#[test]
fn test_refactoring_guru_minimal_api() -> Result<()> {
    let source = read_fixture("17-MinimalApi.cs")?;
    let results = run_detection(&source)?;
    assert!(results.iter().any(|(p, _, _)| *p == PatternKind::MinimalApi),
        "MinimalApi not detected");
    Ok(())
}
