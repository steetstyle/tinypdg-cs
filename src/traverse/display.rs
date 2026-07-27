use crate::traverse::types::*;

pub fn print_header(state: &TraversalState) {
    println!("{}", "═".repeat(70));
    println!("  Entity: {}.{}", state.current.class, state.current.method);
    println!("  Queue: {} remaining | History: {} steps",
        state.queue.len(), state.history.len());
    println!("{}", "═".repeat(70));
    println!();
}

pub fn print_code(node: &NodeRef) {
    println!("  {}.{}()", node.class, node.method);
    println!("  (source: class scope — use ↓ to navigate into calls)");
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
        println!("  {} [{}] {} via {} {}", marker, entry.idx, entry.callee, entry.via, entry.target.class);

        match &entry.kind {
            EdgeKind::Interface { interface, implementations } => {
                println!("       ══ INTERFACE: {} ══", interface);
                for (impl_class, conf) in implementations {
                    println!("       ├─ [{}a] {}.{}  (c={:.2})",
                        entry.idx, impl_class, entry.callee, conf);
                }
            }
            EdgeKind::Virtual { base_class, overrides } => {
                println!("       ══ VIRTUAL: {} ══", base_class);
                for (ov_class, conf) in overrides {
                    println!("       ├─ [{}a] {}.{}  (c={:.2})",
                        entry.idx, ov_class, entry.callee, conf);
                }
            }
            EdgeKind::External => {
                println!("       (external library)");
            }
            EdgeKind::Direct => {}
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
        matches!(self, EdgeKind::Interface { .. } | EdgeKind::Virtual { .. })
    }
}
