use tree_sitter::Parser;
fn main() {
    let mut parser = Parser::new();
    parser.set_language(&tree_sitter_c_sharp::LANGUAGE.into()).unwrap();
    let sources = vec![
        "class C { Foo M() { return new Foo(); } }",
        "class C { int M() { var x = GetValue(); return x; } object GetValue() { return null; } }",
        "class C { C() { } }",
    ];
    for src in &sources {
        println!("\n===== {}", src);
        let tree = parser.parse(src, None).unwrap();
        let root = tree.root_node();
        for i in 0..root.child_count() {
            let c = root.child(i).unwrap();
            if c.kind() == "class_declaration" {
                for j in 0..c.child_count() {
                    let body = c.child(j).unwrap();
                    if body.kind() == "declaration_list" {
                        for k in 0..body.child_count() {
                            let method = body.child(k).unwrap();
                            if method.kind() == "method_declaration" || method.kind() == "constructor_declaration" {
                                println!("  method: {}", method.utf8_text(src.as_bytes()).unwrap_or(""));
                                for m in 0..method.child_count() {
                                    let child = method.child(m).unwrap();
                                    let fname = method.field_name_for_child(m as u32);
                                    println!("    child[{}] field={:?} kind={} text={}", m, fname, child.kind(),
                                        child.utf8_text(src.as_bytes()).unwrap_or(""));
                                }
                                // Find return statements
                                println!("    --- searching returns ---");
                                find_returns(method, src, 4);
                            }
                        }
                    }
                }
            }
        }
    }
}
fn find_returns(node: tree_sitter::Node, source: &str, indent: usize) {
    if node.kind() == "return_statement" {
        let text = node.utf8_text(source.as_bytes()).unwrap_or("");
        println!("{:indent$}RETURN: {}", "", text, indent = indent);
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            println!("{:indent$}  kind={} text={}", "", child.kind(),
                child.utf8_text(source.as_bytes()).unwrap_or(""), indent = indent + 2);
        }
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        find_returns(child, source, indent + 2);
    }
}
