use kode_platform::jobs::CancellationToken;
use kode_rest::{
    model::{Environment, NodePath},
    resolve::{BodyPlan, Protocol, RequestResolver, ResolvedRequest},
};
use serde_json::{Value, json};
use std::path::Path;
fn collection(request: Value) -> Value {
    json!({"info":{"name":"API"},"item":[{"id":"fixture","name":"Call","request":request}]})
}
fn plan(collection: &Value, root: &Path) -> ResolvedRequest {
    RequestResolver::new(collection, NodePath(vec![0]), root, None)
        .unwrap()
        .plan()
        .unwrap()
}
fn body(request: &ResolvedRequest) -> Vec<u8> {
    request
        .body
        .materialize(1024 * 1024, &CancellationToken::new())
        .unwrap()
}
fn header<'a>(request: &'a ResolvedRequest, name: &str) -> Option<&'a str> {
    request
        .headers
        .iter()
        .find(|header| header.name.eq_ignore_ascii_case(name))
        .map(|header| header.value.as_str())
}
#[test]
fn original_nested_variables_path_variables_and_inherited_bearer_auth() {
    let collection = json!({"info":{"name":"API"},"variable":[{"key":"host","value":"prod.example.test"},{"key":"version","value":"v1"},{"key":"environment","value":"prod"},{"key":"token","value":"synthetic-secret"}],"auth":{"type":"bearer","bearer":[{"key":"token","value":"{{token}}"}]},"item":[{"name":"Folder","variable":[{"key":"host","value":"test.example.test"},{"key":"page","value":"2"}],"item":[{"name":"Call","variable":[{"key":"user","value":"42"}],"request":{"method":"GET","url":{"raw":"https://{{host}}/{{version}}/users/:user","variable":[{"key":"user","value":"{{user}}"}],"query":[{"key":"page","value":"{{page}}"}]},"header":[{"key":"X-Env","value":"{{environment}}"}]}}]}]});
    let request = RequestResolver::new(
        &collection,
        NodePath(vec![0, 0]),
        Path::new("/synthetic"),
        None,
    )
    .unwrap()
    .plan()
    .unwrap();
    assert_eq!(request.url, "https://test.example.test/v1/users/42?page=2");
    assert_eq!(header(&request, "X-Env"), Some("prod"));
    assert_eq!(
        header(&request, "Authorization"),
        Some("Bearer synthetic-secret")
    );
    assert!(!format!("{request:?}").contains("synthetic-secret"));
}
#[test]
fn original_cycles_unsupported_auth_and_disabled_variables_fail_before_execution() {
    let mut collection = collection(json!({"method":"GET","url":"https://example.test/{{a}}"}));
    collection["item"][0]["variable"] =
        json!([{"key":"a","value":"{{b}}"},{"key":"b","value":"{{a}}"}]);
    let resolver = RequestResolver::new(
        &collection,
        NodePath(vec![0]),
        Path::new("/synthetic"),
        None,
    )
    .unwrap();
    assert!(resolver.plan().unwrap_err().to_string().contains("cycle"));
    collection["item"][0]["request"]["url"] = json!("https://example.test/");
    collection["auth"] = json!({"type":"oauth2"});
    assert!(
        RequestResolver::new(
            &collection,
            NodePath(vec![0]),
            Path::new("/synthetic"),
            None
        )
        .unwrap()
        .plan()
        .is_err()
    );
    collection.as_object_mut().unwrap().remove("auth");
    collection["variable"] = json!([{"key":"disabled","value":"do-not-use","disabled":true}]);
    collection["item"][0]["request"]["url"] = json!("https://example.test/{{disabled}}");
    assert!(
        RequestResolver::new(
            &collection,
            NodePath(vec![0]),
            Path::new("/synthetic"),
            None
        )
        .unwrap()
        .plan()
        .unwrap_err()
        .to_string()
        .contains("{{disabled}}")
    );
}
#[test]
fn original_api_key_query_auth_inherited_headers_and_request_override() {
    let collection = json!({"info":{},"variable":[{"key":"apiKey","value":"synthetic-key"}],"auth":{"type":"apikey","apikey":[{"key":"key","value":"api_key"},{"key":"value","value":"{{apiKey}}"},{"key":"in","value":"query"}]},"x-kode-headers":[{"key":"X-Collection","value":"collection"},{"key":"X-Shared","value":"collection"}],"item":[{"name":"Folder","x-kode-headers":[{"key":"X-Folder","value":"folder"},{"key":"X-Shared","value":"folder"}],"item":[{"name":"Call","request":{"method":"GET","url":{"raw":"https://example.test/search","query":[{"key":"page","value":"1"}]},"header":[{"key":"X-Request","value":"request"},{"key":"x-shared","value":"request"}]}}]}]});
    let resolver = RequestResolver::new(
        &collection,
        NodePath(vec![0, 0]),
        Path::new("/synthetic"),
        None,
    )
    .unwrap();
    assert_eq!(resolver.effective_headers().unwrap().len(), 6);
    let request = resolver.plan().unwrap();
    assert_eq!(
        request.url,
        "https://example.test/search?page=1&api_key=synthetic-key"
    );
    assert_eq!(header(&request, "X-Collection"), Some("collection"));
    assert_eq!(header(&request, "X-Folder"), Some("folder"));
    assert_eq!(header(&request, "X-Shared"), Some("request"));
    assert!(header(&request, "api_key").is_none());
}
#[test]
fn original_environment_override_and_sse_normalization_and_explicit_noauth() {
    let mut collection =
        collection(json!({"method":"GET","url":"sse://{{host}}/events","auth":{"type":"noauth"}}));
    collection["variable"] = json!([{"key":"host","value":"collection.example.test"}]);
    collection["auth"] = json!({"type":"bearer","token":"synthetic-secret"});
    let environment: Environment = serde_json::from_value(
        json!({"name":"test","values":[{"key":"host","value":"environment.example.test"}]}),
    )
    .unwrap();
    let request = RequestResolver::new(
        &collection,
        NodePath(vec![0]),
        Path::new("/synthetic"),
        Some(&environment),
    )
    .unwrap()
    .plan()
    .unwrap();
    assert_eq!(request.url, "http://environment.example.test/events");
    assert_eq!(request.protocol, Protocol::Sse);
    assert_eq!(header(&request, "Accept"), Some("text/event-stream"));
    assert!(header(&request, "Authorization").is_none());
}
#[test]
fn raw_languages_and_explicit_content_type_overrides_preserve_utf8_body_bytes() {
    for (language, expected) in [
        ("json", "application/json"),
        ("xml", "application/xml"),
        ("html", "text/html"),
        ("javascript", "text/plain"),
    ] {
        let mut collection = collection(
            json!({"method":"post","url":"https://example.test","body":{"mode":"raw","raw":"界 {{value}}","options":{"raw":{"language":language}}}}),
        );
        collection["variable"] = json!([{"key":"value","value":"😀"}]);
        let request = plan(&collection, Path::new("/synthetic"));
        assert_eq!(request.method, "POST");
        assert_eq!(body(&request), "界 😀".as_bytes());
        assert_eq!(header(&request, "Content-Type"), Some(expected));
        collection["item"][0]["request"]["header"] =
            json!([{"key":"content-type","value":"application/custom"}]);
        assert_eq!(
            header(&plan(&collection, Path::new("/synthetic")), "Content-Type"),
            Some("application/custom")
        );
    }
}
#[test]
fn form_urlencoded_duplicate_fields_disabled_rows_and_form_encoding_are_retained() {
    let collection = collection(
        json!({"method":"POST","url":"https://example.test/","body":{"mode":"urlencoded","urlencoded":[{"key":"same","value":"a b"},{"key":"same","value":"=界"},{"key":"disabled","value":"drop","disabled":true},{"key":"","value":"skip"}]}}),
    );
    let request = plan(&collection, Path::new("/synthetic"));
    assert_eq!(
        String::from_utf8(body(&request)).unwrap(),
        "same=a+b&same=%3D%E7%95%8C"
    );
    assert_eq!(
        header(&request, "Content-Type"),
        Some("application/x-www-form-urlencoded")
    );
}
#[test]
fn multipart_text_and_multiple_files_are_planned_without_io_and_materialized_exactly() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("one.bin"), [0, 255, 7]).unwrap();
    std::fs::write(root.path().join("two.txt"), "second").unwrap();
    let collection = collection(
        json!({"method":"POST","url":"https://example.test/","body":{"mode":"formdata","formdata":[{"key":"a\"b","value":"text界","type":"text"},{"key":"file","src":["one.bin","two.txt"],"type":"file","contentType":"application/octet-stream"},{"key":"legacy","value":"one.bin","type":"file"},{"key":"disabled","src":"does-not-exist","type":"file","disabled":true}]}}),
    );
    let request = plan(&collection, root.path());
    assert!(matches!(&request.body,BodyPlan::Multipart{parts,..}if parts.len()==4));
    let bytes = body(&request);
    assert!(bytes.windows(3).any(|part| part == [0, 255, 7]));
    let text = String::from_utf8_lossy(&bytes);
    assert!(text.contains("name=\"a\\\"b\""));
    assert!(text.contains("filename=\"one.bin\""));
    assert!(text.contains("filename=\"two.txt\""));
    assert!(text.contains("Content-Type: application/octet-stream\r\n"));
    assert!(text.contains("text界"));
    let BodyPlan::Multipart { boundary, .. } = &request.body else {
        unreachable!()
    };
    assert_eq!(
        header(&request, "Content-Type"),
        Some(format!("multipart/form-data; boundary={boundary}").as_str())
    );
    assert!(bytes.ends_with(format!("--{boundary}--\r\n").as_bytes()));
}
#[test]
fn binary_file_body_missing_files_buffer_limits_and_cancellation_fail_before_networking() {
    let root = tempfile::tempdir().unwrap();
    let collection = collection(
        json!({"method":"PUT","url":"https://example.test/","body":{"mode":"file","file":{"src":"file.bin"}}}),
    );
    let request = plan(&collection, root.path());
    assert!(
        request
            .body
            .materialize(100, &CancellationToken::new())
            .is_err()
    );
    std::fs::write(root.path().join("file.bin"), [0, 255, 7]).unwrap();
    assert_eq!(body(&request), [0, 255, 7]);
    assert!(
        request
            .body
            .materialize(2, &CancellationToken::new())
            .is_err()
    );
    let cancel = CancellationToken::new();
    cancel.cancel();
    assert!(matches!(
        request.body.materialize(100, &cancel),
        Err(kode_core::DomainError::Cancelled)
    ));
}
#[test]
fn graphql_query_and_nested_variable_objects_or_json_text_are_resolved_as_json() {
    for variables in [
        json!({"id":"{{id}}","nested":[true,{"value":"{{id}}"}]}),
        json!("{\"id\":\"{{id}}\",\"nested\":[true,{\"value\":\"{{id}}\"}]}"),
    ] {
        let mut collection = collection(
            json!({"method":"POST","url":"https://example.test/","body":{"mode":"graphql","graphql":{"query":"query { {{field}} }","variables":variables}}}),
        );
        collection["variable"] = json!([{"key":"id","value":"42"},{"key":"field","value":"name"}]);
        let request = plan(&collection, Path::new("/synthetic"));
        let actual: Value = serde_json::from_slice(&body(&request)).unwrap();
        assert_eq!(
            actual,
            json!({"query":"query { name }","variables":{"id":"42","nested":[true,{"value":"42"}]}})
        );
        assert_eq!(header(&request, "Content-Type"), Some("application/json"));
    }
}
#[test]
fn path_values_are_encoded_query_duplicates_and_fragments_survive_and_parameters_do_not_match_prefixes()
 {
    let collection = collection(
        json!({"method":"GET","url":{"raw":"https://example.test:8443/users/:id/:identity?old=drop#fragment","variable":[{"key":"id","value":"界/a b"},{"key":"identity","value":"stable"}],"query":[{"key":"q","value":"a b"},{"key":"q","value":"界"},{"key":"disabled","value":"drop","disabled":true}]}}),
    );
    let request = plan(&collection, Path::new("/synthetic"));
    assert_eq!(
        request.url,
        "https://example.test:8443/users/%E7%95%8C%2Fa%20b/stable?q=a+b&q=%E7%95%8C#fragment"
    );
    let preview = request.preview(&[], 0).unwrap();
    assert!(preview.text.starts_with("GET /users/"));
    assert!(preview.text.contains("Host: example.test:8443\n"));
    assert!(!preview.text.contains("#fragment"));
    let collection = self::collection(
        json!({"url":{"protocol":"https","host":["api","example","test"],"port":"8443","path":["v1",":id"],"variable":[{"key":"id","value":"42"}]}}),
    );
    assert_eq!(
        plan(&collection, Path::new("/synthetic")).url,
        "https://api.example.test:8443/v1/42"
    );
}
#[test]
fn basic_api_key_header_auth_settings_and_logical_preview_do_not_mutate_the_outgoing_body() {
    let collection = collection(
        json!({"method":"POST","url":"https://example.test/send","auth":{"type":"basic","basic":[{"key":"username","value":"user"},{"key":"password","value":"pass"}]},"header":[{"key":"Host","value":"ignored.example"},{"key":"Content-Length","value":"999"},{"key":"Connection","value":"keep-alive"}],"body":{"mode":"raw","raw":"界😀"}}),
    );
    let mut collection = collection;
    collection["item"][0]["protocolProfileBehavior"] =
        json!({"requestTimeout":-1,"disableRedirects":true,"disableSslVerification":true});
    let request = plan(&collection, Path::new("/synthetic"));
    assert_eq!(
        header(&request, "Authorization"),
        Some("Basic dXNlcjpwYXNz")
    );
    assert_eq!(request.timeout_ms, 1);
    assert!(!request.follow_redirects && !request.ssl_validation);
    let bytes = body(&request);
    let preview = request.preview(&bytes, 3).unwrap();
    assert!(preview.body_truncated);
    assert!(preview.text.contains("Content-Length: 7\n"));
    assert!(preview.text.contains("Host: example.test\n"));
    assert!(
        !preview.text.contains("ignored.example")
            && !preview.text.contains("999")
            && !preview.text.contains("Connection:")
    );
    assert_eq!(body(&request), bytes);
    collection["item"][0]["request"]["auth"] = json!({"type":"apikey","apikey":[{"key":"key","value":"X-Api-Key"},{"key":"value","value":"key"},{"key":"in","value":"header"}]});
    assert_eq!(
        header(&plan(&collection, Path::new("/synthetic")), "X-Api-Key"),
        Some("key")
    );
}
#[test]
fn unsupported_modes_invalid_headers_urls_and_graphql_are_explicit_errors() {
    for request in [
        json!({"url":"file:///tmp/private"}),
        json!({"url":"https://example.test/","method":"GET\nINJECT"}),
        json!({"url":"https://example.test/","header":[{"key":"bad:name","value":"x"}]}),
        json!({"url":"https://example.test/","header":[{"key":"X-Test","value":"secret\r\nInjected: yes"}]}),
        json!({"url":"https://example.test/","body":{"mode":"unsupported"}}),
        json!({"url":"https://example.test/","body":{"mode":"graphql","graphql":{"query":"query{}","variables":"invalid json"}}}),
    ] {
        let collection = collection(request);
        assert!(
            RequestResolver::new(
                &collection,
                NodePath(vec![0]),
                Path::new("/synthetic"),
                None
            )
            .unwrap()
            .plan()
            .is_err()
        );
    }
}
#[test]
fn multipart_explicit_boundaries_are_used_and_missing_boundary_is_supplied_in_the_resolved_header()
{
    for content_type in [
        "multipart/form-data; boundary=chosen-boundary",
        "multipart/form-data; boundary=\"chosen boundary\"",
        "multipart/form-data; charset=utf-8",
    ] {
        let collection = collection(
            json!({"url":"https://example.test/","header":[{"key":"Content-Type","value":content_type}],"body":{"mode":"formdata","formdata":[{"key":"field","value":"value"}]}}),
        );
        let request = plan(&collection, Path::new("/synthetic"));
        let BodyPlan::Multipart { boundary, .. } = &request.body else {
            unreachable!()
        };
        let bytes = body(&request);
        assert!(bytes.starts_with(format!("--{boundary}\r\n").as_bytes()));
        if content_type.contains("boundary=") {
            assert_eq!(header(&request, "Content-Type"), Some(content_type));
        } else {
            assert!(
                header(&request, "Content-Type")
                    .unwrap()
                    .contains(&format!("boundary={boundary}"))
            );
        }
    }
    let collection = collection(
        json!({"url":"https://example.test/","header":[{"key":"Content-Type","value":"multipart/form-data; boundary=\"bad;boundary\""}],"body":{"mode":"formdata"}}),
    );
    assert!(
        RequestResolver::new(
            &collection,
            NodePath(vec![0]),
            Path::new("/synthetic"),
            None
        )
        .unwrap()
        .plan()
        .is_err()
    );
}
#[test]
fn path_parameters_do_not_modify_query_content_and_header_inheritance_retains_source_fallback() {
    let mut collection = collection(
        json!({"url":{"raw":"https://example.test?target=/example/:id","variable":[{"key":"id","value":"42"}]},"header":[{"key":"X-Duplicate","value":"first"},{"key":"x-duplicate","value":"last"},{"key":"X-Inherited","value":"disabled override","disabled":true}]}),
    );
    collection["x-kode-headers"] = json!("future malformed field retained");
    collection["header"] = json!([{"key":"X-Inherited","value":"parent"}]);
    let request = plan(&collection, Path::new("/synthetic"));
    assert!(request.url.contains("target=/example/:id"));
    assert_eq!(header(&request, "X-Inherited"), Some("parent"));
    assert_eq!(header(&request, "X-Duplicate"), Some("last"));
    assert_eq!(
        collection["x-kode-headers"],
        "future malformed field retained"
    );
}
#[test]
fn prepared_file_bytes_are_an_immutable_capture_and_custom_multipart_collisions_fail() {
    let root = tempfile::tempdir().unwrap();
    let file = root.path().join("body.bin");
    std::fs::write(&file, b"captured").unwrap();
    let collection = collection(
        json!({"url":"https://example.test/","body":{"mode":"file","file":{"src":"body.bin"}}}),
    );
    let prepared = plan(&collection, root.path())
        .materialize(100, &CancellationToken::new())
        .unwrap();
    std::fs::write(&file, b"newer file").unwrap();
    assert_eq!(&*prepared.body, b"captured");
    assert!(prepared.preview(100).unwrap().text.ends_with("captured"));
    let collection = self::collection(
        json!({"url":"https://example.test/","header":[{"key":"Content-Type","value":"multipart/form-data; boundary=chosen"}],"body":{"mode":"formdata","formdata":[{"key":"text","value":"text\r\n--chosen\r\ninjected"}]}}),
    );
    let request = plan(&collection, root.path());
    assert!(
        request
            .body
            .materialize(1024, &CancellationToken::new())
            .is_err()
    );
}
#[test]
fn file_errors_and_prepared_debug_do_not_expose_resolved_secret_values() {
    let root = tempfile::tempdir().unwrap();
    let environment: Environment = serde_json::from_value(
        json!({"values":[{"key":"file","value":"synthetic-private-filename","secret":true}]}),
    )
    .unwrap();
    let collection = collection(
        json!({"url":"https://example.test/","body":{"mode":"file","file":{"src":"{{file}}"}}}),
    );
    let request = RequestResolver::new(
        &collection,
        NodePath(vec![0]),
        root.path(),
        Some(&environment),
    )
    .unwrap()
    .plan()
    .unwrap();
    let error = request
        .materialize(1024, &CancellationToken::new())
        .unwrap_err();
    assert!(!format!("{error:?} / {error}").contains("synthetic-private-filename"));
}
#[test]
fn auth_replacement_follows_source_header_order_after_removing_an_existing_value() {
    let collection = collection(
        json!({"url":"https://example.test/","header":[{"key":"X-Key","value":"old"},{"key":"X-Other","value":"other"}],"auth":{"type":"apikey","apikey":[{"key":"key","value":"X-Key"},{"key":"value","value":"new"},{"key":"in","value":"header"}]}}),
    );
    let request = plan(&collection, Path::new("/synthetic"));
    assert_eq!(
        request
            .headers
            .iter()
            .map(|header| header.name.as_str())
            .collect::<Vec<_>>(),
        ["X-Other", "X-Key"]
    );
    assert_eq!(header(&request, "X-Key"), Some("new"));
}
