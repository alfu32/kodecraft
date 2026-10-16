use kode_database::{
    bridge::runtime::Installation,
    catalog::{DriverRegistry, UrlCatalog},
    config::DatabasePaths,
    credentials::{CredentialStore, Secret},
    model::DataSourceDefinition,
};
use kode_platform::{
    jobs::CancellationToken,
    process::{ProcessFactory, ProcessSpec},
};
use kode_tui::{
    app::runtime::database::{DatabaseConfiguration, Services},
    panels::database::connection_dialog::Draft,
};
use serde_json::json;
use std::{
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};
struct NoJava(AtomicUsize);
impl ProcessFactory for NoJava {
    fn spawn(&self, _: &ProcessSpec) -> kode_core::Result<tokio::process::Child> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Err(kode_core::DomainError::Unsupported(
            "No Java fixture".into(),
        ))
    }
}
fn services(root: &Path) -> (Services, Arc<NoJava>) {
    let registry = Arc::new(DriverRegistry::compiled().unwrap());
    let urls = Arc::new(UrlCatalog::compiled(&registry).unwrap());
    let factory = Arc::new(NoJava(AtomicUsize::new(0)));
    (
        Services::with_factory(
            DatabaseConfiguration {
                paths: DatabasePaths {
                    root: root.into(),
                    legacy_root: None,
                    plugin_root: root.join("plugins"),
                    legacy_plugin_root: None,
                },
                registry,
                urls,
                installation: Installation {
                    java: "absent-fixture-java".into(),
                    helper_candidates: vec![],
                },
            },
            factory.clone(),
        )
        .unwrap(),
        factory,
    )
}
fn definition(id: &str) -> DataSourceDefinition {
    serde_json::from_value(json!({"id":id,"name":id,"driver":{"id":"h2","driverClass":"org.h2.Driver"},"jdbcUrl":"jdbc:h2:mem:services","future":true})).unwrap()
}
#[tokio::test]
async fn configuration_crud_and_recents_are_lazy_native_and_private_credentials_are_separate() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("new-root");
    let (services, factory) = services(&path);
    assert!(!path.exists());
    assert!(services.sources().await.unwrap().is_empty());
    assert!(!path.exists());
    let token = CancellationToken::new();
    let saved = services
        .save(
            Draft {
                definition: definition("first"),
                password: Some(Secret::new("synthetic-old-password".into())),
            },
            &token,
        )
        .await
        .unwrap();
    let first = saved.sources[0].clone();
    let old = first.credential_reference.clone().unwrap();
    let credentials = CredentialStore::new(path.clone());
    assert_eq!(
        credentials.get(Some(&old)).unwrap().unwrap().expose(),
        "synthetic-old-password"
    );
    let public = std::fs::read_to_string(path.join("datasources-v1.json")).unwrap();
    assert!(!public.contains("synthetic-old-password"));
    assert_eq!(first.extra["future"], true);
    let saved = services
        .save(
            Draft {
                definition: first,
                password: Some(Secret::new("synthetic-new-password".into())),
            },
            &token,
        )
        .await
        .unwrap();
    let new = saved.sources[0].credential_reference.clone().unwrap();
    assert_ne!(new, old);
    assert!(credentials.get(Some(&old)).unwrap().is_none());
    assert_eq!(
        credentials.get(Some(&new)).unwrap().unwrap().expose(),
        "synthetic-new-password"
    );
    assert!(services.recents("h2").await.unwrap().is_empty());
    assert!(
        services
            .remove("first".into(), &token)
            .await
            .unwrap()
            .sources
            .is_empty()
    );
    assert!(credentials.get(Some(&new)).unwrap().is_none());
    assert_eq!(factory.0.load(Ordering::SeqCst), 0);
    services.shutdown().await.unwrap();
}
#[tokio::test]
async fn shared_credential_is_retained_until_its_last_datasource_is_removed() {
    let root = tempfile::tempdir().unwrap();
    let (services, _) = services(root.path());
    let token = CancellationToken::new();
    let saved = services
        .save(
            Draft {
                definition: definition("first"),
                password: Some(Secret::new("synthetic-shared-password".into())),
            },
            &token,
        )
        .await
        .unwrap();
    let reference = saved.sources[0].credential_reference.clone().unwrap();
    let mut second = definition("second");
    second.credential_reference = Some(reference.clone());
    services
        .save(
            Draft {
                definition: second,
                password: None,
            },
            &token,
        )
        .await
        .unwrap();
    services.remove("first".into(), &token).await.unwrap();
    let credentials = CredentialStore::new(root.path().into());
    assert!(credentials.get(Some(&reference)).unwrap().is_some());
    services.remove("second".into(), &token).await.unwrap();
    assert!(credentials.get(Some(&reference)).unwrap().is_none());
    services.shutdown().await.unwrap();
}
#[tokio::test]
async fn failed_definition_publication_does_not_replace_the_previous_password_or_leave_owned_new_reference()
 {
    let root = tempfile::tempdir().unwrap();
    let (services, _) = services(root.path());
    let token = CancellationToken::new();
    let saved = services
        .save(
            Draft {
                definition: definition("first"),
                password: Some(Secret::new("synthetic-working-password".into())),
            },
            &token,
        )
        .await
        .unwrap();
    let first = saved.sources[0].clone();
    let reference = first.credential_reference.clone().unwrap();
    let mut invalid = first.clone();
    invalid.jdbc_url = "jdbc:h2:mem:fixture;password=forbidden".into();
    assert!(
        services
            .save(
                Draft {
                    definition: invalid,
                    password: Some(Secret::new("synthetic-rejected-password".into()))
                },
                &token
            )
            .await
            .is_err()
    );
    assert_eq!(services.sources().await.unwrap(), vec![first]);
    let credentials = CredentialStore::new(root.path().into());
    assert_eq!(
        credentials.get(Some(&reference)).unwrap().unwrap().expose(),
        "synthetic-working-password"
    );
    let private = std::fs::read_to_string(root.path().join("credentials-v1.json")).unwrap();
    assert!(!private.contains("synthetic-rejected-password"));
    services.shutdown().await.unwrap();
}
