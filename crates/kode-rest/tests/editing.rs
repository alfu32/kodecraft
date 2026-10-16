use kode_platform::jobs::CancellationToken;
use kode_rest::{
    model::{
        NodePath, Workspace,
        editing::{self as edit, EntryValue, Field, Rows},
    },
    resolve::RequestResolver,
};
use serde_json::{Value, json};
use std::path::Path;
fn fixture(request: Value) -> (Workspace, NodePath) {
    let mut model = Workspace::default();
    model.replace_collection(json!({"info":{"name":"API","x-keep":7},"item":[{"name":"Call","x-keep":{"nested":true},"request":request,"response":[{"body":"saved"}]}],"event":[{"script":{"exec":["untouched"]}}]})).unwrap();
    (model, NodePath(vec![0]))
}
fn plan(model: &Workspace, path: &NodePath, root: &Path) -> kode_rest::resolve::ResolvedRequest {
    RequestResolver::new(model.collection(), path.clone(), root, None)
        .unwrap()
        .plan()
        .unwrap()
}
#[test]
fn node_fields_auth_variables_and_settings_preserve_unknowns_and_structured_values() {
    let (mut model, path) =
        fixture(json!({"url":"https://example.test/","method":"GET","x-request":{"keep":1}}));
    let root = NodePath::default();
    edit::field(&mut model, &root, Field::Name, "Renamed").unwrap();
    edit::field(&mut model, &root, Field::Description, "Description").unwrap();
    assert_eq!(model.collection()["info"]["x-keep"], 7);
    edit::auth(&mut model, &root, Some("bearer")).unwrap();
    let mut auth = model.collection()["auth"].clone();
    auth["bearer"][0]["value"] = "original-token".into();
    auth["x-auth"] = json!({"keep":2});
    model.set_node_field(&root, "auth", Some(auth)).unwrap();
    edit::auth(&mut model, &root, Some("bearer")).unwrap();
    assert_eq!(
        model.collection()["auth"]["bearer"][0]["value"],
        "original-token"
    );
    edit::auth(&mut model, &root, Some("noauth")).unwrap();
    assert_eq!(model.collection()["auth"]["x-auth"], json!({"keep":2}));
    let index = edit::add_row(&mut model, &path, Rows::Variable).unwrap();
    model.set_node_field(&path,"variable",Some(json!([{"key":"object","value":{"nested":7},"type":"secret","description":{"content":"note","x-desc":1},"x-row":9}]))).unwrap();
    let mut value = edit::entry_value(
        &edit::rows(model.node(&path).unwrap(), &path, Rows::Variable).unwrap()[index],
        Rows::Variable,
    );
    value.description = "changed".into();
    value.enabled = false;
    edit::edit_row(&mut model, &path, Rows::Variable, index, &value).unwrap();
    let row = &model.node(&path).unwrap()["variable"][0];
    assert_eq!(row["value"], json!({"nested":7}));
    assert_eq!(row["x-row"], 9);
    assert_eq!(row["description"]["x-desc"], 1);
    assert_eq!(row["description"]["content"], "changed");
    assert_eq!(row["disabled"], true);
    edit::field(&mut model, &path, Field::Method, "PATCH").unwrap();
    edit::setting(&mut model, &path, "disableRedirects").unwrap();
    edit::setting(&mut model, &path, "disableSslVerification").unwrap();
    edit::field(&mut model, &path, Field::Timeout, "1200").unwrap();
    let request = plan(&model, &path, Path::new("."));
    assert_eq!(request.method, "PATCH");
    assert!(!request.follow_redirects);
    assert!(!request.ssl_validation);
    assert_eq!(request.timeout_ms, 1200);
    assert_eq!(
        model.node(&path).unwrap()["request"]["x-request"],
        json!({"keep":1})
    );
    assert_eq!(model.node(&path).unwrap()["response"][0]["body"], "saved");
    assert_eq!(
        model.collection()["event"][0]["script"]["exec"][0],
        "untouched"
    );
}
#[test]
fn raw_url_host_edits_retain_query_metadata_and_explicit_query_edits_do_not_send_stale_rows() {
    let (mut model, path) = fixture(
        json!({"url":{"raw":"https://old.example.test/x?q=old#f","query":[{"key":"q","value":"old","x-row":7},{"key":"disabled","value":"never","disabled":true}],"variable":[{"key":"id","value":"1"}],"x-url":{"keep":1}}}),
    );
    edit::field(
        &mut model,
        &path,
        Field::Url,
        "https://new.example.test/x?q=old#f",
    )
    .unwrap();
    assert_eq!(
        model.node(&path).unwrap()["request"]["url"]["query"][0]["x-row"],
        7
    );
    edit::field(
        &mut model,
        &path,
        Field::Url,
        "https://new.example.test/x?q=a%20b&flag#f",
    )
    .unwrap();
    assert!(
        model.node(&path).unwrap()["request"]["url"]
            .get("query")
            .is_none()
    );
    assert_eq!(
        plan(&model, &path, Path::new(".")).url,
        "https://new.example.test/x?q=a%20b&flag#f"
    );
    assert_eq!(
        model.node(&path).unwrap()["request"]["url"]["x-url"],
        json!({"keep":1})
    );
    assert_eq!(
        model.node(&path).unwrap()["request"]["url"]["variable"][0]["value"],
        "1"
    );
}
#[test]
fn structured_query_edits_decode_once_keep_flags_duplicates_and_fragments_and_exclude_disabled_rows()
 {
    let (mut model, path) =
        fixture(json!({"url":"https://example.test/x?flag&q=a%20b&q=two+words#frag"}));
    let rows = edit::rows(model.node(&path).unwrap(), &path, Rows::Query).unwrap();
    assert_eq!(rows[0]["value"], Value::Null);
    assert_eq!(rows[1]["value"], "a b");
    assert_eq!(rows[2]["value"], "two words");
    let mut value = edit::entry_value(&rows[1], Rows::Query);
    value.value = "after &".into();
    edit::edit_row(&mut model, &path, Rows::Query, 1, &value).unwrap();
    assert_eq!(
        plan(&model, &path, Path::new(".")).url,
        "https://example.test/x?flag&q=after+%26&q=two+words#frag"
    );
    edit::toggle_row(&mut model, &path, Rows::Query, 0).unwrap();
    assert_eq!(
        plan(&model, &path, Path::new(".")).url,
        "https://example.test/x?q=after+%26&q=two+words#frag"
    );
    assert_eq!(
        edit::raw_url(&model.node(&path).unwrap()["request"]["url"]),
        "https://example.test/x?q=after+%26&q=two+words#frag"
    );
    edit::remove_row(&mut model, &path, Rows::Query, 1).unwrap();
    assert_eq!(
        plan(&model, &path, Path::new(".")).url,
        "https://example.test/x?q=two+words#frag"
    );
    let (mut model, path) = fixture(
        json!({"url":{"protocol":"https","host":["structured","example","test"],"path":["x"]}}),
    );
    edit::add_row(&mut model, &path, Rows::Query).unwrap();
    assert_eq!(
        plan(&model, &path, Path::new(".")).url,
        "https://structured.example.test/x?param="
    );
    edit::field(&mut model, &path, Field::Url, "https://example.test/?q=%FF").unwrap();
    let before = model.export_collection();
    assert!(edit::add_row(&mut model, &path, Rows::Query).is_err());
    assert_eq!(model.collection(), &*before);
}
#[test]
fn body_mode_edits_reach_materialization_without_losing_inactive_modes_or_file_metadata() {
    let directory = tempfile::tempdir().unwrap();
    std::fs::write(directory.path().join("one.bin"), b"FILE-ONE").unwrap();
    std::fs::write(directory.path().join("two.bin"), b"FILE-TWO").unwrap();
    let (mut model, path) = fixture(
        json!({"url":"https://example.test/","body":{"mode":"raw","raw":"old","x-body":7,"file":{"src":"one.bin","x-file":9}}}),
    );
    let cancel = CancellationToken::new();
    edit::field(&mut model, &path, Field::RawBody, "raw changed").unwrap();
    assert_eq!(
        plan(&model, &path, directory.path())
            .body
            .materialize(4096, &cancel)
            .unwrap(),
        b"raw changed"
    );
    edit::body_mode(&mut model, &path, "urlencoded").unwrap();
    let index = edit::add_row(&mut model, &path, Rows::UrlEncoded).unwrap();
    edit::edit_row(
        &mut model,
        &path,
        Rows::UrlEncoded,
        index,
        &EntryValue {
            key: "a b".into(),
            value: "c&d".into(),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(
        plan(&model, &path, directory.path())
            .body
            .materialize(4096, &cancel)
            .unwrap(),
        b"a+b=c%26d"
    );
    edit::body_mode(&mut model, &path, "formdata").unwrap();
    let index = edit::add_row(&mut model, &path, Rows::FormData).unwrap();
    edit::edit_row(
        &mut model,
        &path,
        Rows::FormData,
        index,
        &EntryValue {
            key: "upload".into(),
            value: r#"["one.bin","two.bin"]"#.into(),
            description: "file".into(),
            ..Default::default()
        },
    )
    .unwrap();
    let bytes = plan(&model, &path, directory.path())
        .body
        .materialize(4096, &cancel)
        .unwrap();
    let text = String::from_utf8(bytes).unwrap();
    assert!(text.contains("FILE-ONE"));
    assert!(text.contains("FILE-TWO"));
    edit::body_mode(&mut model, &path, "file").unwrap();
    edit::field(&mut model, &path, Field::File, "two.bin").unwrap();
    assert_eq!(
        plan(&model, &path, directory.path())
            .body
            .materialize(4096, &cancel)
            .unwrap(),
        b"FILE-TWO"
    );
    assert_eq!(
        model.node(&path).unwrap()["request"]["body"]["file"]["x-file"],
        9
    );
    edit::body_mode(&mut model, &path, "graphql").unwrap();
    edit::field(
        &mut model,
        &path,
        Field::GraphqlQuery,
        "query { item(id: 1) }",
    )
    .unwrap();
    edit::field(&mut model, &path, Field::GraphqlVariables, "{\"id\":2}").unwrap();
    let bytes = plan(&model, &path, directory.path())
        .body
        .materialize(4096, &cancel)
        .unwrap();
    let graphql: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(graphql["variables"], json!({"id":2}));
    edit::field(&mut model, &path, Field::GraphqlVariables, "{unfinished").unwrap();
    assert!(
        RequestResolver::new(model.collection(), path.clone(), directory.path(), None)
            .unwrap()
            .plan()
            .is_err()
    );
    edit::body_mode(&mut model, &path, "raw").unwrap();
    assert_eq!(
        plan(&model, &path, directory.path())
            .body
            .materialize(4096, &cancel)
            .unwrap(),
        b"raw changed"
    );
    assert_eq!(model.node(&path).unwrap()["request"]["body"]["x-body"], 7);
}
#[test]
fn header_edits_keep_metadata_across_renames_reorders_duplicates_and_opaque_entries_and_invalid_input_is_atomic()
 {
    let (mut model, path) = fixture(
        json!({"url":"https://example.test/","header":[{"key":"X-Old","value":"one","description":{"content":"keep"},"x-row":1},17,{"key":"X-Dup","value":"first","x-row":2},{"key":"x-dup","value":"last","x-row":3}]}),
    );
    edit::headers(
        &mut model,
        &path,
        "X-New: changed\n# X-Dup: first\nx-dup: last",
    )
    .unwrap();
    let headers = &model.node(&path).unwrap()["request"]["header"];
    assert_eq!(headers[0]["key"], "X-New");
    assert_eq!(headers[0]["x-row"], 1);
    assert_eq!(headers[0]["description"]["content"], "keep");
    assert_eq!(headers[1], 17);
    assert_eq!(headers[2]["x-row"], 2);
    assert_eq!(headers[2]["disabled"], true);
    assert_eq!(headers[3]["x-row"], 3);
    let before = model.export_collection();
    assert!(edit::headers(&mut model, &path, "X-New: ok\nunfinished").is_err());
    assert_eq!(model.collection(), &*before);
    edit::headers(
        &mut model,
        &path,
        "x-dup: last\nX-New: changed\n# X-Dup: first",
    )
    .unwrap();
    let headers = &model.node(&path).unwrap()["request"]["header"];
    assert_eq!(headers[0]["x-row"], 2);
    assert_eq!(headers[2]["x-row"], 1);
    assert_eq!(headers[3]["x-row"], 3);
}
#[test]
fn incomplete_imports_require_explicit_acknowledgement_and_invalid_targets_never_fall_back_to_root()
{
    let (mut model, path) = fixture(json!({"url":"https://example.test/"}));
    model
        .set_node_field(&path, "x-kode-import-incomplete", Some(true.into()))
        .unwrap();
    edit::field(&mut model, &path, Field::Method, "GET").unwrap();
    assert!(
        RequestResolver::new(model.collection(), path.clone(), Path::new("."), None)
            .unwrap()
            .plan()
            .is_err()
    );
    edit::acknowledge_import(&mut model, &path).unwrap();
    assert!(
        RequestResolver::new(model.collection(), path, Path::new("."), None)
            .unwrap()
            .plan()
            .is_ok()
    );
    let invalid = NodePath(vec![99]);
    let before = model.export_collection();
    assert!(edit::field(&mut model, &invalid, Field::Method, "POST").is_err());
    assert!(edit::auth(&mut model, &invalid, Some("noauth")).is_err());
    assert!(edit::add_row(&mut model, &invalid, Rows::Variable).is_err());
    assert_eq!(model.collection(), &*before);
}
