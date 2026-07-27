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

pub fn extract(source_dir: &str) -> Result<RouteTable> {
    let mut files = Vec::new();
    collect_cs_files(Path::new(source_dir), &mut files);

    let mut table = RouteTable::new();
    for file in &files {
        let source = match fs::read_to_string(file) {
            Ok(s) => s,
            Err(_) => continue,
        };
        let tree = match parse_source(&source) {
            Ok(t) => t,
            Err(_) => continue,
        };
        extract_controller_routes(tree.root_node(), &source, &mut table);
        extract_minimal_api_routes(tree.root_node(), &source, &mut table);
    }
    Ok(table)
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
                        path: full_path,
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

fn extract_minimal_api_routes(node: Node, source: &str, table: &mut RouteTable) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "invocation_expression" {
            if let Some(route) = extract_minimal_api_call(&child, source) {
                let class = resolve_local_function_class(&child, source);
                table.add(RouteEntry {
                    http_method: route.0,
                    path: route.1,
                    class: class.unwrap_or_default(),
                    handler: route.2,
                    source: "MinimalApi".into(),
                });
            }
        }
    }
    let mut cursor2 = node.walk();
    for child in node.children(&mut cursor2) {
        extract_minimal_api_routes(child, source, table);
    }
}

fn extract_minimal_api_call(node: &Node, source: &str) -> Option<(String, String, String)> {
    let mut callee = String::new();

    for i in 0..node.child_count() {
        if let Some(child) = node.child(i) {
            if child.kind() == "member_access_expression" {
                if let Some(name_node) = child.child_by_field_name("name") {
                    if let Ok(text) = name_node.utf8_text(source.as_bytes()) {
                        callee = text.to_string();
                    }
                }
            }
        }
    }

    let http_method = match callee.as_str() {
        "MapGet" => Some("GET"),
        "MapPost" => Some("POST"),
        "MapPut" => Some("PUT"),
        "MapDelete" => Some("DELETE"),
        "MapPatch" => Some("PATCH"),
        _ => None,
    }?;

    let mut args: Vec<String> = Vec::new();
    for i in 0..node.child_count() {
        if let Some(child) = node.child(i) {
            if child.kind() == "argument_list" {
                let mut a_cursor = child.walk();
                for arg in child.children(&mut a_cursor) {
                    if arg.kind() == "argument" {
                        let mut arg_cursor = arg.walk();
                        for inner in arg.children(&mut arg_cursor) {
                            if inner.kind() == "string_literal" {
                                if let Some(content) = extract_string_content(&inner, source) {
                                    args.push(content);
                                }
                            } else if inner.kind() == "identifier" {
                                if let Ok(text) = inner.utf8_text(source.as_bytes()) {
                                    args.push(text.to_string());
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    if args.len() < 2 {
        return None;
    }

    let path = ensure_leading_slash(&args[0]);
    let handler = args[1].clone();

    Some((http_method.to_string(), path, handler))
}

fn resolve_local_function_class(node: &Node, source: &str) -> Option<String> {
    let mut current = Some(*node);
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
        let tree = parse_source(src).unwrap();
        let mut table = RouteTable::new();
        extract_minimal_api_routes(tree.root_node(), src, &mut table);
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
        let tree = parse_source(src).unwrap();
        let mut table = RouteTable::new();
        extract_minimal_api_routes(tree.root_node(), src, &mut table);
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
        let tree = parse_source(src).unwrap();
        let mut table = RouteTable::new();
        extract_minimal_api_routes(tree.root_node(), src, &mut table);
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
        extract_minimal_api_routes(tree.root_node(), src, &mut table);
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
        let tree = parse_source(src).unwrap();
        let mut table = RouteTable::new();
        extract_minimal_api_routes(tree.root_node(), src, &mut table);
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
}
