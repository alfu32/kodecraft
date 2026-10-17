#![cfg(unix)]
//! Live Oracle/MariaDB replays for the native package-source client.
use kode_database::{
    bridge::{
        client::Options as BridgeOptions,
        runtime::{Installation, Runtime},
    },
    connections::Connections,
    credentials::{CredentialStore, Secret},
    drivers::{Installer, Progress as DriverProgress},
    execution::{Executor, Options as QueryOptions, Snapshot},
    metadata::Metadata,
    model::{DataSourceDefinition, DatabaseObjectType, MetadataRequest, MetadataRequestKind},
};
use kode_platform::{jobs::CancellationToken, process::SystemProcessFactory};
use serde_json::json;
use std::path::PathBuf;

async fn execute(executor: &Executor, source: &str, sql: String) -> Result<(), String> {
    let (progress, _) = tokio::sync::watch::channel(Snapshot::default());
    let result = executor
        .execute(
            source,
            sql,
            QueryOptions::default(),
            progress,
            &CancellationToken::new(),
        )
        .await
        .map_err(|error| error.to_string())?;
    if result.completion.as_ref().is_none_or(|final_result| {
        final_result.outcome != kode_database::bridge::wire::Outcome::Completed
    }) {
        return Err("Fixture SQL did not complete".into());
    }
    Ok(())
}

fn driver_spec(vendor: &str, version: &str) -> (String, String, String) {
    match vendor {
        "oracle" => (
            "Oracle".into(),
            "oracle.jdbc.OracleDriver".into(),
            format!("com.oracle.database.jdbc:ojdbc11:{version}"),
        ),
        "mariadb" => (
            "MariaDB".into(),
            "org.mariadb.jdbc.Driver".into(),
            format!("org.mariadb.jdbc:mariadb-java-client:{version}"),
        ),
        _ => panic!("unsupported native package vendor"),
    }
}

#[tokio::test]
#[ignore = "requires owned Oracle/MariaDB fixture credentials and a built Java helper"]
async fn actual_vendor_package_source_round_trip_through_rust_metadata_client() {
    let vendor = std::env::var("KODE_TEST_PACKAGE_VENDOR").expect("vendor");
    let version = std::env::var("KODE_TEST_PACKAGE_DRIVER_VERSION").expect("driver version");
    let jdbc_url = std::env::var("KODE_TEST_PACKAGE_URL").expect("JDBC URL");
    let username = std::env::var("KODE_TEST_PACKAGE_USER").expect("test username");
    let password = std::env::var("KODE_TEST_PACKAGE_PASSWORD").expect("test password");
    let helper = PathBuf::from(std::env::var_os("KODE_TEST_JDBC_HELPER").expect("helper JAR"));
    let root = tempfile::tempdir().unwrap();
    let (display_name, driver_class, coordinates) = driver_spec(&vendor, &version);
    let source = format!("u12-live-{vendor}-{}", std::process::id());
    let definition: DataSourceDefinition = serde_json::from_value(json!({
        "id": source,
        "name": display_name,
        "driver": {
            "id": vendor,
            "displayName": display_name,
            "driverClass": driver_class,
            "coordinates": coordinates,
            "providerId": vendor
        },
        "jdbcUrl": jdbc_url,
        "username": username,
        "autoCommit": true
    }))
    .unwrap();
    let runtime = Runtime::new(
        Installation {
            java: "java".into(),
            helper_candidates: vec![helper],
        },
        BridgeOptions::default(),
        std::sync::Arc::new(SystemProcessFactory),
    );
    let installer = Installer::new(runtime);
    let (driver_progress, _) = tokio::sync::watch::channel(DriverProgress::default());
    installer
        .resolve(
            &definition.driver,
            driver_progress,
            &CancellationToken::new(),
        )
        .await
        .expect("install the selected live qualification driver");
    let connections = Connections::new(
        installer,
        CredentialStore::new(root.path().join("credentials")),
    );
    let metadata = Metadata::new(connections.clone());
    let executor = Executor::new(connections.clone());
    let token = CancellationToken::new();
    let fixture = format!("KODE_RS_RUST_{}_界_%", std::process::id());
    let mut created = false;
    let mut opened = false;
    let result: Result<(), String> = async {
        let session = connections
            .open(definition, Some(Secret::new(password)), &token)
            .await
            .map_err(|error| error.to_string())?;
        opened = true;
        let schema = session
            .schema
            .clone()
            .or_else(|| session.catalog.clone())
            .ok_or_else(|| "Connected session has no selected schema".to_owned())?;
        if vendor == "mariadb" {
            execute(&executor, &source, "SET SESSION sql_mode='ORACLE'".into()).await?;
        }
        let (specification, body) = if vendor == "oracle" {
            (
                format!("CREATE PACKAGE \"{fixture}\" AS FUNCTION calc(n NUMBER) RETURN NUMBER; FUNCTION calc(n VARCHAR2) RETURN VARCHAR2; PROCEDURE increment_value(n IN OUT NUMBER); END;"),
                format!("CREATE PACKAGE BODY \"{fixture}\" AS FUNCTION calc(n NUMBER) RETURN NUMBER IS BEGIN RETURN n + 1; END; FUNCTION calc(n VARCHAR2) RETURN VARCHAR2 IS BEGIN RETURN 'native-rust-source-界:' || n; END; PROCEDURE increment_value(n IN OUT NUMBER) IS BEGIN n := n + 1; END; END;"),
            )
        } else {
            (
                format!("CREATE PACKAGE \"{fixture}\" AS FUNCTION calc_int(n INT) RETURN INT; FUNCTION calc_text(n VARCHAR(32)) RETURN VARCHAR(32); PROCEDURE increment_value(n INOUT INT); END;"),
                format!("CREATE PACKAGE BODY \"{fixture}\" AS FUNCTION calc_int(n INT) RETURN INT AS BEGIN RETURN n + 1; END; FUNCTION calc_text(n VARCHAR(32)) RETURN VARCHAR(32) AS BEGIN RETURN 'native-rust-source-界:' || n; END; PROCEDURE increment_value(n INOUT INT) AS BEGIN SET n = n + 1; END; END;"),
            )
        };
        execute(&executor, &source, specification).await?;
        created = true;
        execute(&executor, &source, body).await?;
        let listing_request = MetadataRequest {
            kind: MetadataRequestKind::Category,
            catalog: session.catalog.clone(),
            schema: Some(schema.clone()),
            category: Some(DatabaseObjectType::Package),
            object_name: None,
            object_type: None,
            object_id: None,
        };
        let listing = metadata
            .introspect(&source, listing_request.clone(), true, &token)
            .await
            .map_err(|error| format!("package listing: {error}"))?;
        let capabilities = listing.capabilities.as_ref().ok_or("Missing package capabilities")?;
        if !capabilities.packages || !capabilities.package_source || !capabilities.package_ddl {
            return Err("Provider did not advertise live package source/DDL capabilities".into());
        }
        let package = listing
            .objects
            .iter()
            .find(|object| object.name == fixture && object.schema.as_deref() == Some(&schema))
            .ok_or_else(|| "Created package is missing from the selected schema".to_owned())?;
        let request = MetadataRequest {
            kind: MetadataRequestKind::Object,
            catalog: package.catalog.clone(),
            schema: package.schema.clone(),
            category: None,
            object_name: Some(package.name.clone()),
            object_type: Some(DatabaseObjectType::Package),
            object_id: Some(package.id.clone()),
        };
        let members = metadata
            .introspect(&source, request.clone(), false, &token)
            .await
            .map_err(|error| format!("package members: {error}"))?;
        if members.objects.len() != 3 {
            return Err(format!("Expected three public package routines, got {}", members.objects.len()));
        }
        let package_source = metadata
            .source(&source, request.clone(), &token)
            .await
            .map_err(|error| format!("package source: {error}"))?;
        if !metadata.source_is_current(&source, &request, &package_source) {
            return Err("Package source did not retain its connection/refresh proof".into());
        }
        let documents = &package_source.result.documents;
        if documents.len() != 2 || documents.iter().any(|document| document.text.trim().is_empty()) {
            return Err("Expected complete nonempty package specification and body".into());
        }
        let source_text = documents.iter().map(|document| document.text.as_str()).collect::<Vec<_>>().join("\n");
        if !source_text.contains("native-rust-source-界:") || !source_text.contains("RETURN n + 1") {
            return Err("Rust source client did not receive the actual package body".into());
        }
        if documents.iter().any(|document| document.kind != kode_database::model::MetadataSourceKind::Ddl) {
            return Err("Oracle/MariaDB package source documents must be DDL".into());
        }
        execute(&executor, &source, format!("DROP PACKAGE \"{fixture}\"")).await?;
        created = false;
        metadata.refresh_all(&source);
        let refreshed = metadata
            .introspect(&source, listing_request, true, &token)
            .await
            .map_err(|error| format!("refreshed package listing: {error}"))?;
        if refreshed
            .objects
            .iter()
            .any(|object| object.name == fixture && object.schema.as_deref() == Some(&schema))
        {
            return Err("Dropped package remained in the refreshed metadata listing".into());
        }
        Ok(())
    }
    .await;
    let cleanup = if created {
        execute(&executor, &source, format!("DROP PACKAGE \"{fixture}\"")).await
    } else {
        Ok(())
    };
    if opened {
        let _ = connections.close(&source, &token).await;
    }
    metadata.shutdown().await;
    connections.shutdown().await.unwrap();
    cleanup.unwrap();
    result.unwrap();
}
