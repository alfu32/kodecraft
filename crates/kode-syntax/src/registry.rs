//! Pinned upstream language functions; no IDE-owned unsafe FFI or dynamic loading.
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, sync::Arc};
use tree_sitter::Language;
#[derive(Debug, Clone)]
pub struct Grammar {
    pub id: &'static str,
    pub language: Language,
    pub node_types: &'static str,
    pub package: &'static str,
    pub version: &'static str,
    pub repository: &'static str,
}
#[derive(Debug, Clone, Serialize)]
pub struct GrammarMetadata {
    pub language: &'static str,
    pub package: &'static str,
    pub version: &'static str,
    pub repository: &'static str,
    pub license: &'static str,
    pub abi: usize,
    pub node_types_sha256: String,
}
impl Grammar {
    pub fn metadata(&self) -> GrammarMetadata {
        GrammarMetadata {
            language: self.id,
            package: self.package,
            version: self.version,
            repository: self.repository,
            license: "MIT",
            abi: self.language.abi_version(),
            node_types_sha256: format!("{:x}", Sha256::digest(self.node_types.as_bytes())),
        }
    }
}
#[derive(Debug, Clone, Default)]
pub struct GrammarRegistry {
    entries: BTreeMap<&'static str, Arc<Grammar>>,
}
impl GrammarRegistry {
    pub fn empty() -> Self {
        Self::default()
    }
    pub fn compiled() -> Self {
        let mut registry = Self::empty();
        macro_rules! add {
            ($id:literal,$binding:ident,$language:ident,$nodes:ident,$package:literal,$version:literal,$repository:literal) => {
                registry.entries.insert(
                    $id,
                    Arc::new(Grammar {
                        id: $id,
                        language: $binding::$language.into(),
                        node_types: $binding::$nodes,
                        package: $package,
                        version: $version,
                        repository: $repository,
                    }),
                );
            };
        }
        add!(
            "kotlin",
            tree_sitter_kotlin_ng,
            LANGUAGE,
            NODE_TYPES,
            "tree-sitter-kotlin-ng",
            "1.1.0",
            "https://github.com/tree-sitter-grammars/tree-sitter-kotlin"
        );
        add!(
            "c",
            tree_sitter_c,
            LANGUAGE,
            NODE_TYPES,
            "tree-sitter-c",
            "0.24.2",
            "https://github.com/tree-sitter/tree-sitter-c"
        );
        add!(
            "cpp",
            tree_sitter_cpp,
            LANGUAGE,
            NODE_TYPES,
            "tree-sitter-cpp",
            "0.23.4",
            "https://github.com/tree-sitter/tree-sitter-cpp"
        );
        add!(
            "javascript",
            tree_sitter_javascript,
            LANGUAGE,
            NODE_TYPES,
            "tree-sitter-javascript",
            "0.25.0",
            "https://github.com/tree-sitter/tree-sitter-javascript"
        );
        add!(
            "typescript",
            tree_sitter_typescript,
            LANGUAGE_TYPESCRIPT,
            TYPESCRIPT_NODE_TYPES,
            "tree-sitter-typescript",
            "0.23.2",
            "https://github.com/tree-sitter/tree-sitter-typescript"
        );
        add!(
            "tsx",
            tree_sitter_typescript,
            LANGUAGE_TSX,
            TSX_NODE_TYPES,
            "tree-sitter-typescript",
            "0.23.2",
            "https://github.com/tree-sitter/tree-sitter-typescript"
        );
        add!(
            "python",
            tree_sitter_python,
            LANGUAGE,
            NODE_TYPES,
            "tree-sitter-python",
            "0.25.0",
            "https://github.com/tree-sitter/tree-sitter-python"
        );
        add!(
            "json",
            tree_sitter_json,
            LANGUAGE,
            NODE_TYPES,
            "tree-sitter-json",
            "0.24.8",
            "https://github.com/tree-sitter/tree-sitter-json"
        );
        add!(
            "php",
            tree_sitter_php,
            LANGUAGE_PHP,
            PHP_NODE_TYPES,
            "tree-sitter-php",
            "0.25.1",
            "https://github.com/tree-sitter/tree-sitter-php"
        );
        add!(
            "css",
            tree_sitter_css,
            LANGUAGE,
            NODE_TYPES,
            "tree-sitter-css",
            "0.25.0",
            "https://github.com/tree-sitter/tree-sitter-css"
        );
        add!(
            "html",
            tree_sitter_html,
            LANGUAGE,
            NODE_TYPES,
            "tree-sitter-html",
            "0.23.2",
            "https://github.com/tree-sitter/tree-sitter-html"
        );
        add!(
            "go",
            tree_sitter_go,
            LANGUAGE,
            NODE_TYPES,
            "tree-sitter-go",
            "0.25.0",
            "https://github.com/tree-sitter/tree-sitter-go"
        );
        add!(
            "ruby",
            tree_sitter_ruby,
            LANGUAGE,
            NODE_TYPES,
            "tree-sitter-ruby",
            "0.23.1",
            "https://github.com/tree-sitter/tree-sitter-ruby"
        );
        add!(
            "bash",
            tree_sitter_bash,
            LANGUAGE,
            NODE_TYPES,
            "tree-sitter-bash",
            "0.25.1",
            "https://github.com/tree-sitter/tree-sitter-bash"
        );
        add!(
            "swift",
            tree_sitter_swift,
            LANGUAGE,
            NODE_TYPES,
            "tree-sitter-swift",
            "0.7.4",
            "https://github.com/alex-pinkus/tree-sitter-swift"
        );
        add!(
            "sql",
            tree_sitter_sequel,
            LANGUAGE,
            NODE_TYPES,
            "tree-sitter-sequel",
            "0.3.11",
            "https://github.com/derekstride/tree-sitter-sql"
        );
        add!(
            "zig",
            tree_sitter_zig,
            LANGUAGE,
            NODE_TYPES,
            "tree-sitter-zig",
            "1.1.2",
            "https://github.com/tree-sitter-grammars/tree-sitter-zig"
        );
        add!(
            "markdown",
            tree_sitter_md,
            LANGUAGE,
            NODE_TYPES_BLOCK,
            "tree-sitter-md",
            "0.5.3",
            "https://github.com/tree-sitter-grammars/tree-sitter-markdown"
        );
        add!(
            "markdown-inline",
            tree_sitter_md,
            INLINE_LANGUAGE,
            NODE_TYPES_INLINE,
            "tree-sitter-md",
            "0.5.3",
            "https://github.com/tree-sitter-grammars/tree-sitter-markdown"
        );
        add!(
            "lua",
            tree_sitter_lua,
            LANGUAGE,
            NODE_TYPES,
            "tree-sitter-lua",
            "0.5.0",
            "https://github.com/tree-sitter-grammars/tree-sitter-lua"
        );
        add!(
            "svelte",
            tree_sitter_svelte_ng,
            LANGUAGE,
            NODE_TYPES,
            "tree-sitter-svelte-ng",
            "1.0.2",
            "https://github.com/tree-sitter-grammars/tree-sitter-svelte"
        );
        add!(
            "perl",
            tree_sitter_perl,
            LANGUAGE,
            NODE_TYPES,
            "tree-sitter-perl",
            "1.1.2",
            "https://github.com/ganezdragon/tree-sitter-perl"
        );
        add!(
            "d",
            tree_sitter_d,
            LANGUAGE,
            NODE_TYPES,
            "tree-sitter-d",
            "0.8.2",
            "https://github.com/gdamore/tree-sitter-d"
        );
        add!(
            "yaml",
            tree_sitter_yaml,
            LANGUAGE,
            NODE_TYPES,
            "tree-sitter-yaml",
            "0.7.2",
            "https://github.com/tree-sitter-grammars/tree-sitter-yaml"
        );
        add!(
            "pascal",
            tree_sitter_pascal,
            LANGUAGE,
            NODE_TYPES,
            "tree-sitter-pascal",
            "0.10.2",
            "https://github.com/Isopod/tree-sitter-pascal"
        );
        add!(
            "ocaml",
            tree_sitter_ocaml,
            LANGUAGE_OCAML,
            OCAML_NODE_TYPES,
            "tree-sitter-ocaml",
            "0.26.0",
            "https://github.com/tree-sitter/tree-sitter-ocaml"
        );
        add!(
            "ocaml-interface",
            tree_sitter_ocaml,
            LANGUAGE_OCAML_INTERFACE,
            INTERFACE_NODE_TYPES,
            "tree-sitter-ocaml",
            "0.26.0",
            "https://github.com/tree-sitter/tree-sitter-ocaml"
        );
        add!(
            "csharp",
            tree_sitter_c_sharp,
            LANGUAGE,
            NODE_TYPES,
            "tree-sitter-c-sharp",
            "0.23.5",
            "https://github.com/tree-sitter/tree-sitter-c-sharp"
        );
        registry
    }
    pub fn get(&self, language: &str) -> Option<Arc<Grammar>> {
        self.entries
            .get(canonical_language(language).as_str())
            .cloned()
    }
    pub fn entries(&self) -> impl Iterator<Item = &Arc<Grammar>> {
        self.entries.values()
    }
    pub fn metadata(&self) -> Vec<GrammarMetadata> {
        self.entries().map(|entry| entry.metadata()).collect()
    }
}
pub fn canonical_language(language: &str) -> String {
    let key = language.trim().trim_start_matches('.').to_ascii_lowercase();
    match key.as_str() {
        "kt" | "kts" => "kotlin",
        "cc" | "cxx" | "hpp" | "h++" | "hh" | "hxx" | "c++" | "c+" | "cplus" => "cpp",
        "js" => "javascript",
        "ts" => "typescript",
        "jsx" => "tsx",
        "py" => "python",
        "rb" => "ruby",
        "sh" | "shellscript" => "bash",
        "golang" => "go",
        "htm" => "html",
        "php3" | "php4" | "php5" => "php",
        "md" => "markdown",
        "pl" => "perl",
        "yml" => "yaml",
        "pas" | "delphi" => "pascal",
        "ml" => "ocaml",
        "mli" => "ocaml-interface",
        "cs" | "c#" | "c-sharp" => "csharp",
        _ => &key,
    }
    .to_owned()
}
