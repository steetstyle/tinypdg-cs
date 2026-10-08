use std::collections::HashMap;
use std::fs;
use std::path::Path;

use anyhow::Result;
use tree_sitter::Node;

use crate::parse::parser::parse_source;

#[derive(Debug, Clone, serde::Serialize)]
pub struct RouteEntry {
    pub http_method: String,
    pub path: String,
    pub class: String,
    pub handler: String,
    pub source: String,
}

#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct RouteTable {
    pub routes: Vec<RouteEntry>,
}

impl RouteTable {
    pub fn new() -> Self {
        RouteTable { routes: Vec::new() }
    }

    fn add(&mut self, entry: RouteEntry) {
        self.routes.push(entry);
    }
}

/// Diagnostics about what the extractor saw.
///
/// `inline_lambda_routes` counts routes that exist in source but cannot be
/// attributed to a named method, so a caller can surface that and make a low
/// mapping rate explainable instead of silently zero.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct ExtractionStats {
    pub files_parsed: usize,
    pub files_failed: usize,
    pub controller_routes: usize,
    pub minimal_api_routes: usize,
    pub inline_lambda_routes: usize,
}

pub fn extract(source_dir: &str) -> Result<RouteTable> {
    extract_with_stats(source_dir).map(|(t, _)| t)
}

pub fn extract_with_stats(source_dir: &str) -> Result<(RouteTable, ExtractionStats)> {
    let mut files = Vec::new();
    collect_cs_files(Path::new(source_dir), &mut files);

    let mut table = RouteTable::new();
    let mut stats = ExtractionStats::default();
    for file in &files {
        let source = match fs::read_to_string(file) {
            Ok(s) => s,
            Err(_) => {
                stats.files_failed += 1;
                continue;
            }
        };
        let tree = match parse_source(&source) {
            Ok(t) => t,
            Err(_) => {
                stats.files_failed += 1;
                continue;
            }
        };
        stats.files_parsed += 1;

        let before = table.routes.len();
        extract_controller_routes(tree.root_node(), &source, &mut table);
        stats.controller_routes += table.routes.len() - before;

        let before = table.routes.len();
        extract_minimal_api_routes(tree.root_node(), &source, &mut table, &mut stats);
        stats.minimal_api_routes += table.routes.len() - before;
    }
    Ok((table, stats))
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

fn extract_controller_routes(node: Node, source: &str, table: &mut RouteTable) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "class_declaration" {
            if let Some(class_name) = child.child_by_field_name("name")
                .and_then(|n| n.utf8_text(source.as_bytes()).ok())
                .map(|s| s.to_string())
            {
                if !is_controller_class(&child, source, &class_name) {
                    continue;
                }
                let base_path = extract_route_attribute(&child, source);
                let mut m_cursor = child.walk();
                for decl in child.children(&mut m_cursor) {
                    if decl.kind() == "declaration_list" {
                        extract_methods_from_declaration_list(decl, source, &class_name, &base_path, table);
                    }
                }
            }
        } else {
            extract_controller_routes(child, source, table);
        }
    }
}

fn is_controller_class(class_node: &Node, source: &str, class_name: &str) -> bool {
    if class_name.ends_with("Controller") {
        return true;
    }
    let mut cursor = class_node.walk();
    for child in class_node.children(&mut cursor) {
        if child.kind() == "base_list" {
            let mut b_cursor = child.walk();
            for base in child.children(&mut b_cursor) {
                if base.kind() == "identifier" {
                    if let Ok(text) = base.utf8_text(source.as_bytes()) {
                        if text == "Controller" || text == "ControllerBase" {
                            return true;
                        }
                    }
                }
            }
        }
    }
    false
}

fn extract_methods_from_declaration_list(
    node: Node,
    source: &str,
    class_name: &str,
    base_path: &Option<String>,
    table: &mut RouteTable,
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "method_declaration" {
            if let Some(method_name) = child.child_by_field_name("name")
                .and_then(|n| n.utf8_text(source.as_bytes()).ok())
                .map(|s| s.to_string())
            {
                let http_methods = extract_http_method_attributes(&child, source);
                for (http_method, sub_path) in http_methods {
                    let full_path = combine_paths(base_path, &sub_path);
                    table.add(RouteEntry {
                        http_method,
                        path: normalize_route_pattern(&full_path),
                        class: class_name.to_string(),
                        handler: method_name.clone(),
                        source: "Controller".into(),
                    });
                }
            }
        } else {
            extract_methods_from_declaration_list(child, source, class_name, base_path, table);
        }
    }
}

fn extract_route_attribute(class_node: &Node, source: &str) -> Option<String> {
    let mut cursor = class_node.walk();
    for child in class_node.children(&mut cursor) {
        if child.kind() == "attribute_list" {
            let mut a_cursor = child.walk();
            for attr in child.children(&mut a_cursor) {
                if attr.kind() == "attribute" {
                    if let Some(attr_name) = attr.child(0)
                        .and_then(|n| n.utf8_text(source.as_bytes()).ok())
                    {
                        if attr_name == "Route" {
                            return extract_string_argument(&attr, source);
                        }
                    }
                }
            }
        }
    }
    None
}

fn extract_http_method_attributes(
    method_node: &Node,
    source: &str,
) -> Vec<(String, Option<String>)> {
    let mut results = Vec::new();
    let mut cursor = method_node.walk();
    for child in method_node.children(&mut cursor) {
        if child.kind() == "attribute_list" {
            let mut a_cursor = child.walk();
            for attr in child.children(&mut a_cursor) {
                if attr.kind() == "attribute" {
                    if let Some(attr_name) = attr.child(0)
                        .and_then(|n| n.utf8_text(source.as_bytes()).ok())
                    {
                        let http_method = match attr_name {
                            "HttpGet" => Some("GET"),
                            "HttpPost" => Some("POST"),
                            "HttpPut" => Some("PUT"),
                            "HttpDelete" => Some("DELETE"),
                            "HttpPatch" => Some("PATCH"),
                            _ => None,
                        };
                        if let Some(m) = http_method {
                            let sub_path = extract_string_argument(&attr, source);
                            results.push((m.to_string(), sub_path));
                        }
                    }
                }
            }
        }
    }
    if results.is_empty() {
        results.push(("GET".to_string(), None));
    }
    results
}

fn extract_string_argument(attr_node: &Node, source: &str) -> Option<String> {
    let mut cursor = attr_node.walk();
    for child in attr_node.children(&mut cursor) {
        if child.kind() == "attribute_argument_list" {
            let mut aa_cursor = child.walk();
            for arg in child.children(&mut aa_cursor) {
                if arg.kind() == "attribute_argument" {
                    let mut arg_cursor = arg.walk();
                    for inner in arg.children(&mut arg_cursor) {
                        if inner.kind() == "string_literal" {
                            if let Some(content) = extract_string_content(&inner, source) {
                                return Some(content);
                            }
                        }
                    }
                }
            }
        }
    }
    None
}

fn extract_string_content(node: &Node, source: &str) -> Option<String> {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "string_literal_content" {
            return child.utf8_text(source.as_bytes()).ok().map(|s| s.to_string());
        }
    }
    None
}

fn combine_paths(base: &Option<String>, sub: &Option<String>) -> String {
    let base = base.as_deref().unwrap_or("");
    let sub = sub.as_deref().unwrap_or("");

    if sub.starts_with('/') {
        return ensure_leading_slash(&sub);
    }

    let mut parts = Vec::new();
    if !base.is_empty() {
        parts.push(base.trim_matches('/'));
    }
    if !sub.is_empty() {
        parts.push(sub.trim_matches('/'));
    }

    if parts.is_empty() {
        "/".to_string()
    } else {
        ensure_leading_slash(&parts.join("/"))
    }
}

fn ensure_leading_slash(s: &str) -> String {
    if s.starts_with('/') {
        s.to_string()
    } else {
        format!("/{}", s)
    }
}

// ───────────────────────── minimal API ─────────────────────────

/// How a minimal-API handler argument resolves to a named method.
enum HandlerRef {
    /// `SomeEndpoint.Handler` — the one-shot idiom. The class is explicit and
    /// usually lives in a different file from the registration site.
    Qualified { class: String, method: String },
    /// `Handler` resolved against the enclosing class (e.g. inside Startup).
    Local { class: String, method: String },
}

const VERBS: &[(&str, &str)] = &[
    ("MapGet", "GET"),
    ("MapPost", "POST"),
    ("MapPut", "PUT"),
    ("MapDelete", "DELETE"),
    ("MapPatch", "PATCH"),
];

fn verb_for(name: &str) -> Option<&'static str> {
    VERBS.iter().find(|(k, _)| *k == name).map(|(_, v)| *v)
}

/// Normalize an ASP.NET route template so consumers can match it with plain
/// segment logic: inline constraints (`{id:guid}`), optional markers (`{id?}`)
/// and catch-all stars (`{*slug}`) all collapse to a bare `{name}` parameter.
fn normalize_route_pattern(path: &str) -> String {
    let mut out = String::with_capacity(path.len());
    let mut chars = path.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '{' {
            out.push(c);
            continue;
        }
        // Consume until the closing '}' (route params cannot nest).
        let mut inner = String::new();
        for c2 in chars.by_ref() {
            if c2 == '}' {
                break;
            }
            inner.push(c2);
        }
        let trimmed = inner.trim_end_matches('?').trim_start_matches('*');
        let name = trimmed.split(':').next().unwrap_or(trimmed).trim();
        out.push('{');
        out.push_str(name);
        out.push('}');
    }
    out
}

/// Join a `MapGroup` prefix with a route path.
fn join_prefix(prefix: &str, path: &str) -> String {
    let prefix = prefix.trim_matches('/');
    if prefix.is_empty() {
        return ensure_leading_slash(path);
    }
    let path = path.trim_matches('/');
    if path.is_empty() {
        return format!("/{}", prefix);
    }
    format!("/{}/{}", prefix, path)
}

/// The dotted method name being invoked, e.g. `MapGet` for `app.MapGet(...)`.
fn member_name(node: Node, source: &str) -> Option<String> {
    if node.kind() != "member_access_expression" {
        return None;
    }
    node.child_by_field_name("name")
        .and_then(|n| n.utf8_text(source.as_bytes()).ok())
        .map(|s| s.to_string())
}

fn node_text(node: Node, source: &str) -> Option<String> {
    node.utf8_text(source.as_bytes()).ok().map(|s| s.to_string())
}

/// First string-literal argument of an `argument_list`.
fn first_string_arg(args: Node, source: &str) -> Option<String> {
    let mut cursor = args.walk();
    for arg in args.children(&mut cursor) {
        if arg.kind() != "argument" {
            continue;
        }
        let mut ac = arg.walk();
        for inner in arg.children(&mut ac) {
            if inner.kind() == "string_literal" {
                if let Some(content) = extract_string_content(&inner, source) {
                    return Some(content);
                }
            }
        }
    }
    None
}

/// Find a `MapGroup("prefix")` call anywhere in `node`'s subtree, so that
/// `var g = app.MapGroup("/api").WithTags(...)` still yields "/api".
fn find_map_group_prefix(node: Node, source: &str) -> Option<String> {
    if node.kind() == "invocation_expression" {
        if let Some(func) = node.child_by_field_name("function") {
            if member_name(func, source).as_deref() == Some("MapGroup") {
                if let Some(args) = node.child_by_field_name("arguments") {
                    if let Some(p) = first_string_arg(args, source) {
                        return Some(p);
                    }
                }
            }
        }
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if let Some(found) = find_map_group_prefix(child, source) {
            return Some(found);
        }
    }
    None
}

/// Record `variable = MapGroup("prefix")` bindings, e.g.
/// `var agencyApi = app.MapGroup("").WithResourceAuthorization();`
fn collect_route_groups(node: Node, source: &str, groups: &mut HashMap<String, String>) {
    let mut cursor = node.walk();
    let children: Vec<Node> = node.children(&mut cursor).collect();
    for child in children {
        if child.kind() == "variable_declarator" {
            let name_node = child.child_by_field_name("name");
            if let Some(name) = name_node.and_then(|n| node_text(n, source)) {
                let mut probe = child.walk();
                let values: Vec<Node> = child.children(&mut probe).collect();
                for value in values {
                    // Skip the declarator's own name node.
                    if Some(value.id()) == name_node.map(|n| n.id()) {
                        continue;
                    }
                    if let Some(prefix) = find_map_group_prefix(value, source) {
                        groups.insert(name.clone(), prefix);
                        break;
                    }
                }
            }
        }
        collect_route_groups(child, source, groups);
    }
}

/// Resolve the handler argument (2nd argument of a Map* call).
fn resolve_handler(args: Node, enclosing: Node, source: &str) -> Option<HandlerRef> {
    let mut cursor = args.walk();
    let mut positional: Vec<Node> = Vec::new();
    for arg in args.children(&mut cursor) {
        if arg.kind() == "argument" {
            positional.push(arg);
        }
    }
    let handler_arg = *positional.get(1)?;

    let mut ac = handler_arg.walk();
    let expr = handler_arg.children(&mut ac).find(|c| {
        matches!(
            c.kind(),
            "member_access_expression"
                | "identifier"
                | "generic_name"
                | "lambda_expression"
                | "anonymous_method_expression"
                | "parenthesized_lambda_expression"
        )
    })?;

    match expr.kind() {
        "member_access_expression" => {
            let method = expr
                .child_by_field_name("name")
                .and_then(|n| node_text(n, source))?
                .trim_start_matches('@')
                .to_string();
            // `Type.Handler`, or `Namespace.Type.Handler` — keep the last two parts.
            let qualifier = expr
                .child_by_field_name("expression")
                .and_then(|n| node_text(n, source))?;
            let class = qualifier
                .rsplit('.')
                .next()
                .unwrap_or(&qualifier)
                .trim_start_matches('@')
                .to_string();
            if class.is_empty() || method.is_empty() {
                return None;
            }
            Some(HandlerRef::Qualified { class, method })
        }
        "identifier" => {
            let method = node_text(expr, source)?.trim_start_matches('@').to_string();
            let class = resolve_local_function_class(enclosing, source).unwrap_or_default();
            Some(HandlerRef::Local { class, method })
        }
        _ => None,
    }
}

fn extract_minimal_api_routes(
    node: Node,
    source: &str,
    table: &mut RouteTable,
    stats: &mut ExtractionStats,
) {
    // Route group variables are file-scoped: collect them ONCE from the whole
    // tree, then scan. Doing the collection inside the recursion would rebuild
    // the map per subtree and lose groups declared above the current node.
    let mut groups: HashMap<String, String> = HashMap::new();
    collect_route_groups(node, source, &mut groups);
    scan_minimal_api_routes(node, source, table, stats, &groups);
}

fn scan_minimal_api_routes(
    node: Node,
    source: &str,
    table: &mut RouteTable,
    stats: &mut ExtractionStats,
    groups: &HashMap<String, String>,
) {
    let mut cursor = node.walk();
    let children: Vec<Node> = node.children(&mut cursor).collect();
    for child in children {
        // Inspect only this invocation's OWN callee. In a chained call such as
        // `app.MapPost("/x", H).WithValidation<V>()` the outer callee is
        // WithValidation and the inner MapPost invocation is reached by the
        // recursion below, so every Map* call is registered exactly once.
        if child.kind() == "invocation_expression" {
            if let Some(func) = child.child_by_field_name("function") {
                if let Some(callee) = member_name(func, source) {
                    if let Some(verb) = verb_for(&callee) {
                        if let Some(args) = child.child_by_field_name("arguments") {
                            if let Some(raw_path) = first_string_arg(args, source) {
                                let prefix = resolve_prefix(func, source, groups);
                                let full_path =
                                    normalize_route_pattern(&join_prefix(&prefix, &raw_path));

                                match resolve_handler(args, child, source) {
                                    Some(HandlerRef::Qualified { class, method })
                                    | Some(HandlerRef::Local { class, method }) => {
                                        table.add(RouteEntry {
                                            http_method: verb.to_string(),
                                            path: full_path,
                                            class,
                                            handler: method,
                                            source: "MinimalApi".into(),
                                        });
                                    }
                                    // The route exists but is an inline lambda,
                                    // so there is no named method to point at.
                                    // Count it instead of inventing an entry.
                                    None => {
                                        stats.inline_lambda_routes += 1;
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        scan_minimal_api_routes(child, source, table, stats, groups);
    }
}

/// Prefix for a Map* call, given its callee member-access node.
fn resolve_prefix(
    func: Node,
    source: &str,
    groups: &HashMap<String, String>,
) -> String {
    func.child_by_field_name("expression")
        .and_then(|expr| {
            if expr.kind() == "identifier" {
                // `api.MapGet(...)` where `api` came from a MapGroup binding.
                node_text(expr, source).and_then(|n| groups.get(&n).cloned())
            } else {
                // `app.MapGroup("/x").MapGet(...)`
                find_map_group_prefix(expr, source)
            }
        })
        .unwrap_or_default()
}

fn resolve_local_function_class(node: Node, source: &str) -> Option<String> {
    let mut current = Some(node);
    while let Some(n) = current {
        if n.kind() == "class_declaration" {
            return n.child_by_field_name("name")
                .and_then(|name| name.utf8_text(source.as_bytes()).ok())
                .map(|s| s.to_string());
        }
        current = n.parent();
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn min_routes(src: &str) -> RouteTable {
        let tree = parse_source(src).unwrap();
        let mut table = RouteTable::new();
        let mut stats = ExtractionStats::default();
        extract_minimal_api_routes(tree.root_node(), src, &mut table, &mut stats);
        table
    }

    fn min_routes_with_stats(src: &str) -> (RouteTable, ExtractionStats) {
        let tree = parse_source(src).unwrap();
        let mut table = RouteTable::new();
        let mut stats = ExtractionStats::default();
        extract_minimal_api_routes(tree.root_node(), src, &mut table, &mut stats);
        (table, stats)
    }

    #[test]
    fn test_extract_controller_routes() {
        let src = r#"
[Route("api/catalog")]
public class CatalogApi : ControllerBase
{
    [HttpGet("{id}")]
    public IActionResult GetItemById(int id) { return Ok(); }

    [HttpPost]
    public IActionResult CreateItem([FromBody] object req) { return Ok(); }
}
"#;
        let tree = parse_source(src).unwrap();
        let mut table = RouteTable::new();
        extract_controller_routes(tree.root_node(), src, &mut table);
        assert_eq!(table.routes.len(), 2);

        let get = table.routes.iter().find(|r| r.http_method == "GET").unwrap();
        assert_eq!(get.path, "/api/catalog/{id}");
        assert_eq!(get.class, "CatalogApi");
        assert_eq!(get.handler, "GetItemById");
        assert_eq!(get.source, "Controller");

        let post = table.routes.iter().find(|r| r.http_method == "POST").unwrap();
        assert_eq!(post.path, "/api/catalog");
        assert_eq!(post.class, "CatalogApi");
        assert_eq!(post.handler, "CreateItem");
    }

    #[test]
    fn test_extract_controller_routes_no_route_attr() {
        let src = r#"
public class CatalogApi : ControllerBase
{
    [HttpGet("{id}")]
    public IActionResult GetItemById(int id) { return Ok(); }
}
"#;
        let tree = parse_source(src).unwrap();
        let mut table = RouteTable::new();
        extract_controller_routes(tree.root_node(), src, &mut table);
        assert_eq!(table.routes.len(), 1);
        assert_eq!(table.routes[0].path, "/{id}");
        assert_eq!(table.routes[0].http_method, "GET");
    }

    #[test]
    fn test_extract_controller_routes_no_method_attr() {
        let src = r#"
[Route("api/items")]
public class ItemsApi : ControllerBase
{
    public IActionResult GetAll() { return Ok(); }
}
"#;
        let tree = parse_source(src).unwrap();
        let mut table = RouteTable::new();
        extract_controller_routes(tree.root_node(), src, &mut table);
        assert_eq!(table.routes.len(), 1);
        assert_eq!(table.routes[0].http_method, "GET");
        assert_eq!(table.routes[0].path, "/api/items");
        assert_eq!(table.routes[0].handler, "GetAll");
    }

    #[test]
    fn test_extract_controller_multiple_http_methods() {
        let src = r#"
[Route("api/catalog")]
public class CatalogApi : ControllerBase
{
    [HttpGet("{id}")]
    public IActionResult GetById(int id) { return Ok(); }

    [HttpPut("{id}")]
    public IActionResult Update(int id) { return Ok(); }

    [HttpDelete("{id}")]
    public IActionResult Delete(int id) { return Ok(); }
}
"#;
        let tree = parse_source(src).unwrap();
        let mut table = RouteTable::new();
        extract_controller_routes(tree.root_node(), src, &mut table);
        assert_eq!(table.routes.len(), 3);
        let methods: Vec<&str> = table.routes.iter().map(|r| r.http_method.as_str()).collect();
        assert!(methods.contains(&"GET"));
        assert!(methods.contains(&"PUT"));
        assert!(methods.contains(&"DELETE"));
        for r in &table.routes {
            assert_eq!(r.path, "/api/catalog/{id}");
        }
    }

    #[test]
    fn test_extract_minimal_api_routes() {
        let src = r#"
app.MapGet("/items", GetItemById);
app.MapPost("/items", CreateItem);
"#;
        let table = min_routes(src);
        assert_eq!(table.routes.len(), 2);

        let get = table.routes.iter().find(|r| r.http_method == "GET").unwrap();
        assert_eq!(get.path, "/items");
        assert_eq!(get.handler, "GetItemById");
        assert_eq!(get.source, "MinimalApi");

        let post = table.routes.iter().find(|r| r.http_method == "POST").unwrap();
        assert_eq!(post.path, "/items");
        assert_eq!(post.handler, "CreateItem");
    }

    #[test]
    fn test_extract_minimal_api_from_class() {
        let src = r#"
public class Startup
{
    public void Configure(IApplicationBuilder app)
    {
        app.MapGet("/api/items", GetAllItems);
        app.MapPost("/api/items", CreateItem);
    }

    public IResult GetAllItems() { return Results.Ok(); }
    public IResult CreateItem(object req) { return Results.Ok(); }
}
"#;
        let table = min_routes(src);
        assert_eq!(table.routes.len(), 2);
        for r in &table.routes {
            assert_eq!(r.class, "Startup");
            assert!(r.handler == "GetAllItems" || r.handler == "CreateItem");
        }
    }

    #[test]
    fn test_extract_minimal_api_put_delete_patch() {
        let src = r#"
app.MapPut("/items/{id}", UpdateItem);
app.MapDelete("/items/{id}", DeleteItem);
app.MapPatch("/items/{id}", PatchItem);
"#;
        let table = min_routes(src);
        assert_eq!(table.routes.len(), 3);
        assert!(table.routes.iter().any(|r| r.http_method == "PUT"));
        assert!(table.routes.iter().any(|r| r.http_method == "DELETE"));
        assert!(table.routes.iter().any(|r| r.http_method == "PATCH"));
        for r in &table.routes {
            assert!(r.path.starts_with("/items"));
        }
    }

    #[test]
    fn test_extract_integrated() {
        let src = r#"
using Microsoft.AspNetCore.Mvc;

[Route("api/catalog")]
public class CatalogApi : ControllerBase
{
    [HttpGet("{id}")]
    public IActionResult GetItemById(int id) { return Ok(); }

    [HttpPost]
    public IActionResult CreateItem([FromBody] object req) { return Ok(); }
}

public class Startup
{
    public void Configure(IApplicationBuilder app)
    {
        app.MapGet("/api/items", GetAllItems);
    }

    public IResult GetAllItems() { return Results.Ok(); }
}
"#;
        let tree = parse_source(src).unwrap();
        let mut table = RouteTable::new();
        extract_controller_routes(tree.root_node(), src, &mut table);
        extract_minimal_api_routes(tree.root_node(), src, &mut table, &mut ExtractionStats::default());
        assert_eq!(table.routes.len(), 3);

        let controller_routes: Vec<&RouteEntry> = table.routes.iter().filter(|r| r.source == "Controller").collect();
        assert_eq!(controller_routes.len(), 2);

        let minimal_routes: Vec<&RouteEntry> = table.routes.iter().filter(|r| r.source == "MinimalApi").collect();
        assert_eq!(minimal_routes.len(), 1);
        assert_eq!(minimal_routes[0].class, "Startup");
        assert_eq!(minimal_routes[0].handler, "GetAllItems");
    }

    #[test]
    fn test_combine_paths() {
        assert_eq!(combine_paths(&None, &None), "/");
        assert_eq!(combine_paths(&Some("api".into()), &None), "/api");
        assert_eq!(combine_paths(&Some("api/catalog".into()), &Some("{id}".into())), "/api/catalog/{id}");
        assert_eq!(combine_paths(&None, &Some("{id}".into())), "/{id}");
        assert_eq!(combine_paths(&Some("api/".into()), &Some("/{id}".into())), "/{id}");
        assert_eq!(combine_paths(&Some("api".into()), &Some("items".into())), "/api/items");
    }

    #[test]
    fn test_collect_cs_files_finds_nothing() {
        let mut files = Vec::new();
        collect_cs_files(Path::new("/nonexistent"), &mut files);
        assert!(files.is_empty());
    }

    #[test]
    fn test_extract_http_get_without_route_arg() {
        let src = r#"
[Route("api/values")]
public class ValuesController : ControllerBase
{
    [HttpGet]
    public IActionResult GetAll() { return Ok(); }
}
"#;
        let tree = parse_source(src).unwrap();
        let mut table = RouteTable::new();
        extract_controller_routes(tree.root_node(), src, &mut table);
        assert_eq!(table.routes.len(), 1);
        assert_eq!(table.routes[0].http_method, "GET");
        assert_eq!(table.routes[0].path, "/api/values");
        assert_eq!(table.routes[0].handler, "GetAll");
    }

    #[test]
    fn test_extract_http_post_without_route_arg() {
        let src = r#"
[Route("api/orders")]
public class OrdersController : ControllerBase
{
    [HttpPost]
    public IActionResult Create([FromBody] object req) { return Ok(); }
}
"#;
        let tree = parse_source(src).unwrap();
        let mut table = RouteTable::new();
        extract_controller_routes(tree.root_node(), src, &mut table);
        assert_eq!(table.routes.len(), 1);
        assert_eq!(table.routes[0].http_method, "POST");
        assert_eq!(table.routes[0].path, "/api/orders");
    }

    #[test]
    fn test_extract_http_methods_without_route_no_base_route() {
        let src = r#"
public class ProductsController : ControllerBase
{
    [HttpGet]
    public IActionResult GetAll() { return Ok(); }

    [HttpPost]
    public IActionResult Create([FromBody] object req) { return Ok(); }

    [HttpPut("{id}")]
    public IActionResult Update(int id) { return Ok(); }
}
"#;
        let tree = parse_source(src).unwrap();
        let mut table = RouteTable::new();
        extract_controller_routes(tree.root_node(), src, &mut table);
        assert_eq!(table.routes.len(), 3);

        let get = table.routes.iter().find(|r| r.http_method == "GET").unwrap();
        assert_eq!(get.path, "/");

        let post = table.routes.iter().find(|r| r.http_method == "POST").unwrap();
        assert_eq!(post.path, "/");

        let put = table.routes.iter().find(|r| r.http_method == "PUT").unwrap();
        assert_eq!(put.path, "/{id}");
    }

    #[test]
    fn test_extract_minimal_api_leading_slash() {
        let src = r#"app.MapGet("items", GetItems);"#;
        let table = min_routes(src);
        assert_eq!(table.routes.len(), 1);
        assert_eq!(table.routes[0].path, "/items");
    }

    #[test]
    fn test_extract_controller_no_routes_no_attrs() {
        let src = r#"
[Route("api/test")]
public class TestController : ControllerBase
{
    public IActionResult DoSomething() { return Ok(); }
}
"#;
        let tree = parse_source(src).unwrap();
        let mut table = RouteTable::new();
        extract_controller_routes(tree.root_node(), src, &mut table);
        assert_eq!(table.routes.len(), 1);
        assert_eq!(table.routes[0].http_method, "GET");
        assert_eq!(table.routes[0].path, "/api/test");
        assert_eq!(table.routes[0].handler, "DoSomething");
    }

    // ── one-shot endpoint idiom: MapGet("/path", Type.Handler) ──

    #[test]
    fn test_qualified_handler_resolves_class_and_method() {
        let src = r#"
public static class EndpointExtension
{
    public static IEndpointRouteBuilder MapApi(this IEndpointRouteBuilder app)
    {
        app.MapGet("/api/agency/{agencyId:guid}/member",
            GetAgencyMembersEndpoint.Handler);
        return app;
    }
}
"#;
        let table = min_routes(src);
        assert_eq!(table.routes.len(), 1);
        let r = &table.routes[0];
        assert_eq!(r.http_method, "GET");
        // Inline constraint stripped so segment matching works downstream.
        assert_eq!(r.path, "/api/agency/{agencyId}/member");
        assert_eq!(r.class, "GetAgencyMembersEndpoint");
        assert_eq!(r.handler, "Handler");
    }

    #[test]
    fn test_qualified_handler_with_namespace_qualifier() {
        let src = r#"
public class Startup
{
    public void Configure(IApplicationBuilder app)
    {
        app.MapPost("/api/orders", Agency.API.Endpoints.PostOrderEndpoint.HandlerAsync);
    }
}
"#;
        let table = min_routes(src);
        assert_eq!(table.routes.len(), 1);
        assert_eq!(table.routes[0].class, "PostOrderEndpoint");
        assert_eq!(table.routes[0].handler, "HandlerAsync");
    }

    #[test]
    fn test_chained_call_registers_route_once() {
        let src = r#"
public static class EndpointExtension
{
    public static IEndpointRouteBuilder MapApi(this IEndpointRouteBuilder app)
    {
        app.MapPost("/api/agency",
                PostCreateAgencyEndpoint.Handler)
            .WithValidation<PostCreateAgencyEndpoint.CreateAgencyRequest>();
        return app;
    }
}
"#;
        let table = min_routes(src);
        assert_eq!(table.routes.len(), 1, "chained call must not duplicate the route");
        assert_eq!(table.routes[0].path, "/api/agency");
        assert_eq!(table.routes[0].class, "PostCreateAgencyEndpoint");
    }

    #[test]
    fn test_map_group_prefix_from_variable() {
        let src = r#"
public static class EndpointExtension
{
    public static IEndpointRouteBuilder MapApi(this IEndpointRouteBuilder app)
    {
        var api = app.MapGroup("/api/v1").WithTags("v1");
        api.MapGet("/items", ItemsEndpoint.Handler);
        return app;
    }
}
"#;
        let table = min_routes(src);
        assert_eq!(table.routes.len(), 1);
        assert_eq!(table.routes[0].path, "/api/v1/items");
    }

    #[test]
    fn test_map_group_empty_prefix_keeps_path() {
        let src = r#"
public static class EndpointExtension
{
    public static IEndpointRouteBuilder MapApi(this IEndpointRouteBuilder app)
    {
        var agencyApi = app.MapGroup("").WithResourceAuthorization();
        agencyApi.MapGet("/api/agency/{agencyId:guid}/tier", GetTierEndpoint.Handler);
        return app;
    }
}
"#;
        let table = min_routes(src);
        assert_eq!(table.routes.len(), 1);
        assert_eq!(table.routes[0].path, "/api/agency/{agencyId}/tier");
    }

    #[test]
    fn test_map_group_inline_chaining() {
        let src = r#"
public class Program
{
    public static void Main(string[] args)
    {
        var app = WebApplication.CreateBuilder(args).Build();
        app.MapGroup("/api/v2").MapGet("/cost", CostEndpoint.Handler);
    }
}
"#;
        let table = min_routes(src);
        assert_eq!(table.routes.len(), 1);
        assert_eq!(table.routes[0].path, "/api/v2/cost");
        assert_eq!(table.routes[0].class, "CostEndpoint");
    }

    #[test]
    fn test_inline_lambda_counted_not_invented() {
        let src = r#"
public class Program
{
    public static void Main(string[] args)
    {
        var app = WebApplication.CreateBuilder(args).Build();
        app.MapGet("/cost", async (HttpContext ctx) => { return Results.Ok(); });
    }
}
"#;
        let (table, stats) = min_routes_with_stats(src);
        assert!(table.routes.is_empty(), "lambda handler has no named method");
        assert_eq!(stats.inline_lambda_routes, 1);
    }

    #[test]
    fn test_normalize_route_pattern() {
        assert_eq!(normalize_route_pattern("/api/{id}"), "/api/{id}");
        assert_eq!(normalize_route_pattern("/api/{id:guid}"), "/api/{id}");
        assert_eq!(normalize_route_pattern("/api/{id:int:min(1)}"), "/api/{id}");
        assert_eq!(normalize_route_pattern("/api/{id?}"), "/api/{id}");
        assert_eq!(normalize_route_pattern("/files/{*path}"), "/files/{path}");
        assert_eq!(normalize_route_pattern("/files/{**path}"), "/files/{path}");
        assert_eq!(normalize_route_pattern("/api/items"), "/api/items");
        // Two params on one segment are not valid ASP.NET syntax, but make sure
        // we do not loop or panic.
        assert_eq!(normalize_route_pattern("/{a}/{b}"), "/{a}/{b}");
    }

    #[test]
    fn test_join_prefix() {
        assert_eq!(join_prefix("", "/items"), "/items");
        assert_eq!(join_prefix("", "items"), "/items");
        assert_eq!(join_prefix("/api", "/items"), "/api/items");
        assert_eq!(join_prefix("api/", "items/"), "/api/items");
        assert_eq!(join_prefix("/api", "/"), "/api");
    }

    #[test]
    fn test_unicpeak_style_registration_file() {
        // Mirrors the real shape: many one-shot endpoints registered in one
        // extension class, each with an inline constraint parameter.
        let src = r#"
using Agency.API.Endpoints.Members;

namespace Agency.API.Extensions;

public static class EndpointExtension
{
    public static IEndpointRouteBuilder MapAgencyApi(this IEndpointRouteBuilder app)
    {
        var agencyApi = app.MapGroup("").WithResourceAuthorization();

        agencyApi.MapPost("/api/agency", PostCreateAgencyEndpoint.Handler)
            .WithValidation<PostCreateAgencyEndpoint.CreateAgencyRequest>();
        agencyApi.MapGet("/api/agency/{agencyId:guid}/member",
            GetAgencyMembersEndpoint.Handler);
        agencyApi.MapDelete("/api/agency/{agencyId:guid}/member/{memberId:guid}",
            RemoveMemberEndpoint.Handler);

        return app;
    }
}
"#;
        let (table, stats) = min_routes_with_stats(src);
        assert_eq!(stats.inline_lambda_routes, 0);
        assert_eq!(table.routes.len(), 3);

        let get = table.routes.iter().find(|r| r.http_method == "GET").unwrap();
        assert_eq!(get.path, "/api/agency/{agencyId}/member");
        assert_eq!(get.class, "GetAgencyMembersEndpoint");
        assert_eq!(get.handler, "Handler");

        let del = table.routes.iter().find(|r| r.http_method == "DELETE").unwrap();
        assert_eq!(del.path, "/api/agency/{agencyId}/member/{memberId}");
        assert_eq!(del.class, "RemoveMemberEndpoint");

        let post = table.routes.iter().find(|r| r.http_method == "POST").unwrap();
        assert_eq!(post.path, "/api/agency");
        assert_eq!(post.class, "PostCreateAgencyEndpoint");
    }
}
