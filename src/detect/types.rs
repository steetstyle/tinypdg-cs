use std::fmt;

use crate::analysis::callgraph::CallGraph;
use crate::resolve::types::TypeGraph;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum PatternKind {
    Singleton, FactoryMethod, AbstractFactory, Builder, Prototype,
    Adapter, Decorator, Proxy, Composite, Facade, Bridge, Flyweight,
    Strategy, Observer, Command, Mediator, ChainOfResponsibility, State,
    TemplateMethod, Visitor, Iterator, Memento, Interpreter, Handler,
    DotnetMediator, ControllerApi, MinimalApi, DiContainer,
}

impl fmt::Display for PatternKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", match self {
            PatternKind::Singleton => "Singleton",
            PatternKind::FactoryMethod => "FactoryMethod",
            PatternKind::AbstractFactory => "AbstractFactory",
            PatternKind::Builder => "Builder",
            PatternKind::Prototype => "Prototype",
            PatternKind::Adapter => "Adapter",
            PatternKind::Decorator => "Decorator",
            PatternKind::Proxy => "Proxy",
            PatternKind::Composite => "Composite",
            PatternKind::Facade => "Facade",
            PatternKind::Bridge => "Bridge",
            PatternKind::Flyweight => "Flyweight",
            PatternKind::Strategy => "Strategy",
            PatternKind::Observer => "Observer",
            PatternKind::Command => "Command",
            PatternKind::Mediator => "Mediator",
            PatternKind::ChainOfResponsibility => "ChainOfResponsibility",
            PatternKind::State => "State",
            PatternKind::TemplateMethod => "TemplateMethod",
            PatternKind::Visitor => "Visitor",
            PatternKind::Iterator => "Iterator",
            PatternKind::Memento => "Memento",
            PatternKind::Interpreter => "Interpreter",
            PatternKind::Handler => "Handler",
            PatternKind::DotnetMediator => "DotnetMediator",
            PatternKind::ControllerApi => "ControllerApi",
            PatternKind::MinimalApi => "MinimalApi",
            PatternKind::DiContainer => "DiContainer",
        })
    }
}

#[derive(Debug, Clone)]
pub struct PatternMatch {
    pub pattern: PatternKind,
    pub class: String,
    pub description: String,
    pub confidence: f64,
    pub participants: Vec<String>,
    pub evidence: Vec<String>,
}

pub struct DetectionContext<'a> {
    pub type_graph: &'a TypeGraph,
    pub callgraph: Option<&'a CallGraph>,
    pub source: &'a str,
}

impl<'a> DetectionContext<'a> {
    pub fn new(type_graph: &'a TypeGraph, source: &'a str) -> Self {
        DetectionContext { type_graph, callgraph: None, source }
    }

    pub fn with_callgraph(type_graph: &'a TypeGraph, callgraph: &'a CallGraph, source: &'a str) -> Self {
        DetectionContext { type_graph, callgraph: Some(callgraph), source }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pattern_kind_display() {
        assert_eq!(format!("{}", PatternKind::Singleton), "Singleton");
        assert_eq!(format!("{}", PatternKind::Strategy), "Strategy");
        assert_eq!(format!("{}", PatternKind::DotnetMediator), "DotnetMediator");
        assert_eq!(format!("{}", PatternKind::ControllerApi), "ControllerApi");
        assert_eq!(format!("{}", PatternKind::MinimalApi), "MinimalApi");
    }
}
