#!/usr/bin/env python3
"""Deterministic L12 catalog inventory; runtime parity tests check every row."""
import argparse
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
ALIASES = {
    "kotlin": ["kt", "kts"], "cpp": ["c++", "cc", "cxx", "hpp", "h++", "hh", "hxx", "c+", "cplus"],
    "go": ["golang"], "javascript": ["js"], "typescript": ["ts"], "tsx": ["jsx"], "html": ["htm"],
    "python": ["py"], "php": ["php3", "php4", "php5"], "ruby": ["rb"], "bash": ["sh", "shellscript"],
    "markdown": ["md"], "perl": ["pl"], "yaml": ["yml"], "pascal": ["pas", "delphi"], "ocaml": ["ml"],
    "ocaml-interface": ["mli"], "csharp": ["c#", "cs", "c-sharp"],
}
CANONICAL = {alias: language for language, aliases in ALIASES.items() for alias in aliases}
def canonical(language):
    return CANONICAL.get(language, language)

def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args()
    manifest = json.loads((ROOT / "resources/grammars/manifest.json").read_text())
    grammars = {entry["language"]: entry for entry in manifest["grammars"]}
    capabilities = {entry["language"]: entry["capability"] for entry in json.loads((ROOT / "docs/parity/L10-capabilities.json").read_text())["rows"]}
    mime_rows = json.loads((ROOT / "resources/mime-types.json").read_text())
    # Later rows override earlier mappings, just as MimeDetector does.
    mime = {"." + entry["extension"].lstrip(".").lower(): entry for entry in mime_rows}
    keyword_names = {line.split("=", 1)[0].strip() for line in (ROOT / "resources/keyword-patterns.txt").read_text().splitlines() if "=" in line and not line.lstrip().startswith("#")}
    catalog = json.loads((ROOT / "resources/codeintel/definitions.json").read_text())["languages"]
    languages = sorted(set(grammars) | {canonical(entry["language"]) for entry in mime.values()} | {canonical(name) for name in keyword_names} | {canonical(name) for entry in catalog for name in [entry["language"], *entry.get("aliases", [])]})
    rows = []
    for language in languages:
        keyword = ("shellscript" if language == "bash" else language) in keyword_names or language in {"html", "html-derivative", "xml", "xsl", "jsx", "tsx", "vue-html", "css", "scss", "less"}
        mappings = []
        for extension, entry in sorted(mime.items()):
            if canonical(entry["language"]) != language:
                continue
            routed = mime.get("." + extension.rsplit(".", 1)[-1], entry)
            mappings.append({"extension": extension, "mime": entry["mime"], "declaredLanguage": entry["language"], "category": entry["category"], "runtimeMime": routed["mime"], "runtimeLanguage": routed["language"]})
        indexable = not mappings or any(entry["category"] == "TEXT" for entry in mappings)
        row = {
            "language": language,
            "aliases": ALIASES.get(language, []),
            "mime": mappings,
            "indexableText": indexable,
            "keywordSupport": keyword,
            "grammar": grammars.get(language),
            "lexical": "native-tree-and-keywords" if language in grammars and keyword else "native-tree" if language in grammars else "keywords" if keyword else "plain",
            "semantic": capabilities.get(language, "heuristic-regex-only" if indexable else "viewer-only-no-index"),
            "heuristicCatalogs": [entry["language"] for entry in catalog if language in {canonical(name) for name in [entry["language"], *entry.get("aliases", [])]}],
            "qualification": "live-fresh-sqlite-reload" if language in grammars else "catalog-routing-and-explicit-fallback" if indexable else "mime-viewer-routing",
            "compilerGuarantees": False,
        }
        rows.append(row)
    output = {"sourceBaseline": "7d3e5953ec2de75f2b5e9c44fb3e65549d39ec68", "runtime": manifest["runtime"], "nativeVariants": len(grammars), "mimeRows": len(mime_rows), "effectiveExtensions": len(mime), "keywordPatterns": len(keyword_names), "configuredHeuristicLanguages": len(catalog), "rows": rows}
    path = ROOT / "docs/parity/language-matrix.json"
    expected = json.dumps(output, indent=2, ensure_ascii=False) + "\n"
    if args.check:
        assert path.read_text() == expected, "language matrix is stale; run python3 scripts/language-matrix.py"
    else:
        path.write_text(expected)
    print(f"Language matrix: {len(rows)} rows / {len(grammars)} native variants / {len(mime)} effective extensions")

if __name__ == "__main__":
    main()
