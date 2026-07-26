use std::collections::HashMap;
use std::fs;

use tiny_pdg_cs::analysis::callgraph::{CallGraph, CallGraphBuilder};
use tiny_pdg_cs::detect::behavioral::detect_behavioral;
use tiny_pdg_cs::detect::creational::detect_creational;
use tiny_pdg_cs::detect::dotnet::detect_dotnet;
use tiny_pdg_cs::detect::structural::detect_structural;
use tiny_pdg_cs::detect::types::DetectionContext;
use tiny_pdg_cs::parse::parser::parse_source;
use tiny_pdg_cs::resolve::symbols::SymbolTable;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let eshop_root = if args.len() > 1 {
        args[1].clone()
    } else {
        "/tmp/eShop/src".to_string()
    };

    let mut all_cs_files: Vec<String> = Vec::new();
    collect_cs_files(&eshop_root, &mut all_cs_files);
    println!("Total C# files found: {}\n", all_cs_files.len());

    // Group by first-level subdirectory (project)
    let mut project_groups: HashMap<String, Vec<String>> = HashMap::new();
    for path in &all_cs_files {
        let rel = path.strip_prefix(&eshop_root).unwrap_or(path);
        let proj = rel.split('/').next().unwrap_or("unknown").to_string();
        project_groups.entry(proj).or_default().push(path.clone());
    }

    let mut all_cg = CallGraph::new();
    let mut all_tg: Option<tiny_pdg_cs::resolve::types::TypeGraph> = None;
    let mut total_parsed = 0;
    let mut total_errors = 0;

    let mut sorted_projects: Vec<_> = project_groups.keys().collect();
    sorted_projects.sort();

    for proj in &sorted_projects {
        let files = &project_groups[proj.as_str()];
        println!("\n{}", "=".repeat(80));
        println!("PROJECT: {} ({} files)", proj, files.len());
        println!("{}", "=".repeat(80));

        let mut cg = CallGraph::new();
        let mut tg: Option<tiny_pdg_cs::resolve::types::TypeGraph> = None;
        let mut parsed = 0;
        let mut errors = 0;

        for file in files {
            let source = match fs::read_to_string(file) {
                Ok(s) => s,
                Err(_) => { errors += 1; continue; }
            };
            let tree = match parse_source(&source) {
                Ok(t) => t,
                Err(_) => { errors += 1; continue; }
            };
            let st = match SymbolTable::from_ast(tree.root_node(), &source) {
                Ok(s) => s,
                Err(_) => { errors += 1; continue; }
            };
            let file_cg = CallGraphBuilder::build(tree.root_node(), &source, &st.type_graph);

            // Merge into all first (cloning, before moving into per-project)
            if all_tg.is_none() {
                all_tg = Some(st.type_graph.clone());
            } else if let Some(ref mut atg) = all_tg {
                for (name, info) in &st.type_graph.classes { atg.classes.entry(name.clone()).or_insert_with(|| info.clone()); }
                for (name, info) in &st.type_graph.interfaces { atg.interfaces.entry(name.clone()).or_insert_with(|| info.clone()); }
            }
            all_cg.calls.extend(file_cg.calls.iter().cloned());
            for (k, v) in &file_cg.class_callees { all_cg.class_callees.entry(k.clone()).or_default().extend(v.iter().cloned()); }
            for (k, v) in &file_cg.class_creations { all_cg.class_creations.entry(k.clone()).or_default().extend(v.iter().cloned()); }

            // Merge into per-project (consumes)
            if tg.is_none() {
                tg = Some(st.type_graph);
            } else if let Some(ref mut ptg) = tg {
                for (name, info) in st.type_graph.classes { ptg.classes.entry(name).or_insert(info); }
                for (name, info) in st.type_graph.interfaces { ptg.interfaces.entry(name).or_insert(info); }
            }
            for call in file_cg.calls { cg.calls.push(call); }
            for (k, v) in file_cg.class_callees { cg.class_callees.entry(k).or_default().extend(v); }
            for (k, v) in file_cg.class_creations { cg.class_creations.entry(k).or_default().extend(v); }

            parsed += 1;
        }

        total_parsed += parsed;
        total_errors += errors;

        let tg = match tg {
            Some(t) => t,
            None => { continue; }
        };

        println!("Parsed: {}/{} (errors: {})", parsed, files.len(), errors);
        println!("Classes: {}, Interfaces: {}, Call sites: {}",
            tg.classes.len(), tg.interfaces.len(), cg.calls.len());

        // Per-project pattern detection
        let ctx = DetectionContext::with_callgraph(&tg, &cg, "");
        let mut detections = Vec::new();
        detections.extend(detect_creational(&ctx));
        detections.extend(detect_structural(&ctx));
        detections.extend(detect_behavioral(&ctx));
        detections.extend(detect_dotnet(&ctx));

        if !detections.is_empty() {
            let mut by_pattern: HashMap<String, Vec<String>> = HashMap::new();
            for d in &detections {
                let label = format!("{}", d.pattern);
                by_pattern.entry(label).or_default()
                    .push(format!("  {} (c={:.2}): {}", d.class, d.confidence, d.description));
            }
            let mut keys: Vec<_> = by_pattern.keys().collect();
            keys.sort();
            for k in keys {
                println!("  {} ({} hits):", k, by_pattern[k].len());
                for line in &by_pattern[k] {
                    println!("{}", line);
                }
            }
        }

        // Per-project call graph: show internal calls by class
        if !cg.calls.is_empty() {
            println!("\n  --- Internal Call Graph ---");
            let mut by_caller: HashMap<String, Vec<(String, String, bool, Option<String>)>> = HashMap::new();
            for call in &cg.calls {
                by_caller.entry(call.caller_class.clone()).or_default()
                    .push((call.callee.clone(), call.target_expr.clone(), call.is_self_call, call.created_type.clone()));
            }
            let mut caller_classes: Vec<_> = by_caller.keys().collect();
            caller_classes.sort();
            for caller in &caller_classes {
                let calls = &by_caller[caller.as_str()];
                println!("    {} ->", caller);
                for (callee, target, is_self, created) in calls {
                    let via = if *is_self { "self" } else if target.is_empty() { "direct" } else { target.as_str() };
                    let suffix = if let Some(ref ct) = created { format!(" [new {}]", ct) } else { String::new() };
                    println!("      {}.{}{}", via, callee, suffix);
                }
            }
        }

        // Top called methods per project
        if !cg.calls.is_empty() {
            let mut callee_count: HashMap<String, usize> = HashMap::new();
            for call in &cg.calls {
                *callee_count.entry(call.callee.clone()).or_default() += 1;
            }
            let mut ranked: Vec<_> = callee_count.into_iter().collect();
            ranked.sort_by(|a, b| b.1.cmp(&a.1));
            println!("\n  Top called:");
            for (method, count) in ranked.iter().take(10) {
                println!("    {} ({}x)", method, count);
            }
        }
    }

    // Full solution summary
    let tg = all_tg.unwrap_or_default();
    println!("\n{}", "=".repeat(80));
    println!("FULL SOLUTION SUMMARY");
    println!("{}", "=".repeat(80));
    println!("Files: {}/{} (errors: {})", total_parsed, all_cs_files.len(), total_errors);
    println!("Classes: {}, Interfaces: {}, Call sites: {}",
        tg.classes.len(), tg.interfaces.len(), all_cg.calls.len());

    println!("\nTop called methods (cross-project):");
    let mut callee_count: HashMap<String, usize> = HashMap::new();
    for call in &all_cg.calls {
        *callee_count.entry(call.callee.clone()).or_default() += 1;
    }
    let mut ranked: Vec<_> = callee_count.into_iter().collect();
    ranked.sort_by(|a, b| b.1.cmp(&a.1));
    for (method, count) in ranked.iter().take(15) {
        println!("  {} ({} calls)", method, count);
    }
}

fn collect_cs_files(dir: &str, files: &mut Vec<String>) {
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                collect_cs_files(&path.to_string_lossy(), files);
            } else if path.extension().map_or(false, |e| e == "cs") {
                files.push(path.to_string_lossy().to_string());
            }
        }
    }
}
