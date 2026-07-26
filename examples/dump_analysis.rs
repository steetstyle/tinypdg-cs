use tree_sitter::Parser;
fn main() {
    let mut parser = Parser::new();
    parser.set_language(&tree_sitter_c_sharp::LANGUAGE.into()).unwrap();
    let sources = vec![
        "class Foo { void Bar() { this.Baz(); } void Baz() { } }",
        "class Foo { void Bar() { var x = new Widget(); x.DoSomething(); } }",
        "class Foo { void Bar() { var s = Singleton.GetInstance(); } }",
        "class C { void M() { new Logger().Log(\"hi\"); } }",
        "class C { int M() { return this.Calc() + 5; } int Calc() { return 42; } }",
    ];
    for src in &sources {
        println!("\n===== {}", src);
        let tree = parser.parse(src, None).unwrap();
        let root = tree.root_node();
        find_method_calls(root, src, 0);
    }
}
fn find_method_calls(node: tree_sitter::Node, source: &str, depth: usize) {
    if depth > 8 { return; }
    let kind = node.kind();
    if kind == "invocation_expression" || kind == "object_creation_expression" {
        let text = node.utf8_text(source.as_bytes()).unwrap_or("");
        println!("  {:indent$}{} : {}", "", kind, text, indent = depth * 2);
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            let ckind = child.kind();
            let ctext = child.utf8_text(source.as_bytes()).unwrap_or("");
            for i in 0..child.child_count() {
                if let Some(fname) = child.field_name_for_child(i as u32) {
                    let gc = child.child(i).unwrap();
                    let gct = gc.utf8_text(source.as_bytes()).unwrap_or("");
                    println!("  {:indent$}  field {}: {} = {}", "", fname, gc.kind(), gct, indent = depth * 2 + 2);
                }
            }
            if !ckind.starts_with("field_") {
                println!("  {:indent$}  {}: {}", "", ckind, ctext, indent = depth * 2 + 2);
            }
        }
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        find_method_calls(child, source, depth + 1);
    }
}
