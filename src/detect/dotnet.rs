//! .NET framework-specific pattern detection:
//! Controller API, Minimal API, MediatR

use crate::detect::types::{DetectionContext, PatternKind, PatternMatch};

pub fn detect_dotnet(ctx: &DetectionContext) -> Vec<PatternMatch> {
    let mut results = Vec::new();
    detect_controller_api(ctx, &mut results);
    detect_minimal_api(ctx, &mut results);
    detect_di_container(ctx, &mut results);
    results
}

fn detect_di_container(ctx: &DetectionContext, results: &mut Vec<PatternMatch>) {
    // DI Container: classes that register services via AddScoped/AddSingleton/AddTransient
    // Typically found in Startup-like classes or static extension methods
    let di_methods = ["AddScoped", "AddSingleton", "AddTransient",
        "AddDbContext", "AddDbContextPool", "AddAuthentication",
        "AddAuthorization", "AddControllers", "AddEndpointsApiExplorer",
        "AddSwaggerGen", "AddCors", "AddMvc", "AddSignalR", "AddGrpc"];

    for class in ctx.type_graph.classes.values() {
        if !class.is_static && class.name != "Program" && !class.name.ends_with("Startup")
            && !class.name.ends_with("Extension") && !class.name.ends_with("Extensions") {
            continue;
        }

        let mut di_count = 0;
        let mut registered_ifaces = Vec::new();

        for method in &class.methods {
            let sig = &method.signature;
            for di_m in &di_methods {
                if sig.contains(di_m) {
                    di_count += 1;
                    // Extract the generic type parameter if present
                    if let Some(start) = sig.find(di_m) {
                        let rest = &sig[start..];
                        if let Some(ts) = rest.find('<') {
                            if let Some(te) = rest.find('>') {
                                let type_arg = &rest[ts+1..te];
                                registered_ifaces.push(type_arg.to_string());
                            }
                        }
                    }
                }
            }
        }

        if di_count >= 2 {
            results.push(PatternMatch {
                pattern: PatternKind::DiContainer,
                class: class.name.clone(),
                description: format!(
                    "'{}' registers {} DI services — DI Container",
                    class.name, di_count
                ),
                confidence: 0.9,
                participants: vec![class.name.clone()],
                evidence: registered_ifaces,
            });
        }
    }
}

fn detect_controller_api(ctx: &DetectionContext, results: &mut Vec<PatternMatch>) {
    for class in ctx.type_graph.classes.values() {
        if class.is_abstract { continue; }

        let action_methods: Vec<_> = class.methods.iter()
            .filter(|m| {
                !m.method.starts_with("get_") && !m.method.starts_with("set_")
                    && m.method != class.name
            })
            .collect();
        if action_methods.len() < 2 { continue; }

        let extends_controller_base = is_controller_base(&class.name, ctx);
        let has_http_attr = action_methods.iter().any(|m| has_http_attribute(m, ctx));

        if extends_controller_base || has_http_attr {
            results.push(PatternMatch {
                pattern: PatternKind::ControllerApi,
                class: class.name.clone(),
                description: format!(
                    "'{}' with {} action methods — Controller API",
                    class.name, action_methods.len()
                ),
                confidence: if extends_controller_base && has_http_attr { 0.98 }
                    else if extends_controller_base { 0.95 }
                    else { 0.9 },
                participants: vec![class.name.clone()],
                evidence: action_methods.iter().map(|m| m.method.clone()).collect(),
            });
            continue;
        }

        let with_params = action_methods.iter()
            .filter(|m| {
                let sig = &m.signature;
                if let Some(start) = sig.find('(') {
                    if let Some(end) = sig.find(')') {
                        return end - start > 1;
                    }
                }
                false
            })
            .count();
        if action_methods.len() >= 3 && with_params as f64 / action_methods.len() as f64 >= 0.5 {
            results.push(PatternMatch {
                pattern: PatternKind::ControllerApi,
                class: class.name.clone(),
                description: format!(
                    "'{}' has {} action methods ({} with params) — Controller API",
                    class.name, action_methods.len(), with_params
                ),
                confidence: 0.7,
                participants: vec![class.name.clone()],
                evidence: action_methods.iter().map(|m| m.method.clone()).collect(),
            });
        }
    }
}

fn has_http_attribute(_m: &crate::resolve::types::MethodDescriptor, ctx: &DetectionContext) -> bool {
    let http_attrs = ["HttpGet", "HttpPost", "HttpPut", "HttpDelete",
        "HttpPatch", "HttpHead", "HttpOptions"];
    for attr in &http_attrs {
        if ctx.source.contains(&format!("[{}]", attr))
            || ctx.source.contains(&format!("[{}(", attr))
        {
            return true;
        }
    }
    false
}

fn is_controller_base(class_name: &str, ctx: &DetectionContext) -> bool {
    let mut current = class_name.to_string();
    loop {
        let next = ctx.type_graph.classes.get(&current)
            .and_then(|c| c.base_class.as_ref().cloned());
        match next {
            Some(base) => {
                if base == "ControllerBase" || base == "Controller" {
                    return true;
                }
                current = base;
            }
            None => return false,
        }
    }
}

fn detect_minimal_api(ctx: &DetectionContext, results: &mut Vec<PatternMatch>) {
    // Check source for MapGet/MapPost/MapPut/MapDelete patterns
    let has_map_calls = ctx.source.contains("MapGet(")
        || ctx.source.contains("MapPost(")
        || ctx.source.contains("MapPut(")
        || ctx.source.contains("MapDelete(")
        || ctx.source.contains("MapPatch(");

    for class in ctx.type_graph.classes.values() {
        if !class.is_static && class.name != "Program" { continue; }

        let has_webapp_param = class.methods.iter().any(|m| {
            let sig = &m.signature;
            if let Some(start) = sig.find('(') {
                if let Some(end) = sig.find(')') {
                    return sig[start..=end].contains("WebApplication");
                }
            }
            false
        });

        let non_ctor: Vec<_> = class.methods.iter()
            .filter(|m| m.method != class.name)
            .collect();
        if non_ctor.is_empty() { continue; }

        let confidence = if has_webapp_param && has_map_calls { 0.95 }
            else if has_map_calls { 0.85 }
            else if has_webapp_param { 0.8 }
            else { continue; };

        results.push(PatternMatch {
            pattern: PatternKind::MinimalApi,
            class: class.name.clone(),
            description: format!(
                "'{}' uses {} — Minimal API",
                class.name,
                if has_map_calls { "MapGet/MapPost endpoints" } else { "WebApplication" }
            ),
            confidence,
            participants: vec![class.name.clone()],
            evidence: non_ctor.iter().map(|m| m.method.clone()).collect(),
        });
    }
}
