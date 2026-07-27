//! Core types for call resolution

use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum CallTarget {
    /// Static method call: `ClassName.MethodName`
    Static {
        class: String,
        method: String,
    },
    /// Instance method call: `obj.MethodName`
    Instance {
        method: String,
    },
    /// Virtual method call: could be overridden
    Virtual {
        class: Option<String>,
        method: String,
    },
    /// Abstract/interface method call
    Abstract {
        interface: String,
        method: String,
    },
    /// DI resolved call: interface → concrete via container
    DiResolved {
        interface: String,
        method: String,
    },
}

/// Resolution confidence with numerical ranking (0-100)
/// Higher = more certain
#[derive(Debug, Clone, PartialEq)]
pub enum Confidence {
    Direct,               // 100
    ExplicitImpl,         // 95 — single interface impl
    DiRegistration,       // 95 — AddScoped<IFoo, Foo>()
    CHA,                  // 70 — class hierarchy analysis
    MultiImpl,            // 60 — multiple interface implementations
    RTA,                  // 50 — rapid type analysis
    Reflection,           // 40 — typeof+GetMethod
    DynamicString,        // 10 — reflection with runtime string
    Unknown,              // 0
}

impl Confidence {
    pub fn score(&self) -> u8 {
        match self {
            Confidence::Direct => 100,
            Confidence::ExplicitImpl => 95,
            Confidence::DiRegistration => 95,
            Confidence::CHA => 70,
            Confidence::MultiImpl => 60,
            Confidence::RTA => 50,
            Confidence::Reflection => 40,
            Confidence::DynamicString => 10,
            Confidence::Unknown => 0,
        }
    }
}

impl std::fmt::Display for Confidence {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}({})", match self {
            Confidence::Direct => "Direct",
            Confidence::ExplicitImpl => "ExplicitImpl",
            Confidence::DiRegistration => "DiRegistration",
            Confidence::CHA => "CHA",
            Confidence::MultiImpl => "MultiImpl",
            Confidence::RTA => "RTA",
            Confidence::Reflection => "Reflection",
            Confidence::DynamicString => "DynamicString",
            Confidence::Unknown => "Unknown",
        }, self.score())
    }
}

/// A single call resolution result
#[derive(Debug, Clone, PartialEq)]
pub struct CallSite {
    /// Which method this call belongs to
    pub caller: String,
    /// The call expression
    pub target: CallTarget,
    /// Resolution confidence
    pub confidence: Confidence,
    /// Possible concrete methods this resolves to
    pub resolved: Vec<MethodDescriptor>,
}

/// Combines multiple resolution strategies for one call site.
/// The winning result is the one with highest confidence.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolutionSet {
    pub callsite: CallSite,
    pub alternatives: Vec<CallSite>,
}

impl ResolutionSet {
    pub fn new(site: CallSite) -> Self {
        ResolutionSet {
            callsite: site.clone(),
            alternatives: vec![site],
        }
    }

    /// Pick the best (highest confidence) resolution
    pub fn best(&self) -> &CallSite {
        self.alternatives.iter()
            .max_by_key(|a| a.confidence.score())
            .unwrap_or(&self.callsite)
    }

    pub fn add(&mut self, site: CallSite) {
        if site.confidence.score() > self.callsite.confidence.score() {
            self.callsite = site.clone();
        }
        self.alternatives.push(site);
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Default, serde::Serialize)]
pub struct MethodDescriptor {
    pub class: String,
    pub method: String,
    pub signature: String,
    pub is_static: bool,
    pub is_virtual: bool,
    pub is_abstract: bool,
    #[serde(default)]
    pub file: String,
    #[serde(default)]
    pub line_start: usize,
    #[serde(default)]
    pub line_end: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize)]
pub struct FieldDescriptor {
    pub name: String,
    pub field_type: String,
    pub is_static: bool,
    pub is_readonly: bool,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ClassInfo {
    pub name: String,
    pub base_class: Option<String>,
    pub interfaces: Vec<String>,
    pub methods: Vec<MethodDescriptor>,
    pub fields: Vec<FieldDescriptor>,
    pub is_abstract: bool,
    pub is_sealed: bool,
    pub is_static: bool,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct InterfaceInfo {
    pub name: String,
    pub methods: Vec<MethodDescriptor>,
}

/// DI registration: e.g. `AddScoped<IFoo, Foo>()`
#[derive(Debug, Clone)]
pub struct DiRegistration {
    pub interface_type: String,
    pub implementation_type: String,
    pub lifetime: DiLifetime,
    pub confidence: Confidence,
}

#[derive(Debug, Clone, PartialEq)]
pub enum DiLifetime {
    Singleton,
    Scoped,
    Transient,
}

/// Factory descriptor: factory method that creates instances
#[derive(Debug, Clone)]
pub struct FactoryDescriptor {
    pub return_type: String,
    pub factory_method: String,
    pub conditional: bool,
}

/// Reflection call pattern
#[derive(Debug, Clone)]
pub struct ReflectionPattern {
    pub target_type: String,
    pub method_name: String,
    pub is_static_string: bool, // true if method name is compile-time constant
}

/// Type graph: class hierarchy + interface implementations
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct TypeGraph {
    pub classes: HashMap<String, ClassInfo>,
    pub interfaces: HashMap<String, InterfaceInfo>,
}

impl TypeGraph {
    pub fn new() -> Self {
        TypeGraph {
            classes: HashMap::new(),
            interfaces: HashMap::new(),
        }
    }

    /// All concrete (non-abstract, non-static) subclasses of a class/interface
    pub fn concrete_subclasses(&self, name: &str) -> Vec<&ClassInfo> {
        let mut result = Vec::new();
        for class in self.classes.values() {
            if class.is_abstract || class.is_static {
                continue;
            }
            if class.name == name
                || self.is_subclass_of(class, name)
                || self.implements_interface(class, name)
            {
                result.push(class);
            }
        }
        result
    }

    /// Find all classes that implement a given interface
    pub fn implementors_of(&self, interface: &str) -> Vec<&ClassInfo> {
        self.classes.values()
            .filter(|c| !c.is_abstract && !c.is_static && self.implements_interface(c, interface))
            .collect()
    }

    fn is_subclass_of(&self, class: &ClassInfo, ancestor: &str) -> bool {
        let mut current = class.base_class.as_deref();
        while let Some(base) = current {
            if base == ancestor {
                return true;
            }
            current = self.classes.get(base).and_then(|c| c.base_class.as_deref());
        }
        false
    }

    fn implements_interface(&self, class: &ClassInfo, iface: &str) -> bool {
        class.interfaces.iter().any(|i| i == iface)
            || class.base_class.as_ref().map_or(false, |base| {
                self.classes.get(base).map_or(false, |c| self.implements_interface(c, iface))
            })
    }

    /// Populate method file paths after parsing.
    pub fn annotate_method_files(&mut self, file: &str) {
        for ci in self.classes.values_mut() {
            for m in &mut ci.methods {
                if m.file.is_empty() {
                    m.file = file.to_string();
                }
            }
        }
        for ii in self.interfaces.values_mut() {
            for m in &mut ii.methods {
                if m.file.is_empty() {
                    m.file = file.to_string();
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_type_graph_empty() {
        let tg = TypeGraph::new();
        assert_eq!(tg.concrete_subclasses("IFoo").len(), 0);
    }

    #[test]
    fn test_concrete_subclass() {
        let mut tg = TypeGraph::new();
        tg.classes.insert("Base".into(), ClassInfo {
            name: "Base".into(),
            base_class: None,
            interfaces: vec![],
            methods: vec![],
            fields: vec![],
            is_abstract: false,
            is_sealed: false,
            is_static: false,
        });
        tg.classes.insert("Derived".into(), ClassInfo {
            name: "Derived".into(),
            base_class: Some("Base".into()),
            interfaces: vec![],
            methods: vec![],
            fields: vec![],
            is_abstract: false,
            is_sealed: false,
            is_static: false,
        });
        let subs = tg.concrete_subclasses("Base");
        assert_eq!(subs.len(), 2); // Base + Derived
    }

    #[test]
    fn test_confidence_scoring() {
        assert!(Confidence::Direct.score() > Confidence::RTA.score());
        assert!(Confidence::Reflection.score() > Confidence::Unknown.score());
    }

    #[test]
    fn test_resolution_set_best() {
        let low = CallSite {
            caller: "Test".into(),
            target: CallTarget::Instance { method: "Foo".into() },
            confidence: Confidence::CHA,
            resolved: vec![],
        };
        let high = CallSite {
            caller: "Test".into(),
            target: CallTarget::Instance { method: "Foo".into() },
            confidence: Confidence::Direct,
            resolved: vec![],
        };
        let mut rs = ResolutionSet::new(low.clone());
        rs.add(high.clone());
        assert_eq!(rs.best().confidence, Confidence::Direct);
    }

    #[test]
    fn test_implementors_of() {
        let mut tg = TypeGraph::new();
        tg.interfaces.insert("IFoo".into(), InterfaceInfo {
            name: "IFoo".into(),
            methods: vec![],
        });
        tg.classes.insert("Foo".into(), ClassInfo {
            name: "Foo".into(),
            base_class: None,
            interfaces: vec!["IFoo".into()],
            methods: vec![],
            fields: vec![],
            is_abstract: false,
            is_sealed: false,
            is_static: false,
        });
        let impls = tg.implementors_of("IFoo");
        assert_eq!(impls.len(), 1);
        assert_eq!(impls[0].name, "Foo");
    }
}
