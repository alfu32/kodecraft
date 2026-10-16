use kode_core::DomainError;
use kode_platform::jobs::CancellationToken;
use kode_rest::{
    import::{Format, Import, Kind, export_file, export_target_identity, parse, read_file},
    model::{Environment, EnvironmentValue, NodePath, Workspace},
    resolve::RequestResolver,
};
use serde_json::{Value, json};
use std::{fs, path::Path};
fn fixture(name: &str) -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/rest/import")
        .join(name)
}
fn imported(name: &str) -> Import {
    read_file(&fixture(name), &CancellationToken::new()).unwrap()
}
fn requests(import: &Import) -> Vec<&Value> {
    import.collection["item"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|folder| folder["item"].as_array().unwrap())
        .collect()
}
fn without_ids(mut value: Value) -> Value {
    if let Some(items) = value.get_mut("item").and_then(Value::as_array_mut) {
        for item in items {
            *item = without_ids(item.clone());
        }
    }
    if value.get("request").is_some() {
        value.as_object_mut().unwrap().remove("id");
    }
    value
}
#[test]
fn json_yaml_pairs_preserve_big_numbers_and_convert_identically() {
    for name in ["openapi3", "swagger2"] {
        let json = imported(&format!("{name}.json"));
        let yaml = imported(&format!("{name}.yaml"));
        assert_eq!(json.kind, Kind::OpenApi);
        assert_eq!(without_ids(json.collection), without_ids(yaml.collection));
        assert_eq!(json.notices, yaml.notices);
    }
}
#[test]
fn original_json_model_case_imports_into_requested_folder_and_keeps_selection() {
    let tmp = tempfile::tempdir().unwrap();
    let source = tmp.path().join("openapi.json");
    fs::write(&source, r#"{"openapi":"3.0.0","info":{"title":"Pets"},"servers":[{"url":"https://api.example.test"}],"paths":{"/pets/{id}":{"get":{"tags":["Pets"],"summary":"Get pet","parameters":[{"name":"id","in":"path","required":true,"schema":{"type":"string"}}]}}}}"#).unwrap();
    let mut model = Workspace::default();
    let folder = model.add_folder(&NodePath::default(), "Imported").unwrap();
    model.select(&folder);
    let imported = read_file(&source, &CancellationToken::new()).unwrap();
    assert_eq!(
        imported.apply(&mut model, &folder).unwrap(),
        vec![folder.child(0)]
    );
    let group = model.node(&folder.child(0)).unwrap();
    assert_eq!(group["item"][0]["request"]["method"], "GET");
    assert!(
        group["item"][0]["request"]["url"]["raw"]
            .as_str()
            .unwrap()
            .contains(":id")
    );
    assert_eq!(model.selected_path(), folder);
}
#[test]
fn original_minimal_yaml_model_case_and_default_group_title() {
    let tmp = tempfile::tempdir().unwrap();
    let source = tmp.path().join("openapi.yaml");
    fs::write(&source, "openapi: 3.0.0\ninfo:\n  title: YAML API\npaths:\n  /health:\n    get:\n      summary: Health\n").unwrap();
    let mut model = Workspace::default();
    assert_eq!(
        read_file(&source, &CancellationToken::new())
            .unwrap()
            .apply(&mut model, &NodePath::default())
            .unwrap()
            .len(),
        1
    );
    assert!(!model.collection()["item"].as_array().unwrap().is_empty());
    let imported = parse(
        "openapi: 3.0.0\ninfo: {title: Root API}\npaths: {'/': {get: {summary: Root}}}",
        Format::Yaml,
    )
    .unwrap();
    assert_eq!(imported.collection["item"][0]["name"], "Root API");
}
#[test]
fn parameters_overrides_scopes_local_refs_servers_media_and_auth_reach_real_resolver() {
    let imported = imported("openapi3.json");
    let collection = &imported.collection;
    let request = &requests(&imported)[0]["request"];
    assert_eq!(
        request["url"]["query"],
        json!([{"key":"q","value":"new value"}])
    );
    assert_eq!(
        request["url"]["variable"],
        json!([{"key":"id","value":"a/b"}])
    );
    let env = Environment {
        values: vec![EnvironmentValue {
            key: "bearerToken".into(),
            value: json!("synthetic"),
            ..Default::default()
        }],
        ..Default::default()
    };
    let resolved =
        RequestResolver::new(collection, NodePath(vec![0, 0]), Path::new("."), Some(&env))
            .unwrap()
            .plan()
            .unwrap();
    assert_eq!(
        resolved.url,
        "https://api.example.test/v1/pets/a%2Fb?q=new+value"
    );
    assert!(
        resolved
            .headers
            .iter()
            .any(|header| header.name == "Authorization" && header.value == "Bearer synthetic")
    );
    assert!(
        resolved
            .headers
            .iter()
            .any(|header| header.name == "X-Trace" && header.value == "trace")
    );
    let requests = requests(&imported);
    assert_eq!(requests[1]["request"]["auth"]["type"], "noauth");
    assert_eq!(
        requests[1]["request"]["body"]["raw"],
        r#"{"name":"Miso","age":3}"#
    );
    assert_eq!(
        requests[1]["request"]["body"]["options"]["raw"]["language"],
        "json"
    );
    assert_eq!(
        requests[2]["request"]["body"]["raw"],
        r#"{"name":"Miso","age":3}"#
    );
    assert_eq!(
        requests[3]["request"]["url"]["raw"],
        "https://write.example.test/text"
    );
    assert_eq!(requests[3]["request"]["body"]["raw"], r#""string body""#);
    assert_eq!(requests[4]["request"]["body"]["raw"], "<pet/>");
    assert_eq!(
        requests[4]["request"]["body"]["options"]["raw"]["language"],
        "xml"
    );
}
#[test]
fn swagger_base_path_body_schema_api_key_and_form() {
    let imported = imported("swagger2.json");
    let env = Environment {
        values: vec![EnvironmentValue {
            key: "apiKey".into(),
            value: json!("sample"),
            ..Default::default()
        }],
        ..Default::default()
    };
    let prepared = RequestResolver::new(
        &imported.collection,
        NodePath(vec![0, 0]),
        Path::new("."),
        Some(&env),
    )
    .unwrap()
    .plan()
    .unwrap()
    .materialize(1024, &CancellationToken::new())
    .unwrap();
    assert_eq!(
        prepared.request.url,
        "http://legacy.example.test:8080/api/pets?access_key=sample"
    );
    assert_eq!(&*prepared.body, br#"{"name":"Miso"}"#);
    let form = RequestResolver::new(
        &imported.collection,
        NodePath(vec![1, 0]),
        Path::new("."),
        None,
    )
    .unwrap()
    .plan()
    .unwrap()
    .materialize(1024, &CancellationToken::new())
    .unwrap();
    assert_eq!(&*form.body, b"name=Miso");
}
#[test]
fn postman_root_import_is_lossless_and_nested_import_retains_inheritance_extensions() {
    let source: Value = serde_json::from_str(include_str!(
        "../../../fixtures/rest/model/preserved.postman.json"
    ))
    .unwrap();
    let mut source = source;
    source["variable"] = json!([{"key":"host","value":"https://example.test"}]);
    source["auth"] = json!({"type":"bearer","bearer":[{"key":"token","value":"sample"}]});
    source["x-kode-headers"] = json!([{"key":"X-Collection","value":"retained"}]);
    source["item"] = json!([{"id":"keep-id","name":"Call","request":{"method":"GET","url":"{{host}}"},"response":[{"body":"saved example"}]}]);
    let imported = Import::from_document(source.clone()).unwrap();
    let mut model = Workspace::default();
    imported.apply(&mut model, &NodePath::default()).unwrap();
    assert_eq!(model.collection(), &source);
    let folder = model
        .add_folder(&NodePath::default(), "destination")
        .unwrap();
    let paths = imported.apply(&mut model, &folder).unwrap();
    let nested = model.node(&paths[0]).unwrap();
    for (key, value) in source.as_object().unwrap() {
        assert_eq!(&nested[key], value, "{key}");
    }
    let resolved =
        RequestResolver::new(model.collection(), paths[0].child(0), Path::new("."), None)
            .unwrap()
            .plan()
            .unwrap();
    assert!(
        resolved
            .headers
            .iter()
            .any(|h| h.name == "X-Collection" && h.value == "retained")
    );
    assert!(
        resolved
            .headers
            .iter()
            .any(|h| h.name == "Authorization" && h.value == "Bearer sample")
    );
}
#[test]
fn invalid_destinations_and_shapes_are_atomic_with_actionable_paths() {
    let mut model = Workspace::default();
    let request = model.add_request(&NodePath::default(), "existing").unwrap();
    let snapshot = model.export_collection();
    let revision = model.revision();
    for path in [request, NodePath(vec![99])] {
        assert!(
            imported("openapi3.json")
                .apply(&mut model, &path)
                .unwrap_err()
                .to_string()
                .contains("folder")
        );
        assert_eq!(model.collection(), &*snapshot);
        assert_eq!(model.revision(), revision);
    }
    for (text, expected) in [
        (r#"{"openapi":"3.0.0"}"#, "/paths"),
        (
            r#"{"openapi":"3.0.0","paths":{"/x":{"get":[]}}}"#,
            "/paths/~1x/get",
        ),
        (r#"{"item":[]}"#, "info"),
        (r#"{"info":{},"item":[],"openapi":"1.0"}"#, "versions"),
    ] {
        assert!(
            parse(text, Format::Json)
                .unwrap_err()
                .to_string()
                .contains(expected)
        );
    }
}
#[test]
fn unresolved_refs_cycles_unsupported_security_and_schema_are_explicit_and_not_sendable() {
    let document = json!({"openapi":"3.1.0","info":{"title":"Unsupported"},"components":{"schemas":{"Loop":{"$ref":"#/components/schemas/Loop"}},"securitySchemes":{"oauth":{"type":"oauth2","flows":{}}}},"webhooks":{"incoming":{}},"paths":{
        "/remote":{"get":{"parameters":[{"$ref":"https://example.invalid/never-fetch"}]}},
        "/cycle":{"post":{"requestBody":{"content":{"application/json":{"schema":{"$ref":"#/components/schemas/Loop"}}}}}},
        "/oauth":{"get":{"security":[{"oauth":[]}]}},
        "/composition":{"post":{"requestBody":{"content":{"application/json":{"schema":{"oneOf":[{"type":"string"},{"type":"number"}]}}}}}}
    }});
    let imported = Import::from_document(document.clone()).unwrap();
    assert_eq!(imported.collection["x-kode-openapi"]["document"], document);
    for code in [
        "external-reference",
        "reference-cycle",
        "unsupported-security-scheme",
        "schema-construct",
        "unsupported-schema-sample",
        "non-request-operation",
    ] {
        assert!(
            imported.notices.iter().any(|notice| notice.code == code),
            "missing {code}"
        );
    }
    for index in 0..4 {
        assert_eq!(
            imported.collection["item"][index]["item"][0]["x-kode-import-incomplete"],
            true
        );
        let error = RequestResolver::new(
            &imported.collection,
            NodePath(vec![index, 0]),
            Path::new("."),
            None,
        )
        .unwrap()
        .plan()
        .unwrap_err();
        assert!(error.to_string().contains("incomplete"));
    }
}
#[test]
fn internal_reference_escapes_auth_alternatives_basic_and_header_api_key() {
    let document = json!({"swagger":"2.0","securityDefinitions":{"basic":{"type":"basic"},"key":{"type":"apiKey","in":"header","name":"X-Key"}},"parameters":{"a/b~c":{"name":"q","in":"query","example":42}},"paths":{
        "/basic":{"get":{"security":[{"basic":[]},{"key":[]}],"parameters":[{"$ref":"#/parameters/a~1b~0c"}]}},
        "/key":{"get":{"security":[{"key":[]}]}},
        "/combined":{"get":{"security":[{"basic":[],"key":[]}]}}
    }});
    let imported = Import::from_document(document).unwrap();
    let requests = requests(&imported);
    assert_eq!(requests[0]["request"]["auth"]["type"], "basic");
    assert_eq!(requests[0]["request"]["url"]["query"][0]["value"], "42");
    assert_eq!(
        requests[1]["request"]["auth"]["apikey"][2]["value"],
        "header"
    );
    assert!(
        imported
            .notices
            .iter()
            .any(|n| n.code == "security-alternatives")
    );
    assert!(
        imported
            .notices
            .iter()
            .any(|n| n.code == "combined-security")
    );
}
#[test]
fn selected_media_and_server_alternatives_and_relative_servers_are_visible() {
    let document = json!({"openapi":"3.0.0","paths":{"/x":{"post":{"servers":[{"url":"/relative"},{"url":"https://example.test"}],"requestBody":{"content":{"text/plain":{"example":"first"},"application/json":{"example":"second"}}}}}}});
    let imported = Import::from_document(document).unwrap();
    let requests = requests(&imported);
    assert_eq!(requests[0]["request"]["body"]["raw"], "first");
    for code in ["server-choice", "relative-server", "media-choice"] {
        assert!(imported.notices.iter().any(|n| n.code == code));
    }
}
#[test]
fn duplicate_yaml_keys_tags_documents_alias_expansion_and_json_depth_fail_without_echoing_values() {
    for text in [
        "openapi: 3.0.0\nopenapi: private-secret\npaths: {}",
        "openapi: !include /tmp/private-secret\npaths: {}",
        "openapi: 3.0.0\npaths: {}\n---\nprivate-secret",
        "openapi: 3.0.0\npaths: &a [*a]\nprivate-secret: ok",
        "openapi: 3.0.0\npaths: [invalid\nprivate-secret",
    ] {
        let error = parse(text, Format::Yaml).unwrap_err().to_string();
        assert!(error.contains("YAML"), "{error}");
        assert!(!error.contains("private-secret"));
    }
    let too_deep = format!("{}0{}", "[".repeat(140), "]".repeat(140));
    assert!(parse(&too_deep, Format::Json).is_err());
    assert!(parse(&" ".repeat(16 * 1024 * 1024 + 1), Format::Json).is_err());
}
#[test]
fn file_errors_cancel_bom_invalid_utf8_and_guarded_export_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("collection.json");
    let cancel = CancellationToken::new();
    assert!(matches!(
        read_file(&path, &cancel),
        Err(DomainError::Io { .. })
    ));
    fs::write(&path, b"\xff").unwrap();
    assert!(
        read_file(&path, &cancel)
            .unwrap_err()
            .to_string()
            .contains("UTF-8")
    );
    fs::write(&path, r#"{"info":private-secret}"#).unwrap();
    let error = read_file(&path, &cancel).unwrap_err().to_string();
    assert!(error.contains("collection.json") && error.contains("line"));
    assert!(!error.contains("private-secret"));
    let collection = json!({"info":{"name":"Preserved"},"item":[{"id":"a","request":{"url":"https://example.invalid"},"response":[{"body":"saved example"}]}],"x-kode":42});
    let imported = parse(&format!("\u{feff}{collection}"), Format::Auto).unwrap();
    fs::remove_file(&path).unwrap();
    let identity = export_file(&path, &imported.collection, None, &cancel).unwrap();
    assert_eq!(read_file(&path, &cancel).unwrap().collection, collection);
    assert!(matches!(
        export_file(&path, &collection, None, &cancel),
        Err(DomainError::SaveConflict(_))
    ));
    let mut updated = collection.clone();
    updated["info"]["name"] = json!("Updated");
    let identity = export_file(&path, &updated, Some(&identity), &cancel).unwrap();
    fs::write(&path, "external edit").unwrap();
    assert!(matches!(
        export_file(&path, &collection, Some(&identity), &cancel),
        Err(DomainError::SaveConflict(_))
    ));
    assert_eq!(fs::read_to_string(&path).unwrap(), "external edit");
    cancel.cancel();
    assert!(matches!(
        read_file(&path, &cancel),
        Err(DomainError::Cancelled)
    ));
    assert!(matches!(
        export_file(
            &dir.path().join("cancelled.json"),
            &collection,
            None,
            &cancel
        ),
        Err(DomainError::Cancelled)
    ));
    assert!(!dir.path().join("cancelled.json").exists());
}

#[test]
fn yaml_numeric_spans_preserve_decimal_precision_aliases_and_quoted_types() {
    let text = "openapi: 3.1.0\npaths: {}\nx:\n  exact: &n 123456789012345678901234567890\n  alias: *n\n  decimal: 1.2345678901234567890123456789\n  positive: +123456789012345678901234567890\n  hex: 0xFF\n  quoted: '123456789012345678901234567890'\n  bool: true\n";
    let imported = parse(text, Format::Yaml).unwrap();
    let source = &imported.collection["x-kode-openapi"]["document"]["x"];
    assert_eq!(
        source["exact"].to_string(),
        "123456789012345678901234567890"
    );
    assert_eq!(source["alias"], source["exact"]);
    assert_eq!(source["positive"], source["exact"]);
    assert_eq!(
        source["decimal"].to_string(),
        "1.2345678901234567890123456789"
    );
    assert_eq!(source["hex"], 255);
    assert!(source["quoted"].is_string());
    assert_eq!(source["bool"], true);
}
#[test]
fn nested_name_extensions_are_retained_and_unsecured_operations_override_parent_auth() {
    let mut model = Workspace::default();
    let root = NodePath::default();
    model
        .set_node_field(
            &root,
            "auth",
            Some(json!({"type":"bearer","bearer":[{"key":"token","value":"must-not-leak"}]})),
        )
        .unwrap();
    let destination = model.add_folder(&root, "destination").unwrap();
    let imported = Import::from_document(json!({"info":{"name":"Title"},"name":"Extension name","item":[],"x-kode-import-original-name":{"custom":1}})).unwrap();
    let paths = imported.apply(&mut model, &destination).unwrap();
    assert_eq!(model.node(&paths[0]).unwrap(), &imported.collection);
    let openapi =
        Import::from_document(json!({"openapi":"3.0.0","paths":{"/health":{"get":{}}}})).unwrap();
    let paths = openapi.apply(&mut model, &destination).unwrap();
    let resolved =
        RequestResolver::new(model.collection(), paths[0].child(0), Path::new("."), None)
            .unwrap()
            .plan()
            .unwrap();
    assert!(
        resolved
            .headers
            .iter()
            .all(|header| header.name != "Authorization")
    );
}
#[test]
fn destination_depth_limit_rejects_atomically_and_exports_remain_readable() {
    let mut source = json!({"leaf":true});
    for _ in 0..93 {
        source = json!({"nested":source});
    }
    let imported = Import::from_document(json!({"info":{},"item":[],"x":source})).unwrap();
    let mut model = Workspace::default();
    let outer = model.add_folder(&NodePath::default(), "outer").unwrap();
    let inner = model.add_folder(&outer, "inner").unwrap();
    let before = model.export_collection();
    assert!(imported.apply(&mut model, &inner).is_err());
    assert_eq!(model.collection(), &*before);
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("new/nested/export.json");
    let cancel = CancellationToken::new();
    export_file(&path, &imported.collection, None, &cancel).unwrap();
    assert_eq!(
        read_file(&path, &cancel).unwrap().collection,
        imported.collection
    );
}
#[test]
fn arrays_query_serialization_and_path_server_override_are_explicit() {
    let imported = Import::from_document(json!({"openapi":"3.0.0","servers":[{"url":"https://root.example.test"}],"paths":{"/x":{"servers":[{"url":"https://path.example.test"}],"get":{"parameters":[{"name":"q","in":"query","example":["a b","c"]},{"name":"X-List","in":"header","example":["a","b"]}]}}}})).unwrap();
    let resolved = RequestResolver::new(
        &imported.collection,
        NodePath(vec![0, 0]),
        Path::new("."),
        None,
    )
    .unwrap()
    .plan()
    .unwrap();
    assert_eq!(resolved.url, "https://path.example.test/x?q=a+b&q=c");
    assert!(
        resolved
            .headers
            .iter()
            .any(|header| header.name == "X-List" && header.value == "a,b")
    );
    let unsupported = Import::from_document(json!({"openapi":"3.0.0","paths":{"/x":{"get":{"parameters":[{"name":"filter","in":"query","style":"deepObject","example":{"q":1}}]}}}})).unwrap();
    assert!(
        unsupported
            .notices
            .iter()
            .any(|notice| notice.code == "parameter-serialization")
    );
    assert_eq!(requests(&unsupported)[0]["x-kode-import-incomplete"], true);
}

#[test]
fn vendor_path_extensions_and_ref_sibling_constraints_are_retained_without_false_conversion() {
    let document = json!({"openapi":"3.1.0","components":{"schemas":{"Base":{"type":"object"}}},"paths":{"x-note":{"data":1},"/x":{"post":{"summary":null,"operationId":"FallbackName","requestBody":{"content":{"application/json":{"schema":{"$ref":"#/components/schemas/Base","properties":{"mustKeep":{"const":7}}}}}}}}}});
    let imported = Import::from_document(document.clone()).unwrap();
    assert_eq!(requests(&imported).len(), 1);
    assert_eq!(requests(&imported)[0]["name"], "FallbackName");
    assert_eq!(requests(&imported)[0]["x-kode-import-incomplete"], true);
    assert!(
        imported
            .notices
            .iter()
            .any(|notice| notice.code == "reference-siblings")
    );
    assert_eq!(imported.collection["x-kode-openapi"]["document"], document);
}

#[test]
fn explicitly_selected_export_target_is_bounded_regular_cancelable_and_still_guarded() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("chosen.json");
    let cancel = CancellationToken::new();
    let collection = json!({"info":{},"item":[]});
    assert!(export_target_identity(&path, &cancel).unwrap().is_none());
    fs::write(&path, "previous contents").unwrap();
    let identity = export_target_identity(&path, &cancel).unwrap().unwrap();
    export_file(&path, &collection, Some(&identity), &cancel).unwrap();
    assert_eq!(read_file(&path, &cancel).unwrap().collection, collection);
    assert!(matches!(
        export_file(&path, &collection, Some(&identity), &cancel),
        Err(DomainError::SaveConflict(_))
    ));
    assert!(export_target_identity(dir.path(), &cancel).is_err());
    let file = fs::File::create(&path).unwrap();
    file.set_len(16 * 1024 * 1024 + 1).unwrap();
    assert!(export_target_identity(&path, &cancel).is_err());
    cancel.cancel();
    assert!(matches!(
        export_target_identity(&path, &cancel),
        Err(DomainError::Cancelled)
    ));
}
