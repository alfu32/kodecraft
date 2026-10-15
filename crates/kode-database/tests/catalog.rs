use kode_database::{
    catalog::{DriverRegistry, UrlCatalog},
    model::*,
    security::{contains_secret, validate_persistent_json},
};
use serde_json::{Value, json};
#[test]
fn all_59_frozen_entries_resolve_with_exact_classes_artifacts_aliases_vendor_pages_and_url_templates()
 {
    let registry = DriverRegistry::compiled().unwrap();
    let urls = UrlCatalog::compiled(&registry).unwrap();
    assert_eq!(registry.definitions().len(), 59);
    let raw: Value =
        serde_json::from_slice(include_bytes!("../../../resources/database/drivers.json")).unwrap();
    for (entry, expected) in registry
        .definitions()
        .iter()
        .zip(raw["drivers"].as_array().unwrap())
    {
        assert_eq!(entry.id, expected["id"].as_str().unwrap());
        assert_eq!(
            entry.driver_class,
            expected["driverClass"].as_str().unwrap()
        );
        let spec = registry.spec_for(&entry.id).unwrap();
        let provider = registry
            .by_id(spec.provider_id.as_deref().unwrap())
            .unwrap();
        assert_eq!(
            spec.artifact.as_ref(),
            entry.artifact.as_ref().or(provider.artifact.as_ref())
        );
        assert!(urls.entry(&entry.id).is_some());
        assert!(!contains_secret(&entry.default_jdbc_url_template));
    }
    assert_eq!(registry.by_url("JDBC:H2:mem:kode").unwrap().id, "h2");
    assert_eq!(
        registry.by_url("jdbc:postgresql://host/db").unwrap().id,
        "postgresql"
    );
    assert_eq!(
        registry.by_url("jdbc:new-driver:").unwrap().id,
        "custom-jdbc"
    );
    assert!(registry.by_url("https://example.invalid").is_none());
    let alias = registry.spec_for("COCKROACHDB").unwrap();
    assert_eq!(alias.provider_id.as_deref(), Some("postgresql"));
    assert_eq!(
        alias.artifact.as_ref().unwrap().group_id.as_deref(),
        Some("org.postgresql")
    );
    assert_eq!(alias.artifact.as_ref().unwrap().version, None);
    assert_eq!(registry.spec_for("custom-jdbc").unwrap().artifact, None);
    assert_eq!(
        registry.spec_for("sqlserver-localdb").unwrap().driver_class,
        "sqlserver",
        "retain source class verbatim"
    );
    let vendor = registry.spec_for("athena").unwrap().artifact.unwrap();
    assert_eq!(vendor.repository_type, JdbcRepositoryType::VendorUrl);
    assert!(vendor.vendor_download_page.is_some());
    assert!(vendor.download_url.is_none());
    let custom = registry
        .spec_for("elasticsearch")
        .unwrap()
        .artifact
        .unwrap();
    assert_eq!(
        custom.repository.as_deref(),
        Some("https://artifacts.elastic.co/maven")
    );
    assert_eq!(urls.templates("postgresql"), urls.templates("cockroachdb"));
    assert_eq!(urls.templates("azure-sql")[0].id, "azure");
    assert!(
        urls.templates("sqlite")
            .iter()
            .all(|template| !template.template.contains("//"))
    );
    assert!(urls.templates("tarantool").is_empty());
    assert_eq!(urls.recent_family("cockroachdb"), "postgresql");
    assert_eq!(urls.recent_family("spark-thrift"), "hive");
}
#[test]
fn catalog_cycles_duplicates_missing_entries_secret_placeholders_and_tls_bypasses_are_rejected() {
    let registry = DriverRegistry::compiled().unwrap();
    let original: Value = serde_json::from_slice(include_bytes!(
        "../../../resources/database/jdbc-url-templates.json"
    ))
    .unwrap();
    for mutation in 0..6 {
        let mut value = original.clone();
        match mutation {
            0 => {
                value["drivers"].as_array_mut().unwrap().pop();
            }
            1 => {
                value["drivers"][0]["inherit"] = json!("postgresql");
            }
            2 => {
                value["drivers"][0]["templates"][0]["template"] =
                    json!("jdbc:postgresql://host/db?password={password}");
            }
            3 => {
                value["drivers"][0]["templates"][0]["template"] =
                    json!("jdbc:sqlserver://host;trustServerCertificate=true");
            }
            4 => {
                value["drivers"][0]["templates"][1]["id"] =
                    value["drivers"][0]["templates"][0]["id"].clone();
            }
            _ => {
                value["version"] = json!(2);
            }
        }
        assert!(
            UrlCatalog::from_json(&serde_json::to_vec(&value).unwrap(), &registry).is_err(),
            "mutation {mutation}"
        );
    }
    let mut value: Value =
        serde_json::from_slice(include_bytes!("../../../resources/database/drivers.json")).unwrap();
    value["drivers"][0]["aliasOf"] = json!("postgresql");
    assert!(DriverRegistry::from_json(&serde_json::to_vec(&value).unwrap()).is_err());
}
#[test]
fn safe_model_roundtrip_keeps_unknowns_references_urls_class_coordinates_and_local_multijar_paths()
{
    let value = json!({"dataSources":[{"id":"synthetic","name":"Local H2","driver":{"id":"h2","displayName":"H2","driverClass":"org.h2.Driver","coordinates":"com.h2database:h2:2.2.224","jarPath":"/synthetic/a.jar:/synthetic/b.jar","pluginField":{"value":42}},"jdbcUrl":"jdbc:h2:mem:test","username":"sa","credentialReference":"synthetic.password","properties":{"applicationName":"kode"},"futureDatasource":{"number":43}}],"sessions":[{"id":"console","dataSourceId":"synthetic","title":"SQL 1","buffer":"select 1"}],"futureProject":true});
    let state: DatabaseProjectState = serde_json::from_value(value).unwrap();
    let source = &state.data_sources[0];
    assert!(source.auto_commit);
    assert!(!source.read_only);
    assert_eq!(
        source.driver.local_classpath(':'),
        [
            std::path::PathBuf::from("/synthetic/a.jar"),
            std::path::PathBuf::from("/synthetic/b.jar")
        ]
    );
    let encoded = serde_json::to_value(&state).unwrap();
    validate_persistent_json(&encoded).unwrap();
    assert_eq!(encoded["dataSources"][0]["jdbcUrl"], "jdbc:h2:mem:test");
    assert_eq!(
        encoded["dataSources"][0]["driver"]["driverClass"],
        "org.h2.Driver"
    );
    assert_eq!(
        encoded["dataSources"][0]["driver"]["pluginField"]["value"],
        42
    );
    assert_eq!(encoded["futureProject"], true);
    assert_eq!(
        serde_json::from_value::<DatabaseProjectState>(encoded).unwrap(),
        state
    );
    assert!(!format!("{source:?}").contains("jdbc:h2"));
}
#[test]
fn credential_urls_and_properties_cannot_escape_through_direct_or_unknown_model_serialization() {
    let safe = json!({"id":"synthetic","name":"H2","driver":{"driverClass":"org.h2.Driver"},"jdbcUrl":"jdbc:h2:mem:test"});
    let mut source: DataSourceDefinition = serde_json::from_value(safe.clone()).unwrap();
    for url in [
        "jdbc:postgresql://user:synthetic-value@host/db",
        "jdbc:postgresql://host/db?PWD=synthetic-value",
        "jdbc:oracle:thin:user/synthetic-value@host:1521/db",
        "jdbc:db2://host/db:password=synthetic-value;",
        "jdbc:postgresql://host/db?%70ass%77ord=synthetic-value",
        "jdbc:postgresql://host/db;OAuthClientSecret=synthetic-value",
    ] {
        assert!(contains_secret(url));
        source.jdbc_url = url.into();
        let error = serde_json::to_string(&source).unwrap_err();
        assert!(!error.to_string().contains("synthetic-value"));
        let mut value = safe.clone();
        value["jdbcUrl"] = json!(url);
        assert!(serde_json::from_value::<DataSourceDefinition>(value).is_err());
    }
    source.jdbc_url = "jdbc:h2:mem:test".into();
    source
        .properties
        .insert("password".into(), "synthetic-value".into());
    assert!(serde_json::to_string(&source).is_err());
    source.properties.clear();
    source
        .extra
        .insert("plugin".into(), json!({"clientSecret":"synthetic-value"}));
    assert!(serde_json::to_string(&source).is_err());
    assert!(!format!("{source:?}").contains("synthetic-value"));
    assert!(!contains_secret(
        "jdbc:postgresql://host/db?applicationName=kode&sslmode=require"
    ));
    assert!(!contains_secret("jdbc:h2:file:./database;AUTO_SERVER=TRUE"));
}
