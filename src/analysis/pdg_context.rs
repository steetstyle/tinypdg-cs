use std::collections::HashMap;
use std::fs;

use petgraph::graph::DiGraph;

use crate::cfg::builder::{build_cfg, BasicBlock, BlockEdge, BlockKind};
use crate::resolve::types::TypeGraph;

#[derive(Debug)]
pub struct PdgContext {
    sources: HashMap<String, String>,
    cfgs: HashMap<String, DiGraph<BasicBlock, BlockEdge>>,
}

impl PdgContext {
    pub fn empty() -> Self {
        PdgContext {
            sources: HashMap::new(),
            cfgs: HashMap::new(),
        }
    }

    pub fn build(project_path: &std::path::Path) -> Self {
        let mut ctx = PdgContext {
            sources: HashMap::new(),
            cfgs: HashMap::new(),
        };
        ctx.load_files(project_path);
        ctx
    }

    fn load_files(&mut self, dir: &std::path::Path) {
        if dir.is_file() {
            if let Some(ext) = dir.extension() {
                if ext == "cs" {
                    self.load_file(dir);
                }
            }
        } else if let Ok(entries) = fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    self.load_files(&path);
                } else if path.extension().map_or(false, |e| e == "cs") {
                    self.load_file(&path);
                }
            }
        }
    }

    fn load_file(&mut self, path: &std::path::Path) {
        let path_str = path.to_string_lossy().to_string();
        if let Ok(source) = fs::read_to_string(&path_str) {
            if let Ok(cfg) = build_cfg(&source) {
                self.sources.insert(path_str.clone(), source);
                self.cfgs.insert(path_str, cfg);
            }
        }
    }

    pub fn get_method_file(class: &str, method: &str, tg: &TypeGraph) -> Option<String> {
        let ci = tg.classes.get(class)?;
        ci.methods.iter().find(|m| m.method == method)
            .map(|m| m.file.clone())
            .filter(|f| !f.is_empty())
    }

    pub fn get_control_context(&self, file: &str, line: usize) -> Option<Vec<String>> {
        let cfg = self.cfgs.get(file)?;
        let source = self.sources.get(file)?;
        let source_lines: Vec<&str> = source.lines().collect();

        // Find which basic block contains this line
        let block = cfg.node_indices().find(|&n| {
            let b = &cfg[n];
            b.start_line <= line && line <= b.end_line
        })?;

        // Walk up control dependence predecessors to find conditions
        let mut conditions = Vec::new();
        let mut visited = std::collections::HashSet::new();
        let mut stack = vec![block];
        visited.insert(block);

        while let Some(n) = stack.pop() {
            for pred in cfg.neighbors_directed(n, petgraph::Direction::Incoming) {
                if !visited.insert(pred) { continue; }
                let b = &cfg[pred];
                match &b.kind {
                    BlockKind::Condition => {
                        let cond_text = Self::extract_block_text(&source_lines, b);
                        conditions.push(format!("when: if ({}) [L{}]", cond_text, b.start_line));
                    }
                    BlockKind::LoopHeader => {
                        let loop_text = Self::extract_block_text(&source_lines, b);
                        conditions.push(format!("when: {} [L{}]", loop_text, b.start_line));
                    }
                    BlockKind::Handler => {
                        let handler_text = Self::extract_block_text(&source_lines, b);
                        conditions.push(format!("when: {} [L{}]", handler_text, b.start_line));
                    }
                    _ => {
                        stack.push(pred);
                    }
                }
            }
        }

        if conditions.is_empty() {
            None
        } else {
            conditions.reverse();
            Some(conditions)
        }
    }

    pub fn get_data_context(&self, file: &str, line: usize) -> Option<Vec<String>> {
        let cfg = self.cfgs.get(file)?;
        let source = self.sources.get(file)?;
        let source_lines: Vec<&str> = source.lines().collect();

        let block = cfg.node_indices().find(|&n| {
            let b = &cfg[n];
            b.start_line <= line && line <= b.end_line
        })?;

        let block_text = Self::extract_block_text(&source_lines, &cfg[block]);

        // Scan for method call arguments: look for `methodName(arg1, arg2)` patterns
        let mut data_flow = Vec::new();
        for word in block_text.split_whitespace() {
            if word.contains('(') && word.contains(')') {
                // Extract argument expressions
                let paren_start = word.find('(').unwrap();
                let paren_end = word.rfind(')').unwrap();
                let args_text = &word[paren_start + 1..paren_end];
                if !args_text.is_empty() {
                    for arg in args_text.split(',') {
                        let arg = arg.trim();
                        if arg.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '.')
                            && !arg.is_empty()
                            && !arg.chars().next().map_or(true, |c| c.is_ascii_digit())
                        {
                            // This could be a variable — find where it was defined
                            let def_line = Self::find_definition(&source_lines, arg, line);
                            if let Some(dl) = def_line {
                                data_flow.push(format!("{} from L{}", arg, dl));
                            }
                        }
                    }
                }
            }
        }

        if data_flow.is_empty() { None } else { Some(data_flow) }
    }

    fn extract_block_text(lines: &[&str], block: &BasicBlock) -> String {
        if block.start_line == 0 || block.start_line > lines.len() {
            return String::new();
        }
        let start = block.start_line - 1;
        let end = block.end_line.min(lines.len());
        if start >= end { return String::new(); }
        lines[start..end].join(" ").trim().to_string()
    }

    fn find_definition(lines: &[&str], var: &str, up_to_line: usize) -> Option<usize> {
        for (i, line) in lines.iter().enumerate() {
            let line_num = i + 1;
            if line_num >= up_to_line { break; }
            // Match patterns like: `var x = ...`, `Type x = ...`, `x = ...`
            let trimmed = line.trim();
            if trimmed.starts_with(&format!("var {} ", var))
                || trimmed.starts_with(&format!("var {}=", var))
                || trimmed.contains(&format!(" {} =", var))
                || trimmed.contains(&format!(" {}\n=", var))
            {
                // Make sure it's not a usage on the right side
                let eq_pos = trimmed.find('=');
                if let Some(eq) = eq_pos {
                    let lhs = trimmed[..eq].trim();
                    if lhs == var || lhs.ends_with(&format!(" {}", var)) || lhs.starts_with(&format!("var {}", var)) {
                        return Some(line_num);
                    }
                }
            }
        }
        None
    }
}
