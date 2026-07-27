use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;

use anyhow::Result;

use crate::analysis::callgraph::{CallGraph, CallGraphBuilder, CallSite};
use crate::resolve::types::TypeGraph;
use crate::cfg::builder::build_cfg;
use crate::cfg::builder::BlockKind;
use crate::detect::behavioral::detect_behavioral;
use crate::detect::creational::detect_creational;
use crate::detect::dotnet::detect_dotnet;
use crate::detect::structural::detect_structural;
use crate::detect::types::{DetectionContext, PatternMatch};
use crate::hammock::builder::find_hammocks;
use crate::parse::parser::parse_source;
use crate::pdg::pdg_builder::build_pdg;
use crate::resolve::symbols::SymbolTable;

pub fn handle_parse(file: &str) -> Result<()> {
    let source = fs::read_to_string(file)?;
    let tree = parse_source(&source)?;
    let root = tree.root_node();

    let st = SymbolTable::from_ast(root, &source)?;
    let json = serde_json::to_string_pretty(&st.type_graph)?;
    println!("{}", json);
    Ok(())
}

pub fn handle_cfg(file: &str, format: Option<&str>) -> Result<()> {
    let source = fs::read_to_string(file)?;
    let cfg = build_cfg(&source)?;

    match format.unwrap_or("dot") {
        "dot" => {
            println!("digraph CFG {{");
            for node in cfg.node_indices() {
                let block = &cfg[node];
                println!("  n{} [label=\"[{}] {:?} (L{}-L{})\"];",
                    block.id, block.id, block.kind, block.start_line, block.end_line);
            }
            for edge in cfg.raw_edges() {
                let from = cfg[edge.source()].id;
                let to = cfg[edge.target()].id;
                println!("  n{} -> n{} [label=\"{:?}\"];", from, to, edge.weight);
            }
            println!("}}");
        }
        "json" => {
            let mut nodes = Vec::new();
            for node in cfg.node_indices() {
                let block = &cfg[node];
                nodes.push(serde_json::json!({
                    "id": block.id,
                    "kind": format!("{:?}", block.kind),
                    "start_line": block.start_line,
                    "end_line": block.end_line,
                }));
            }
            let mut edges = Vec::new();
            for edge in cfg.raw_edges() {
                let from = cfg[edge.source()].id;
                let to = cfg[edge.target()].id;
                edges.push(serde_json::json!({
                    "from": from,
                    "to": to,
                    "kind": format!("{:?}", edge.weight),
                }));
            }
            println!("{}", serde_json::to_string_pretty(&serde_json::json!({
                "nodes": nodes,
                "edges": edges,
            }))?);
        }
        _ => anyhow::bail!("Unsupported format: {}. Use 'dot' or 'json'.", format.unwrap()),
    }
    Ok(())
}

pub fn handle_pdg(file: &str, format: Option<&str>) -> Result<()> {
    let source = fs::read_to_string(file)?;
    let cfg = build_cfg(&source)?;
    let pdg = build_pdg(&cfg)?;

    match format.unwrap_or("dot") {
        "dot" => {
            println!("digraph PDG {{");
            for node in pdg.node_indices() {
                let block = &pdg[node];
                println!("  n{} [label=\"[{}] {:?} (L{}-L{})\"];",
                    block.id, block.id, block.kind, block.start_line, block.end_line);
            }
            for edge in pdg.raw_edges() {
                let from = pdg[edge.source()].id;
                let to = pdg[edge.target()].id;
                println!("  n{} -> n{} [label=\"{}\"];", from, to, edge.weight);
            }
            println!("}}");
        }
        "json" => {
            let mut nodes = Vec::new();
            for node in pdg.node_indices() {
                let block = &pdg[node];
                nodes.push(serde_json::json!({
                    "id": block.id,
                    "kind": format!("{:?}", block.kind),
                    "start_line": block.start_line,
                    "end_line": block.end_line,
                }));
            }
            let mut edges = Vec::new();
            for edge in pdg.raw_edges() {
                let from = pdg[edge.source()].id;
                let to = pdg[edge.target()].id;
                edges.push(serde_json::json!({
                    "from": from,
                    "to": to,
                    "kind": format!("{}", edge.weight),
                }));
            }
            println!("{}", serde_json::to_string_pretty(&serde_json::json!({
                "nodes": nodes,
                "edges": edges,
            }))?);
        }
        _ => anyhow::bail!("Unsupported format: {}. Use 'dot' or 'json'.", format.unwrap()),
    }
    Ok(())
}

pub fn handle_hammock(file: &str, _level: Option<&str>) -> Result<()> {
    let source = fs::read_to_string(file)?;
    let cfg = build_cfg(&source)?;

    let entry = cfg.node_indices().find(|i| cfg[*i].kind == BlockKind::Entry);
    let exit = cfg.node_indices().find(|i| cfg[*i].kind == BlockKind::Exit);

    match (entry, exit) {
        (Some(e), Some(x)) => {
            let hammocks = find_hammocks(&cfg, e, x);
            if hammocks.is_empty() {
                println!("No hammock regions found.");
            } else {
                println!("Found {} hammock region(s):", hammocks.len());
                for (i, h) in hammocks.iter().enumerate() {
                    let header = &cfg[h.header];
                    let footer = &cfg[h.footer];
                    println!("  Hammock {}: header=[{}] {:?} body={} nodes footer=[{}] {:?}",
                        i + 1, header.id, header.kind, h.body.len(), footer.id, footer.kind);
                }
            }
        }
        _ => {
            println!("Could not find entry/exit nodes in CFG.");
        }
    }
    Ok(())
}

pub fn handle_resolve(path: &str, kind: Option<&str>) -> Result<()> {
    let path = Path::new(path);
    if !path.exists() {
        anyhow::bail!("Path does not exist: {}", path.display());
    }

    let mut all_cs_files = Vec::new();
    if path.is_file() {
        all_cs_files.push(path.to_string_lossy().to_string());
    } else {
        collect_cs_files(path, &mut all_cs_files);
    }

    let mut cg = CallGraph::new();
    let mut type_graph = None;

    for file in &all_cs_files {
        let source = match fs::read_to_string(file) {
            Ok(s) => s,
            Err(_) => continue,
        };
        let tree = match parse_source(&source) {
            Ok(t) => t,
            Err(_) => continue,
        };
        let st = match SymbolTable::from_ast(tree.root_node(), &source) {
            Ok(s) => s,
            Err(_) => continue,
        };
        let file_cg = CallGraphBuilder::build(tree.root_node(), &source, &st.type_graph);
        for call in file_cg.calls {
            cg.calls.push(call);
        }
        for (k, v) in file_cg.class_callees {
            cg.class_callees.entry(k).or_default().extend(v);
        }
        for (k, v) in file_cg.class_creations {
            cg.class_creations.entry(k).or_default().extend(v);
        }
        if type_graph.is_none() {
            type_graph = Some(st.type_graph);
        } else if let Some(ref mut tg) = type_graph {
            for (name, info) in st.type_graph.classes {
                tg.classes.entry(name).or_insert(info);
            }
            for (name, info) in st.type_graph.interfaces {
                tg.interfaces.entry(name).or_insert(info);
            }
        }
    }

    let tg = type_graph.unwrap_or_default();
    match kind.unwrap_or("all") {
        "di" | "factory" | "reflection" | "cha" | "all" => {
            println!("Resolution analysis for {} file(s):", all_cs_files.len());
            println!("  Classes: {}, Interfaces: {}", tg.classes.len(), tg.interfaces.len());
            println!("  Call sites: {}", cg.calls.len());
            println!();
            if !cg.calls.is_empty() {
                println!("Top called methods:");
                let mut callee_count: HashMap<String, usize> = HashMap::new();
                for call in &cg.calls {
                    *callee_count.entry(call.callee.clone()).or_default() += 1;
                }
                let mut ranked: Vec<_> = callee_count.into_iter().collect();
                ranked.sort_by(|a, b| b.1.cmp(&a.1));
                for (method, count) in ranked.iter().take(15) {
                    println!("  {} ({} calls)", method, count);
                }
            }
        }
        _ => anyhow::bail!("Unsupported resolution kind: {}. Use di, factory, reflection, cha, or all.", kind.unwrap()),
    }
    Ok(())
}

pub fn handle_detect(path: &str) -> Result<()> {
    let path = Path::new(path);
    if !path.exists() {
        anyhow::bail!("Path does not exist: {}", path.display());
    }

    let mut all_cs_files = Vec::new();
    if path.is_file() {
        all_cs_files.push(path.to_string_lossy().to_string());
    } else {
        collect_cs_files(path, &mut all_cs_files);
    }

    let mut cg = CallGraph::new();
    let mut type_graph = None;

    for file in &all_cs_files {
        let source = match fs::read_to_string(file) {
            Ok(s) => s,
            Err(_) => continue,
        };
        let tree = match parse_source(&source) {
            Ok(t) => t,
            Err(_) => continue,
        };
        let st = match SymbolTable::from_ast(tree.root_node(), &source) {
            Ok(s) => s,
            Err(_) => continue,
        };
        let file_cg = CallGraphBuilder::build(tree.root_node(), &source, &st.type_graph);
        for call in file_cg.calls {
            cg.calls.push(call);
        }
        for (k, v) in file_cg.class_callees {
            cg.class_callees.entry(k).or_default().extend(v);
        }
        for (k, v) in file_cg.class_creations {
            cg.class_creations.entry(k).or_default().extend(v);
        }
        if type_graph.is_none() {
            type_graph = Some(st.type_graph);
        } else if let Some(ref mut tg) = type_graph {
            for (name, info) in st.type_graph.classes {
                tg.classes.entry(name).or_insert(info);
            }
            for (name, info) in st.type_graph.interfaces {
                tg.interfaces.entry(name).or_insert(info);
            }
        }
    }

    let tg = type_graph.unwrap_or_default();
    let ctx = DetectionContext::with_callgraph(&tg, &cg, "");

    let mut detections = Vec::new();
    detections.extend(detect_creational(&ctx));
    detections.extend(detect_structural(&ctx));
    detections.extend(detect_behavioral(&ctx));
    detections.extend(detect_dotnet(&ctx));

    println!("Analysis of {} file(s):", all_cs_files.len());
    println!("  Classes: {}, Interfaces: {}, Call sites: {}",
        tg.classes.len(), tg.interfaces.len(), cg.calls.len());
    println!();

    if detections.is_empty() {
        println!("No patterns detected.");
    } else {
        let mut by_pattern: HashMap<String, Vec<&PatternMatch>> = HashMap::new();
        for d in &detections {
            let label = format!("{}", d.pattern);
            by_pattern.entry(label).or_default().push(d);
        }
        let mut keys: Vec<_> = by_pattern.keys().collect();
        keys.sort();
        for k in keys {
            let hits = &by_pattern[k];
            println!("{} ({} hits):", k, hits.len());
            for d in hits {
                println!("  {} (c={:.2})", d.class, d.confidence);
                if !d.evidence.is_empty() {
                    println!("    evidence: {}", d.evidence.join(", "));
                }
            }
        }
    }
    Ok(())
}

pub fn handle_traverse(path: &str, class: &str, context: Option<&str>) -> Result<()> {
    let path = Path::new(path);
    if !path.exists() {
        anyhow::bail!("Path does not exist: {}", path.display());
    }

    let (tg, cg) = load_project(path)?;

    let mut state = match crate::traverse::types::TraversalState::init(&tg, &cg, class, context.map(|s| s.to_string())) {
        Some(s) => s,
        None => anyhow::bail!("Class '{}' not found or has no methods", class),
    };

    crate::traverse::engine::run(&mut state, &tg, &cg)?;
    Ok(())
}

pub fn handle_callgraph(path: &str, class: Option<&str>, depth: usize, only_outbound: bool, only_inbound: bool, trace: bool) -> Result<()> {
    let path = Path::new(path);
    if !path.exists() {
        anyhow::bail!("Path does not exist: {}", path.display());
    }

    let mut all_cs_files = Vec::new();
    if path.is_file() {
        all_cs_files.push(path.to_string_lossy().to_string());
    } else {
        collect_cs_files(path, &mut all_cs_files);
    }

    let mut cg = CallGraph::new();
    let mut type_graph = None;

    for file in &all_cs_files {
        let source = match fs::read_to_string(file) {
            Ok(s) => s,
            Err(_) => continue,
        };
        let tree = match parse_source(&source) {
            Ok(t) => t,
            Err(_) => continue,
        };
        let st = match SymbolTable::from_ast(tree.root_node(), &source) {
            Ok(s) => s,
            Err(_) => continue,
        };
        let file_cg = CallGraphBuilder::build(tree.root_node(), &source, &st.type_graph);
        for call in file_cg.calls {
            cg.calls.push(call);
        }
        for (k, v) in file_cg.class_callees {
            cg.class_callees.entry(k).or_default().extend(v);
        }
        for (k, v) in file_cg.class_creations {
            cg.class_creations.entry(k).or_default().extend(v);
        }
        if type_graph.is_none() {
            type_graph = Some(st.type_graph);
        } else if let Some(ref mut tg) = type_graph {
            for (name, info) in st.type_graph.classes {
                tg.classes.entry(name).or_insert(info);
            }
            for (name, info) in st.type_graph.interfaces {
                tg.interfaces.entry(name).or_insert(info);
            }
        }
    }

    let tg = type_graph.unwrap_or_default();
    println!("Parsed {} file(s)", all_cs_files.len());
    println!("Classes: {}, Interfaces: {}, Call sites: {}",
        tg.classes.len(), tg.interfaces.len(), cg.calls.len());
    println!();

    match class {
        Some(class_name) => {
            let matching: Vec<String> = tg.classes.keys()
                .filter(|k| k.to_lowercase().contains(&class_name.to_lowercase()))
                .cloned()
                .collect();

            if matching.is_empty() {
                println!("No class found matching '{}'", class_name);
                let mut names: Vec<&str> = tg.classes.keys().map(|s| s.as_str()).collect();
                names.sort();
                for n in names {
                    println!("  {}", n);
                }
                return Ok(());
            }

            for cn in &matching {
                println!("{}", "=".repeat(70));
                println!("CALL GRAPH FOR: {}", cn);
                println!("{}", "=".repeat(70));

                if !only_inbound {
                    println!("\n── Outbound calls (what {} calls) ──", cn);
                    let mut class_methods: Vec<String> = tg.classes.get(cn.as_str())
                        .map(|c| c.methods.iter().filter(|m| !m.method.starts_with("get_") && !m.method.starts_with("set_")).map(|m| m.method.clone()).collect())
                        .unwrap_or_default();
                    class_methods.sort();
                    class_methods.dedup();
                    if class_methods.is_empty() {
                        println!("  (none)");
                    } else {
                        for method in &class_methods {
                            let calls_from: Vec<_> = cg.calls.iter()
                                .filter(|c| c.caller_class == *cn && c.caller_method == *method)
                                .collect();
                            if calls_from.is_empty() { continue; }
                            let trace_depth = if trace { depth } else { 1 };
                            if trace_depth >= 1 {
                                println!("  {}() calls:", method);
                                print_call_tree(&tg, &cg, cn, method, &mut HashSet::new(), "    ", trace_depth);
                            }
                        }
                    }
                }

                if !only_outbound {
                    println!("\n── Inbound calls (what calls {}) ──", cn);
                    let mut class_methods: Vec<String> = tg.classes.get(cn.as_str())
                        .map(|c| c.methods.iter().filter(|m| !m.method.starts_with("get_") && !m.method.starts_with("set_")).map(|m| m.method.clone()).collect())
                        .unwrap_or_default();
                    class_methods.sort();
                    class_methods.dedup();

                    let all_callees_of_class: Vec<&str> = if class_methods.is_empty() {
                        // Try matching class name as callee
                        cg.calls.iter()
                            .filter(|c| c.callee == *cn)
                            .map(|c| c.callee.as_str())
                            .collect()
                    } else {
                        // Match by method names
                        let mut result = Vec::new();
                        for m in &class_methods {
                            let count = cg.calls.iter().filter(|c| c.callee == *m).count();
                            if count > 0 {
                                result.push(m.as_str());
                            }
                        }
                        result
                    };

                    if all_callees_of_class.is_empty() {
                        println!("  (none)");
                    } else {
                        for method_name in &class_methods {
                            let calls: Vec<_> = cg.calls.iter()
                                .filter(|c| c.callee == *method_name)
                                .collect();
                            if calls.is_empty() { continue; }
                            println!("  {}() <-", method_name);
                            let mut by_caller: HashMap<String, Vec<String>> = HashMap::new();
                            for c in &calls {
                                let via = if c.target_expr.is_empty() { "direct".to_string() } else { c.target_expr.clone() };
                                by_caller.entry(c.caller_class.clone()).or_default().push(via);
                            }
                            let mut callers: Vec<String> = by_caller.keys().cloned().collect();
                            callers.sort();
                            for caller in &callers {
                                let vias = by_caller.get(caller).cloned().unwrap_or_default();
                                let via_str = if vias.len() == 1 { format!(" via {}", vias[0]) } else { String::new() };
                                println!("    └── {} ({}x{})", caller, vias.len(), via_str);
                            }
                        }
                    }
                }
            }
        }
        None => {
            println!("{}", "=".repeat(70));
            println!("FULL CALL GRAPH SUMMARY");
            println!("{}", "=".repeat(70));

            let mut by_caller: HashMap<String, Vec<String>> = HashMap::new();
            for call in &cg.calls {
                by_caller.entry(call.caller_class.clone()).or_default().push(call.callee.clone());
            }
            let mut caller_classes: Vec<String> = by_caller.keys().cloned().collect();
            caller_classes.sort();
            for caller in &caller_classes {
                let callees = by_caller.get(caller).cloned().unwrap_or_default();
                let mut counts: HashMap<String, usize> = HashMap::new();
                for c in &callees { *counts.entry(c.clone()).or_default() += 1; }
                let mut sorted: Vec<_> = counts.into_iter().collect();
                sorted.sort_by(|a, b| b.1.cmp(&a.1));
                println!("  {} ({} calls):", caller, callees.len());
                for (callee, count) in sorted.iter().take(5) {
                    println!("    {} ({}x)", callee, count);
                }
            }
        }
    }
    Ok(())
}

fn resolve_callee_class(callee: &str, target_expr: &str, tg: &TypeGraph) -> Option<String> {
    if !target_expr.is_empty() && tg.classes.contains_key(target_expr) {
        return Some(target_expr.to_string());
    }
    for (class_name, class_info) in &tg.classes {
        if class_info.methods.iter().any(|m| m.method == callee) {
            return Some(class_name.clone());
        }
    }
    None
}

fn print_call_tree(
    tg: &TypeGraph,
    cg: &CallGraph,
    class: &str,
    method: &str,
    visited: &mut HashSet<(String, String)>,
    prefix: &str,
    depth: usize,
) {
    if depth == 0 { return; }

    let key = (class.to_string(), method.to_string());
    if !visited.insert(key.clone()) {
        println!("{}  (cycle)", prefix);
        return;
    }

    let calls: Vec<_> = cg.calls.iter()
        .filter(|c| c.caller_class == class && c.caller_method == method)
        .collect();

    if calls.is_empty() {
        visited.remove(&key);
        return;
    }

    let mut by_callee: HashMap<String, Vec<&CallSite>> = HashMap::new();
    for c in &calls {
        by_callee.entry(c.callee.clone()).or_default().push(c);
    }

    let mut callee_names: Vec<String> = by_callee.keys().cloned().collect();
    callee_names.sort();
    let total = callee_names.len();

    for (i, callee) in callee_names.iter().enumerate() {
        let sites = &by_callee[callee];
        let count = sites.len();
        let first = sites[0];
        let via = if first.target_expr.is_empty() { String::new() } else { format!(" via {}", first.target_expr) };

        let is_last = i == total - 1;
        let connector = if is_last { "└──" } else { "├──" };
        let next_prefix = format!("{}{}", prefix, if is_last { "   " } else { "│  " });

        println!("{}{} {}{} ({}x)", prefix, connector, callee, via, count);

        let dispatch_ifaces: Vec<(String, Vec<String>)> = tg.interfaces.iter()
            .filter(|(_, iface)| iface.methods.iter().any(|m| m.method == callee.as_str()))
            .filter_map(|(name, _)| {
                let impls: Vec<String> = tg.concrete_subclasses(name)
                    .iter().map(|c| c.name.clone()).collect();
                if impls.is_empty() { None } else { Some((name.clone(), impls)) }
            })
            .collect();

        if !dispatch_ifaces.is_empty() {
            for (iface_name, implementors) in &dispatch_ifaces {
                println!("{}══ DISPATCH: {} → {} ══", next_prefix, first.target_expr, iface_name);
                for impl_class in implementors {
                    println!("{}── {} ──", next_prefix, impl_class);
                    print_call_tree(tg, cg, impl_class, callee,
                                  &mut HashSet::new(),
                                  &format!("{}  ", next_prefix), depth - 1);
                }
            }
        } else {
            if let Some(resolved_class) = resolve_callee_class(callee, &first.target_expr, tg) {
                let mut branch_visited = if resolved_class == class { visited.clone() } else { HashSet::new() };
                print_call_tree(tg, cg, &resolved_class, callee, &mut branch_visited,
                              &next_prefix, depth - 1);
            }
        }
    }

    visited.remove(&key);
}

fn collect_cs_files(dir: &Path, files: &mut Vec<String>) {
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                collect_cs_files(&path, files);
            } else if path.extension().map_or(false, |e| e == "cs") {
                files.push(path.to_string_lossy().to_string());
            }
        }
    }
}

fn load_project(path: &Path) -> Result<(TypeGraph, CallGraph)> {
    let mut all_cs_files = Vec::new();
    if path.is_file() {
        all_cs_files.push(path.to_string_lossy().to_string());
    } else {
        collect_cs_files(path, &mut all_cs_files);
    }

    let mut cg = CallGraph::new();
    let mut type_graph = None;

    for file in &all_cs_files {
        let source = match fs::read_to_string(file) {
            Ok(s) => s,
            Err(_) => continue,
        };
        let tree = match crate::parse::parser::parse_source(&source) {
            Ok(t) => t,
            Err(_) => continue,
        };
        let st = match crate::resolve::symbols::SymbolTable::from_ast(tree.root_node(), &source) {
            Ok(s) => s,
            Err(_) => continue,
        };
        let file_cg = CallGraphBuilder::build(tree.root_node(), &source, &st.type_graph);
        for call in file_cg.calls {
            cg.calls.push(call);
        }
        for (k, v) in file_cg.class_callees {
            cg.class_callees.entry(k).or_default().extend(v);
        }
        for (k, v) in file_cg.class_creations {
            cg.class_creations.entry(k).or_default().extend(v);
        }
        if type_graph.is_none() {
            type_graph = Some(st.type_graph);
        } else if let Some(ref mut tg) = type_graph {
            for (name, info) in st.type_graph.classes {
                tg.classes.entry(name).or_insert(info);
            }
            for (name, info) in st.type_graph.interfaces {
                tg.interfaces.entry(name).or_insert(info);
            }
        }
    }

    let tg = type_graph.unwrap_or_default();
    println!("Parsed {} file(s)", all_cs_files.len());
    println!("Classes: {}, Interfaces: {}, Call sites: {}",
        tg.classes.len(), tg.interfaces.len(), cg.calls.len());
    println!();

    Ok((tg, cg))
}
