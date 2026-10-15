use kode_database::{
    catalog::{DriverRegistry, UrlCatalog},
    config::{
        DataSourceRepository,
        recents::{UrlRecentRepository, UrlSuggestionService},
    },
    credentials::{CredentialStore, Secret, parse_java_properties},
    model::{DataSourceDefinition, DatabaseProjectState},
};
use serde_json::{Value, json};
use std::{collections::BTreeMap, fs};
fn definition(id: &str, name: &str) -> DataSourceDefinition {
    serde_json::from_value(json!({"id":id,"name":name,"driver":{"id":"h2","displayName":"H2","driverClass":"org.h2.Driver","coordinates":"com.h2database:h2:2.2.224"},"jdbcUrl":"jdbc:h2:mem:test","username":"sa","credentialReference":"synthetic.password"})).unwrap()
}
#[test]
fn datasource_repository_and_private_credentials_match_source_upsert_remove_and_name_order_without_exporting_values()
 {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("rust-database");
    let repo = DataSourceRepository::new(root.clone());
    let credentials = CredentialStore::new(root.clone());
    assert!(repo.load().unwrap().is_empty());
    assert!(credentials.get(None).unwrap().is_none());
    assert!(!root.exists());
    let local = definition("local", "local-h2");
    repo.upsert(local.clone()).unwrap();
    let secret = Secret::new("synthetic-value".into());
    credentials.put("synthetic.password", &secret).unwrap();
    assert_eq!(repo.load().unwrap(), [local]);
    assert_eq!(
        credentials
            .get(Some("synthetic.password"))
            .unwrap()
            .unwrap()
            .expose(),
        secret.expose()
    );
    assert_eq!(format!("{secret:?}"), "[REDACTED]");
    assert_eq!(secret.to_string(), "[REDACTED]");
    assert!(
        !fs::read_to_string(root.join("datasources-v1.json"))
            .unwrap()
            .contains(secret.expose())
    );
    repo.upsert(definition("z", "Zulu")).unwrap();
    repo.upsert(definition("a", "alpha")).unwrap();
    assert_eq!(
        repo.load()
            .unwrap()
            .iter()
            .map(|source| source.id.as_str())
            .collect::<Vec<_>>(),
        ["a", "local", "z"]
    );
    repo.upsert(definition("z", "Beta")).unwrap();
    assert_eq!(repo.load().unwrap().len(), 3);
    repo.remove("local").unwrap();
    assert_eq!(repo.load().unwrap().len(), 2);
    credentials.remove(Some("synthetic.password")).unwrap();
    assert!(
        credentials
            .get(Some("synthetic.password"))
            .unwrap()
            .is_none()
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(root.join("credentials-v1.json"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
}
#[test]
fn java_properties_fixture_matches_actual_latin1_keys_continuations_comments_and_utf16_escape_semantics()
 {
    let fixture: Value = serde_json::from_slice(include_bytes!(
        "../../../fixtures/database/java-properties.json"
    ))
    .unwrap();
    let bytes: Vec<_> = fixture["inputLatin1"]
        .as_str()
        .unwrap()
        .chars()
        .map(|ch| u8::try_from(ch as u32).unwrap())
        .collect();
    let expected: BTreeMap<String, String> =
        serde_json::from_value(fixture["expected"].clone()).unwrap();
    assert_eq!(parse_java_properties(&bytes).unwrap(), expected);
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("legacy.properties");
    fs::write(&path, &bytes).unwrap();
    let store = CredentialStore::new(directory.path().join("rust"));
    assert_eq!(store.import_java_properties(&path).unwrap(), 11);
    assert_eq!(store.import_java_properties(&path).unwrap(), 0);
    assert_eq!(store.get(Some("unicode")).unwrap().unwrap().expose(), "é😀");
    assert_eq!(fs::read(path).unwrap(), bytes);
    for bytes in [b"ref=\\u12zz".as_slice(), b"ref=\\uD800", b"ref=\\uDC00"] {
        let error = parse_java_properties(bytes).unwrap_err();
        assert!(!error.to_string().contains("ref="));
    }
}
#[test]
fn explicit_legacy_import_preserves_original_urls_classes_refs_unknowns_and_separates_secret_properties()
 {
    let directory = tempfile::tempdir().unwrap();
    let legacy = directory.path().join("legacy");
    let root = directory.path().join("rust");
    fs::create_dir(&legacy).unwrap();
    let mut source = serde_json::to_value(definition("local", "Legacy H2")).unwrap();
    source["driver"]["jarPath"] = json!("/synthetic/first.jar:/synthetic/second.jar");
    source["driver"]["futureDriver"] = json!([1, 2]);
    source["properties"] = json!({"sslpassword":"synthetic-key-value","applicationName":"kode"});
    source["futureSource"] = json!({"value":42});
    let original =
        serde_json::to_vec(&json!({"version":1,"dataSources":[source],"futureStore":true}))
            .unwrap();
    let properties = b"synthetic.password=synthetic\\ value\\u00E9\n";
    fs::write(legacy.join("datasources.json"), &original).unwrap();
    fs::write(legacy.join("credentials.properties"), properties).unwrap();
    let repo = DataSourceRepository::new(root.clone());
    let credentials = CredentialStore::new(root.clone());
    let report = repo.import_legacy(&legacy, &credentials).unwrap();
    assert_eq!(report.data_sources, 1);
    assert_eq!(report.credentials, 2);
    assert!(!report.already_present);
    let sources = repo.load().unwrap();
    let source = &sources[0];
    assert_eq!(source.jdbc_url, "jdbc:h2:mem:test");
    assert_eq!(source.driver.driver_class, "org.h2.Driver");
    assert_eq!(
        source.driver.coordinates.as_deref(),
        Some("com.h2database:h2:2.2.224")
    );
    assert_eq!(
        source.driver.jar_path.as_deref(),
        Some("/synthetic/first.jar:/synthetic/second.jar")
    );
    assert_eq!(
        source.credential_reference.as_deref(),
        Some("synthetic.password")
    );
    assert_eq!(
        credentials
            .get(source.credential_reference.as_deref())
            .unwrap()
            .unwrap()
            .expose(),
        "synthetic valueé"
    );
    let reference = &source.credential_properties["sslpassword"];
    assert_eq!(
        credentials.get(Some(reference)).unwrap().unwrap().expose(),
        "synthetic-key-value"
    );
    assert_eq!(source.properties["applicationName"], "kode");
    assert!(!source.properties.contains_key("sslpassword"));
    assert_eq!(source.extra["futureSource"]["value"], 42);
    let export = serde_json::to_string(&DatabaseProjectState {
        data_sources: sources,
        ..Default::default()
    })
    .unwrap();
    assert!(!export.contains("synthetic value"));
    assert!(!export.contains("synthetic-key-value"));
    let public = fs::read_to_string(root.join("datasources-v1.json")).unwrap();
    assert!(public.contains("futureStore"));
    assert!(!public.contains("synthetic-key-value"));
    assert_eq!(fs::read(legacy.join("datasources.json")).unwrap(), original);
    assert_eq!(
        fs::read(legacy.join("credentials.properties")).unwrap(),
        properties
    );
    assert!(
        repo.import_legacy(&legacy, &credentials)
            .unwrap()
            .already_present
    );
    assert!(!legacy.join(".database-config.lock").exists());
    assert!(repo.import_legacy(&root, &credentials).is_err());
}
#[test]
fn unsafe_legacy_import_and_reference_collision_preserve_both_sources_and_existing_private_values()
{
    let directory = tempfile::tempdir().unwrap();
    let legacy = directory.path().join("legacy");
    let root = directory.path().join("rust");
    fs::create_dir(&legacy).unwrap();
    let repo = DataSourceRepository::new(root.clone());
    let credentials = CredentialStore::new(root.clone());
    let mut source = serde_json::to_value(definition("local", "H2")).unwrap();
    source["jdbcUrl"] = json!("jdbc:h2:mem:test;PASSWORD=synthetic-leak");
    fs::write(
        legacy.join("datasources.json"),
        serde_json::to_vec(&json!({"dataSources":[source]})).unwrap(),
    )
    .unwrap();
    fs::write(
        legacy.join("credentials.properties"),
        b"synthetic.password=synthetic-new\n",
    )
    .unwrap();
    let error = repo.import_legacy(&legacy, &credentials).unwrap_err();
    assert!(!error.to_string().contains("synthetic-leak"));
    assert!(!root.join("credentials-v1.json").exists());
    assert!(!root.join("datasources-v1.json").exists());
    credentials
        .put("synthetic.password", &Secret::new("synthetic-old".into()))
        .unwrap();
    let before = fs::read(root.join("credentials-v1.json")).unwrap();
    fs::write(
        legacy.join("datasources.json"),
        serde_json::to_vec(&json!({"dataSources":[definition("local","H2")]})).unwrap(),
    )
    .unwrap();
    assert!(repo.import_legacy(&legacy, &credentials).is_err());
    assert_eq!(fs::read(root.join("credentials-v1.json")).unwrap(), before);
    assert!(!root.join("datasources-v1.json").exists());
}
#[test]
fn recents_are_exact_trimmed_mru_twelve_by_family_persistent_and_safe_with_source_seed_order() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("rust");
    let recents = UrlRecentRepository::new(root.clone());
    assert!(
        !recents
            .remember("postgresql", "jdbc:postgresql://user:synthetic@host/db")
            .unwrap()
    );
    assert!(!root.exists());
    assert!(
        recents
            .remember("postgresql", " jdbc:postgresql://one/db ")
            .unwrap()
    );
    recents
        .remember("postgresql", "jdbc:postgresql://two/db")
        .unwrap();
    recents
        .remember("postgresql", "jdbc:postgresql://one/db")
        .unwrap();
    assert_eq!(
        recents.load("postgresql").unwrap(),
        ["jdbc:postgresql://one/db", "jdbc:postgresql://two/db"]
    );
    for index in 0..20 {
        recents
            .remember("postgresql", &format!("jdbc:postgresql://host{index}/db"))
            .unwrap();
    }
    let loaded = UrlRecentRepository::new(root.clone())
        .load("postgresql")
        .unwrap();
    assert_eq!(loaded.len(), 12);
    assert_eq!(loaded[0], "jdbc:postgresql://host19/db");
    assert!(!loaded.iter().any(|url| url.contains("one/")));
    assert!(
        !recents
            .remember("postgresql", "jdbc:postgresql://host/db?PWD=synthetic")
            .unwrap()
    );
    assert!(
        !recents
            .remember(
                "postgresql",
                "jdbc:postgresql://host/db;OAuthClientSecret=synthetic"
            )
            .unwrap()
    );
    assert!(
        recents
            .remember(
                "postgresql",
                "jdbc:postgresql://host/db?applicationName=kode&sslmode=require"
            )
            .unwrap()
    );
    let registry = DriverRegistry::compiled().unwrap();
    let service = UrlSuggestionService {
        catalog: UrlCatalog::compiled(&registry).unwrap(),
        recents,
    };
    service
        .remember("postgresql", "jdbc:postgresql://shared/db")
        .unwrap();
    service
        .remember("hive", "jdbc:hive2://shared:10000/db")
        .unwrap();
    assert_eq!(
        service.for_driver("cockroachdb").unwrap().recents[0],
        "jdbc:postgresql://shared/db"
    );
    assert_eq!(
        service.for_driver("spark-thrift").unwrap().recents,
        ["jdbc:hive2://shared:10000/db"]
    );
    service
        .recents
        .seed(
            "mysql",
            &[
                "jdbc:mysql://FIRST/db".into(),
                "jdbc:mysql://second/db".into(),
                "jdbc:mysql://FIRST/db".into(),
            ],
        )
        .unwrap();
    assert_eq!(
        service.recents.load("mysql").unwrap(),
        ["jdbc:mysql://FIRST/db", "jdbc:mysql://second/db"]
    );
}
#[test]
fn recents_import_discards_credential_urls_without_changing_legacy_bytes_or_overwriting_rust_state()
{
    let directory = tempfile::tempdir().unwrap();
    let legacy = directory.path().join("legacy");
    let root = directory.path().join("rust");
    fs::create_dir(&legacy).unwrap();
    let bytes=serde_json::to_vec(&json!({"version":1,"families":{"postgresql":["jdbc:postgresql://one/db","jdbc:postgresql://host/db?password=synthetic","jdbc:postgresql://one/db","jdbc:postgresql://TWO/db"]},"future":42})).unwrap();
    fs::write(legacy.join("jdbc-url-recents.json"), &bytes).unwrap();
    let recents = UrlRecentRepository::new(root.clone());
    assert!(recents.import_legacy(&legacy).unwrap());
    assert_eq!(
        recents.load("postgresql").unwrap(),
        ["jdbc:postgresql://one/db", "jdbc:postgresql://TWO/db"]
    );
    assert_eq!(
        fs::read(legacy.join("jdbc-url-recents.json")).unwrap(),
        bytes
    );
    assert!(
        !fs::read_to_string(root.join("jdbc-url-recents-v1.json"))
            .unwrap()
            .contains("synthetic")
    );
    assert!(!recents.import_legacy(&legacy).unwrap());
}
#[test]
fn corrupted_stores_and_competing_writers_are_visible_and_never_overwrite_existing_configuration() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let repo = DataSourceRepository::new(root.into());
    repo.upsert(definition("local", "H2")).unwrap();
    let lock = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(root.join(".database-config.lock"))
        .unwrap();
    lock.try_lock().unwrap();
    let before = fs::read(root.join("datasources-v1.json")).unwrap();
    assert!(repo.upsert(definition("other", "Other")).is_err());
    assert_eq!(fs::read(root.join("datasources-v1.json")).unwrap(), before);
    lock.unlock().unwrap();
    fs::write(root.join("datasources-v1.json"), b"{corrupt").unwrap();
    assert!(repo.load().is_err());
    assert!(repo.upsert(definition("other", "Other")).is_err());
    assert_eq!(
        fs::read(root.join("datasources-v1.json")).unwrap(),
        b"{corrupt"
    );
}
#[cfg(unix)]
#[test]
fn private_permissions_and_symlink_targets_cannot_expose_or_replace_external_data() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("rust");
    let credentials = CredentialStore::new(root.clone());
    credentials
        .put("ref", &Secret::new("synthetic".into()))
        .unwrap();
    let path = root.join("credentials-v1.json");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
    assert!(credentials.get(Some("ref")).is_err());
    assert!(credentials.put("ref", &Secret::new("new".into())).is_err());
    fs::remove_file(&path).unwrap();
    let external = directory.path().join("external");
    fs::write(&external, b"external unchanged").unwrap();
    symlink(&external, &path).unwrap();
    assert!(credentials.get(Some("ref")).is_err());
    assert!(credentials.put("ref", &Secret::new("new".into())).is_err());
    assert_eq!(fs::read(&external).unwrap(), b"external unchanged");
    fs::remove_file(root.join(".database-config.lock")).unwrap();
    assert!(
        std::process::Command::new("mkfifo")
            .arg(root.join(".database-config.lock"))
            .status()
            .unwrap()
            .success()
    );
    assert!(
        DataSourceRepository::new(root.clone())
            .upsert(definition("fixture", "Fixture"))
            .is_err()
    );
}
