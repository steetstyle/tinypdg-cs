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
    let eshop_root = "/tmp/eShop/src";
    let mut all_cs_files: Vec<String> = Vec::new();
    collect_cs_files(eshop_root, &mut all_cs_files);
    println!("Total C# files found: {}\n", all_cs_files.len());

    let mut project_groups: HashMap<String, Vec<String>> = HashMap::new();
    for path in &all_cs_files {
        let rel = path.strip_prefix(eshop_root).unwrap_or(path);
        let proj = rel.split('/').next().unwrap_or("unknown").to_string();
        project_groups.entry(proj).or_default().push(path.clone());
    }

    for (proj, files) in &project_groups {
        println!("\n========== {} ({} files) ==========", proj, files.len());

        let mut cg = CallGraph::new();
        let mut combined_type_graph = None;
        let mut file_count = 0;
        let mut error_count = 0;

        for file in files {
            let source = match fs::read_to_string(file) {
                Ok(s) => s,
                Err(_) => { error_count += 1; continue; }
            };

            let tree = match parse_source(&source) {
                Ok(t) => t,
                Err(_) => { error_count += 1; continue; }
            };

            let st = match SymbolTable::from_ast(tree.root_node(), &source) {
                Ok(s) => s,
                Err(_) => { error_count += 1; continue; }
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
            for (k, v) in file_cg.method_calls {
                cg.method_calls.entry(k).or_default().extend(v);
            }

            if combined_type_graph.is_none() {
                combined_type_graph = Some(st.type_graph);
            } else if let Some(ref mut tg) = combined_type_graph {
                for (name, info) in st.type_graph.classes {
                    tg.classes.entry(name).or_insert(info);
                }
                for (name, info) in st.type_graph.interfaces {
                    tg.interfaces.entry(name).or_insert(info);
                }
            }

            file_count += 1;
        }

        if combined_type_graph.is_none() { continue; }
        let tg = combined_type_graph.unwrap();
        let ctx = DetectionContext::with_callgraph(&tg, &cg, "");

        let mut detections = Vec::new();
        detections.extend(detect_creational(&ctx));
        detections.extend(detect_structural(&ctx));
        detections.extend(detect_behavioral(&ctx));
        detections.extend(detect_dotnet(&ctx));

        println!("Parsed: {}/{} (errors: {})", file_count, files.len(), error_count);
        println!("Classes: {}, Interfaces: {}, Calls: {}", tg.classes.len(), tg.interfaces.len(), cg.calls.len());

        if !detections.is_empty() {
            println!("\nPatterns:");
            let mut by_pattern: HashMap<String, Vec<String>> = HashMap::new();
            for d in &detections {
                let label = format!("{:?}", d.pattern);
                by_pattern.entry(label).or_default()
                    .push(format!("  {} (c={:.2})", d.class, d.confidence));
            }
            let mut keys: Vec<_> = by_pattern.keys().collect();
            keys.sort();
            for k in keys {
                println!("  {}: {} hits", k, by_pattern[k].len());
                for line in &by_pattern[k] {
                    println!("{}", line);
                }
            }
        }

        if !cg.calls.is_empty() {
            let mut callee_count: HashMap<String, usize> = HashMap::new();
            for call in &cg.calls {
                *callee_count.entry(call.callee.clone()).or_default() += 1;
            }
            let mut ranked: Vec<_> = callee_count.into_iter().collect();
            ranked.sort_by(|a, b| b.1.cmp(&a.1));
            println!("\nTop called:");
            for (method, count) in ranked.iter().take(10) {
                println!("  {} ({}x)", method, count);
            }
        }
        println!();
    }

    // Full cross-project call graph
    println!("\n========== FULL SOLUTION CALL GRAPH ==========");
    let mut all_cg = CallGraph::new();
    let mut all_tg: Option<tiny_pdg_cs::resolve::types::TypeGraph> = None;
    let mut total_files = 0;
    let mut total_errors = 0;

    for file in &all_cs_files {
        let source = match fs::read_to_string(file) {
            Ok(s) => s,
            Err(_) => { total_errors += 1; continue; }
        };
        let tree = match parse_source(&source) {
            Ok(t) => t,
            Err(_) => { total_errors += 1; continue; }
        };
        let st = match SymbolTable::from_ast(tree.root_node(), &source) {
            Ok(s) => s,
            Err(_) => { total_errors += 1; continue; }
        };
        let file_cg = CallGraphBuilder::build(tree.root_node(), &source, &st.type_graph);
        for call in file_cg.calls {
            all_cg.calls.push(call);
        }
        for (k, v) in file_cg.class_callees {
            all_cg.class_callees.entry(k).or_default().extend(v);
        }
        for (k, v) in file_cg.class_creations {
            all_cg.class_creations.entry(k).or_default().extend(v);
        }
        if all_tg.is_none() {
            all_tg = Some(st.type_graph);
        } else if let Some(ref mut tg) = all_tg {
            for (name, info) in st.type_graph.classes {
                tg.classes.entry(name).or_insert(info);
            }
            for (name, info) in st.type_graph.interfaces {
                tg.interfaces.entry(name).or_insert(info);
            }
        }
        total_files += 1;
    }

    let tg = all_tg.unwrap_or_else(|| tiny_pdg_cs::resolve::types::TypeGraph::new());
    println!("Files: {}/{} (errors: {})", total_files, all_cs_files.len(), total_errors);
    println!("Classes: {}, Interfaces: {}", tg.classes.len(), tg.interfaces.len());
    println!("Total call sites: {}", all_cg.calls.len());

    let ctx = DetectionContext::with_callgraph(&tg, &all_cg, "");
    let mut detections = Vec::new();
    detections.extend(detect_creational(&ctx));
    detections.extend(detect_structural(&ctx));
    detections.extend(detect_behavioral(&ctx));
    detections.extend(detect_dotnet(&ctx));

    if !detections.is_empty() {
        println!("\nPattern detections (full solution):");
        let mut by_pattern: HashMap<String, Vec<String>> = HashMap::new();
        for d in &detections {
            let label = format!("{:?}", d.pattern);
            by_pattern.entry(label).or_default()
                .push(format!("  {} (c={:.2}) — {}", d.class, d.confidence, d.description));
        }
        let mut keys: Vec<_> = by_pattern.keys().collect();
        keys.sort();
        for k in keys {
            println!("  {}:", k);
            for line in &by_pattern[k] {
                println!("{}", line);
            }
        }
    }

    println!("\nTop called methods (cross-project):");
    let mut callee_count: HashMap<String, usize> = HashMap::new();
    for call in &all_cg.calls {
        *callee_count.entry(call.callee.clone()).or_default() += 1;
    }
    let mut ranked: Vec<_> = callee_count.into_iter().collect();
    ranked.sort_by(|a, b| b.1.cmp(&a.1));
    for (method, count) in ranked.iter().take(20) {
        println!("  {} ({} calls)", method, count);
    }

    println!("\nCreation sites:");
    let mut created: HashMap<String, usize> = HashMap::new();
    for call in &all_cg.calls {
        if call.is_creation {
            if let Some(ref ty) = call.created_type {
                *created.entry(ty.clone()).or_default() += 1;
            }
        }
    }
    let mut creaded: Vec<_> = created.into_iter().collect();
    creaded.sort_by(|a, b| b.1.cmp(&a.1));
    for (ty, count) in creaded.iter().take(10) {
        println!("  new {} ({} times)", ty, count);
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
