use std::fs;

use crate::resolve::types::TypeGraph;
use crate::analysis::callgraph::CallGraph;
use crate::traverse::types::*;

pub fn print_header(state: &TraversalState) {
    println!("{}", "═".repeat(70));
    println!("  Entity: {}.{}", state.current.class, state.current.method);
    println!("  Queue: {} remaining | History: {} steps",
        state.queue.len(), state.history.len());
    println!("{}", "═".repeat(70));
    println!();
}

pub fn print_code(state: &TraversalState, tg: &TypeGraph, cg: &CallGraph) {
    // External node — show caller's source
    let is_external = !tg.classes.contains_key(&state.current.class)
        && !tg.interfaces.contains_key(&state.current.class);

    if is_external {
        let callers: Vec<&crate::analysis::callgraph::CallSite> = cg.calls.iter()
            .filter(|c| c.callee == state.current.method && c.target_expr == state.current.class)
            .collect();
        if let Some(caller) = callers.first() {
            let md = tg.classes.get(&caller.caller_class)
                .and_then(|c| c.methods.iter().find(|m| m.method == caller.caller_method));
            if let Some(m) = md {
                if !m.file.is_empty() && m.line_start > 0 {
                    if let Ok(source) = fs::read_to_string(&m.file) {
                        let lines: Vec<&str> = source.lines().collect();
                        let start = m.line_start.saturating_sub(1);
                        let end = m.line_end.min(lines.len());
                        println!("  ┌─ {} (external) {}.{}", m.file, state.current.class, state.current.method);
                        for (i, line) in lines[start..end].iter().enumerate() {
                            println!("  │ {:>4} {}", start + i + 1, line);
                        }
                        println!("  └─");
                        println!();
                        return;
                    }
                }
            }
        }
        // Fallback: can't find caller context
        println!("  (external) {}.{} — no caller context available", state.current.class, state.current.method);
        println!();
        return;
    }

    let md = tg.classes.get(&state.current.class)
        .and_then(|c| c.methods.iter().find(|m| m.method == state.current.method))
        .or_else(|| {
            tg.interfaces.get(&state.current.class)
                .and_then(|i| i.methods.iter().find(|m| m.method == state.current.method))
        });

    let md = match md {
        Some(m) if !m.file.is_empty() && m.line_start > 0 => m,
        _ => {
            println!("  ⚠ Source not available (external/library method)");
            println!();
            return;
        }
    };

    let source = match fs::read_to_string(&md.file) {
        Ok(s) => s,
        Err(_) => {
            println!("  ⚠ Could not read source file: {}", md.file);
            println!();
            return;
        }
    };

    let lines: Vec<&str> = source.lines().collect();
    let start = md.line_start.saturating_sub(1);
    let end = md.line_end.min(lines.len());

    println!("  ┌─ {}:{}", md.file, md.line_start);
    for (i, line) in lines[start..end].iter().enumerate() {
        println!("  │ {:>4} {}", start + i + 1, line);
    }
    println!("  └─");
    println!();
}

pub fn print_nav(entries: &[NavEntry], label: &str) {
    if entries.is_empty() {
        println!("  {} (none)", label);
        return;
    }

    let mut i = 0;
    while i < entries.len() {
        let entry = &entries[i];
        let marker = if entry.kind.is_dispatch() { "►" } else { " " };

        if entry.target.class.is_empty() || entry.via == entry.target.class {
            println!("  {} [{}] {} via {}", marker, entry.idx, entry.callee, entry.via);
        } else {
            println!("  {} [{}] {} via {} {}", marker, entry.idx, entry.callee, entry.via, entry.target.class);
        }
        if let Some(ref ctx) = entry.context {
            println!("       context: {}", ctx);
        }

        match &entry.kind {
            EdgeKind::Interface { interface, implementations } => {
                println!("       ══ INTERFACE: {} ══", interface);
                for (i, (impl_class, conf)) in implementations.iter().enumerate() {
                    let letter = (b'a' + i as u8) as char;
                    println!("       ├─ [{}{}] {}.{}  (c={:.2})",
                        entry.idx, letter, impl_class, entry.callee, conf);
                }
            }
            EdgeKind::Virtual { base_class, overrides } => {
                println!("       ══ VIRTUAL: {} ══", base_class);
                for (i, (ov_class, conf)) in overrides.iter().enumerate() {
                    let letter = (b'a' + i as u8) as char;
                    println!("       ├─ [{}{}] {}.{}  (c={:.2})",
                        entry.idx, letter, ov_class, entry.callee, conf);
                }
            }
            EdgeKind::External => {
                println!("       (external library)");
            }
            EdgeKind::Direct => {}
            EdgeKind::Delegate { handlers } => {
                println!("       ══ DELEGATE HANDLERS ══");
                for (i, handler) in handlers.iter().enumerate() {
                    let letter = (b'a' + i as u8) as char;
                    println!("       ├─ [{}{}] {} ({})", entry.idx, letter, handler, entry.target.class);
                }
            }
        }
        i += 1;
    }
    println!();
}

pub fn print_history(state: &TraversalState) {
    println!("{}", "─".repeat(50));
    println!("  TRAVERSAL HISTORY");
    println!("{}", "─".repeat(50));
    for (i, entry) in state.history.iter().enumerate() {
        println!("  {}: {}.{}  →  {}",
            i + 1, entry.node.class, entry.node.method, entry.action);
    }
    if !state.judgments.is_empty() {
        println!();
        println!("  JUDGMENTS:");
        for (class, j) in &state.judgments {
            let label = match j {
                Judgment::Primary => "PRIMARY FAILURE",
                Judgment::Symptom => "SYMPTOM ONLY",
                Judgment::Unrelated => "UNRELATED",
            };
            let evidence = state.evidence.get(class).map(|s| s.as_str()).unwrap_or("");
            println!("    {} → {}: {}", class, label, evidence);
        }
    }
    println!();
}

pub fn print_prompt() {
    print!("  Actions: <n> down | u<n> up | c(p|s|u) complete | d discard | h history | q quit\n  > ");
    use std::io::{Write, stdout};
    let _ = stdout().flush();
}

impl EdgeKind {
    fn is_dispatch(&self) -> bool {
        matches!(self, EdgeKind::Interface { .. } | EdgeKind::Virtual { .. } | EdgeKind::Delegate { .. })
    }
}
