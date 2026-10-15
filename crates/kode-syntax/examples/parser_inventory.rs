//! Reproducible parser ABI/node-field qualification; stdout is a JSON artifact.
use kode_core::{DocumentId, DocumentOrigin, WorkspaceGeneration, document::Document};
use kode_platform::jobs::CancellationToken;
use kode_syntax::{keywords::KeywordProvider, parser::ParserSession, registry::GrammarRegistry};
use serde_json::json;
fn sample(language: &str) -> &'static str {
    match language {
        "kotlin" => {
            "package demo\nclass Café(val name: String) {\n fun greet(): String {\n return name\n }\n}\n"
        }
        "c" => {
            "typedef struct Customer { int value; } Customer;\nint use(Customer *customer) { return customer->value; }\n"
        }
        "cpp" => {
            "namespace demo { class Customer { public: int value; int get() { return value; } }; }\n"
        }
        "javascript" => "export class Customer { greet() { return 'hello'; } }\n",
        "typescript" => {
            "export interface Customer { name: string; }\nexport function greet(value: Customer): string { return value.name; }\n"
        }
        "tsx" => "export function App() { return <Button className=\"primary\">hello</Button>; }\n",
        "python" => "class Customer:\n    def greet(self):\n        return 'hello'\n",
        "json" => "{\"name\": \"demo\", \"enabled\": true, \"count\": 2}\n",
        "php" => "<?php class Customer { public function greet() { return 'hello'; } }\n",
        "css" => ".myBox { color: red; padding: 2px; }\n",
        "html" => "<div class=\"primary\">hello</div>\n",
        "go" => "package demo\nfunc greet(name string) string { return name }\n",
        "ruby" => "class Customer\n  def greet\n    'hello'\n  end\nend\n",
        "bash" => "#!/bin/bash\ngreet() { echo hello; }\n",
        "swift" => "struct Customer { let name: String; func greet() -> String { return name } }\n",
        "sql" => {
            "CREATE TABLE customers (id INTEGER, name TEXT);\nSELECT name FROM customers WHERE id = 1;\n"
        }
        "zig" => "pub fn main() void { const value: u32 = 2; _ = value; }\n",
        "markdown" => "# Title\n\nHello **world**.\n",
        "markdown-inline" => "Hello **world** and `code`.",
        "lua" => "local function greet(name) return name end\n",
        "svelte" => "<script>let name = 'world';</script>\n<h1>Hello {name}</h1>\n",
        "perl" => "sub greet { return 'hello'; }\n",
        "d" => "module demo;\nint main() { return 0; }\n",
        "yaml" => "name: demo\ncount: 2\n",
        "pascal" => "program Demo;\nbegin\nend.\n",
        "ocaml" => "module M = struct let value = 2 end\n",
        "ocaml-interface" => "module M : sig val value : int end\n",
        "csharp" => {
            "namespace Demo { class Customer { public string Greet() { return \"hello\"; } } }\n"
        }
        _ => panic!("missing parser fixture for {language}"),
    }
}
fn main() {
    let registry = GrammarRegistry::compiled();
    let mut cases = vec![];
    for grammar in registry.entries() {
        let source = sample(grammar.id);
        let document = Document::new(
            DocumentId::fresh().unwrap(),
            DocumentOrigin::Virtual {
                owner: "qualification".into(),
                key: grammar.id.into(),
                label: grammar.id.into(),
            },
            Some(grammar.id.into()),
            source,
        );
        let parsed = ParserSession::new(grammar.clone())
            .unwrap()
            .parse(
                &document.snapshot(),
                WorkspaceGeneration(1),
                &CancellationToken::new(),
            )
            .unwrap();
        let mut nodes = vec![];
        let mut cursor = parsed.tree().walk();
        loop {
            let node = cursor.node();
            nodes.push(json!({"kind":node.kind(),"parent":node.parent().map(|parent|parent.kind()),"field":cursor.field_name(),"named":node.is_named(),"range":{"start":node.start_byte(),"end":node.end_byte()},"text":&source[node.byte_range()]}));
            if cursor.goto_first_child() {
                continue;
            }
            loop {
                if cursor.goto_next_sibling() {
                    break;
                }
                if !cursor.goto_parent() {
                    break;
                }
            }
            if cursor.node().id() == parsed.root_node().id() {
                break;
            }
        }
        cases.push(json!({"grammar":grammar.metadata(),"source":source,"root":parsed.root_node().kind(),"hasErrors":parsed.has_errors(),"diagnostics":parsed.diagnostics(),"nodes":nodes}));
    }
    let keywords = KeywordProvider::compiled().unwrap();
    let value = json!({"runtime":"0.26.13","bindings":cases,"keywords":{"languages":keywords.languages(),"diagnostics":keywords.diagnostics}});
    println!("{}", serde_json::to_string_pretty(&value).unwrap());
}
