#![cfg(unix)]
use kode_core::{DomainError, Result};
use kode_lsp::{
    catalog::{Catalog, Platform},
    discovery::DiscoveryOptions,
    install::{InstallJob, PackageFetcher},
};
use kode_platform::jobs::CancellationToken;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    io::{Cursor, Write},
    os::unix::fs::PermissionsExt,
    path::Path,
};

fn archive(files: &[(&str, &str)]) -> Vec<u8> {
    let mut writer = zip::ZipWriter::new(Cursor::new(vec![]));
    for (name, value) in files {
        writer
            .start_file(*name, zip::write::SimpleFileOptions::default())
            .unwrap();
        writer.write_all(value.as_bytes()).unwrap();
    }
    writer.finish().unwrap().into_inner()
}
#[derive(Clone)]
struct Packages(BTreeMap<String, Vec<u8>>);
impl PackageFetcher for Packages {
    fn fetch(
        &self,
        url: &str,
        output: &mut fs::File,
        cancel: &CancellationToken,
        progress: &mut dyn FnMut(u64, Option<u64>),
    ) -> Result<()> {
        if cancel.is_cancelled() {
            return Err(DomainError::Cancelled);
        }
        let bytes = self
            .0
            .get(url)
            .ok_or_else(|| DomainError::HttpTransport("503 fixture package unavailable".into()))?;
        output.write_all(bytes).unwrap();
        progress(bytes.len() as u64, Some(bytes.len() as u64));
        Ok(())
    }
}
fn program(path: &Path, body: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, format!("#!/bin/sh\n{body}\n")).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}
fn options(root: &Path) -> DiscoveryOptions {
    DiscoveryOptions {
        platform: Platform::new("linux", "x86_64"),
        install_root: root.join("isolated install 界"),
        search_path: vec![root.join("tools")],
        configured_commands: BTreeMap::new(),
    }
}
fn fixture(root: &Path) -> (InstallJob, Packages) {
    program(&root.join("tools/node"), "printf 'v22.22.2\\n'");
    let main = archive(&[
        ("package/server.js", "require('dependency');"),
        (
            "package/package.json",
            "{\"scripts\":{\"postinstall\":\"MUST-NOT-RUN\"}}",
        ),
    ]);
    let dep = archive(&[("package/index.js", "module.exports = 42;")]);
    let data = json!({"servers":[{"id":"fixture","name":"Fixture","preferredCommand":["node"],"versions":[{"version":"1","downloads":[{
        "url":"https://example.invalid/main.zip","sha256":format!("{:x}",Sha256::digest(&main)),"command":["node","package/server.js"],
        "kodeRsPackage":{"version":1,"runtime":{"kind":"node","minimum":"22.22.2"},"dependencies":[{"url":"https://example.invalid/dependency.zip","sha256":format!("{:x}",Sha256::digest(&dep)),"directory":"node_modules/dependency","unpackSubdir":"package"}]}
    }]}]}]});
    let entry = Catalog::parse(&serde_json::to_vec(&data).unwrap())
        .unwrap()
        .servers
        .remove(0);
    (
        InstallJob {
            entry,
            options: options(root),
        },
        Packages(BTreeMap::from([
            ("https://example.invalid/main.zip".into(), main),
            ("https://example.invalid/dependency.zip".into(), dep),
        ])),
    )
}
#[tokio::test]
async fn dependency_closure_is_pinned_isolated_and_published_with_runtime_manifest() {
    let root = tempfile::tempdir().unwrap();
    let (job, packages) = fixture(root.path());
    let receipt =
        tokio::task::spawn_blocking(move || job.run(&packages, &CancellationToken::new(), |_| {}))
            .await
            .unwrap()
            .unwrap();
    assert_eq!(
        fs::read_to_string(receipt.directory.join("node_modules/dependency/index.js")).unwrap(),
        "module.exports = 42;"
    );
    assert!(receipt.directory.join("package/server.js").is_file());
    assert_eq!(
        receipt.manifest.extra["kodeRsPackage"]["runtime"]["minimum"],
        "22.22.2"
    );
    assert_eq!(
        receipt.manifest.extra[kode_lsp::package::COMMAND_KEY],
        json!(receipt.manifest.command)
    );
    assert!(!root.path().join("node_modules").exists());
    assert!(!root.path().join("MUST-NOT-RUN").exists());
}
#[tokio::test]
async fn dependency_checksum_http_collision_and_cancellation_failures_keep_previous_install() {
    let root = tempfile::tempdir().unwrap();
    let (job, packages) = fixture(root.path());
    let saved = job.clone();
    let source = packages.clone();
    let receipt =
        tokio::task::spawn_blocking(move || saved.run(&source, &CancellationToken::new(), |_| {}))
            .await
            .unwrap()
            .unwrap();
    let before = fs::read(receipt.directory.join("installed.json")).unwrap();
    for mode in ["checksum", "http", "collision", "cancel"] {
        let mut job = job.clone();
        let mut packages = packages.clone();
        if mode == "checksum" {
            job.entry.versions[0].downloads[0].extra["kodeRsPackage"]["dependencies"][0]["sha256"] =
                "0".repeat(64).into();
        }
        if mode == "http" {
            packages.0.remove("https://example.invalid/dependency.zip");
        }
        if mode == "collision" {
            job.entry.versions[0].downloads[0].extra["kodeRsPackage"]["dependencies"][0]["directory"] =
                "package".into();
        }
        let result = tokio::task::spawn_blocking(move || {
            let cancel = CancellationToken::new();
            job.run(&packages, &cancel, |progress| {
                if mode == "cancel" && progress.phase == "Downloading dependencies" {
                    cancel.cancel();
                }
            })
        })
        .await
        .unwrap();
        assert!(result.is_err(), "{mode}");
        assert_eq!(
            fs::read(receipt.directory.join("installed.json")).unwrap(),
            before
        );
        assert!(
            !receipt
                .directory
                .parent()
                .unwrap()
                .join(".kode-rs-install.lock")
                .exists()
        );
        assert!(
            !fs::read_dir(receipt.directory.parent().unwrap())
                .unwrap()
                .any(|entry| entry
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".kode-rs-lsp-stage-"))
        );
    }
}
#[tokio::test]
async fn missing_or_old_runtime_fails_before_creating_install_root_and_override_remains_valid() {
    let root = tempfile::tempdir().unwrap();
    let (job, packages) = fixture(root.path());
    for present in [false, true] {
        let job = job.clone();
        let packages = packages.clone();
        if present {
            program(&root.path().join("tools/node"), "printf 'v20.19.3\\n'");
        } else {
            fs::remove_file(root.path().join("tools/node")).unwrap();
        }
        let install = job.options.install_root.clone();
        let error = tokio::task::spawn_blocking(move || {
            job.run(&packages, &CancellationToken::new(), |_| {})
        })
        .await
        .unwrap()
        .err()
        .unwrap()
        .to_string();
        assert!(error.contains("22.22.2"));
        assert!(!install.exists());
    }
    let mut options = job.options.clone();
    let custom = root.path().join("tools/custom-server");
    program(&custom, "exit 0");
    options
        .configured_commands
        .insert("fixture".into(), vec![custom.to_str().unwrap().into()]);
    let manifest: kode_lsp::catalog::InstallManifest = serde_json::from_value(json!({
        "id":"fixture","version":"1","installedAt":0,"command":["node","package/server.js"],
        "kodeRsPackage":job.entry.versions[0].downloads[0].extra["kodeRsPackage"]
    }))
    .unwrap();
    assert_eq!(
        options
            .resolve_for_launch(
                &job.entry,
                Some(&manifest),
                root.path(),
                &CancellationToken::new()
            )
            .unwrap()
            .args[0],
        custom.to_str().unwrap()
    );
    // Editing installed.json is also an explicit user override, even when the
    // previous package plan remains in its unknown-field map.
    let mut edited = manifest;
    edited
        .extra
        .insert(kode_lsp::package::COMMAND_KEY.into(), json!(edited.command));
    edited.command = Some(vec![custom.to_str().unwrap().into()]);
    options.configured_commands.clear();
    assert_eq!(
        options
            .resolve_for_launch(
                &job.entry,
                Some(&edited),
                root.path(),
                &CancellationToken::new()
            )
            .unwrap()
            .args[0],
        custom.to_str().unwrap()
    );
}
#[tokio::test]
async fn malformed_runtime_banner_produces_a_bounded_actionable_error() {
    let root = tempfile::tempdir().unwrap();
    let (job, packages) = fixture(root.path());
    program(
        &root.path().join("tools/node"),
        "printf v; /usr/bin/head -c 100000 /dev/zero | /usr/bin/tr '\\000' '9'; printf '\\n'",
    );
    let install = job.options.install_root.clone();
    let error =
        tokio::task::spawn_blocking(move || job.run(&packages, &CancellationToken::new(), |_| {}))
            .await
            .unwrap()
            .err()
            .unwrap()
            .to_string();
    assert!(error.contains("22.22.2") && error.len() < 512);
    assert!(!install.exists());
}
#[tokio::test]
async fn cancelling_version_probe_reaps_the_owned_process_group() {
    let root = tempfile::tempdir().unwrap();
    let (job, packages) = fixture(root.path());
    let pidfile = root.path().join("pid");
    program(
        &root.path().join("tools/node"),
        &format!(
            "/bin/sleep 60 &\nprintf '%s' \"$!\" > '{}'\nwait",
            pidfile.display()
        ),
    );
    let cancel = CancellationToken::new();
    let worker_cancel = cancel.clone();
    let worker = tokio::task::spawn_blocking(move || job.run(&packages, &worker_cancel, |_| {}));
    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        while !pidfile.exists() {
            tokio::time::sleep(std::time::Duration::from_millis(2)).await;
        }
    })
    .await
    .unwrap();
    let pid = fs::read_to_string(&pidfile).unwrap();
    cancel.cancel();
    assert!(matches!(worker.await.unwrap(), Err(DomainError::Cancelled)));
    #[cfg(target_os = "linux")]
    {
        // A killed grandchild may briefly be a zombie until its new parent reaps it.
        let status = fs::read_to_string(format!("/proc/{pid}/stat"));
        assert!(
            status.is_err()
                || status
                    .unwrap()
                    .split(')')
                    .nth(1)
                    .unwrap()
                    .trim_start()
                    .starts_with('Z')
        );
    }
}
#[tokio::test]
async fn jdt_launch_uses_distinct_project_data_and_copied_configuration_without_mutating_install() {
    let root = tempfile::tempdir().unwrap();
    let (mut job, mut packages) = fixture(root.path());
    program(
        &root.path().join("tools/java"),
        "printf 'openjdk version \"25.0.1\"\\n' >&2",
    );
    let archive = archive(&[
        ("plugins/org.eclipse.equinox.launcher_fixture.jar", "jar"),
        ("config_linux/config.ini", "immutable template"),
    ]);
    packages
        .0
        .insert("https://example.invalid/main.zip".into(), archive.clone());
    let download = &mut job.entry.versions[0].downloads[0];
    download.sha256 = Some(format!("{:x}", Sha256::digest(archive)));
    download.extra["kodeRsPackage"] =
        json!({"version":1,"runtime":{"kind":"java","minimum":"25.0.0"},"launcher":"jdtls"});
    let options = job.options.clone();
    let entry = job.entry.clone();
    let receipt =
        tokio::task::spawn_blocking(move || job.run(&packages, &CancellationToken::new(), |_| {}))
            .await
            .unwrap()
            .unwrap();
    let manifest = receipt.manifest.clone();
    let launch_options = options.clone();
    let launch_entry = entry.clone();
    let commands = tokio::task::spawn_blocking(move || {
        ["project one 界", "project two 界"].map(|name| {
            launch_options
                .resolve_for_launch(
                    &launch_entry,
                    Some(&manifest),
                    Path::new(name),
                    &CancellationToken::new(),
                )
                .unwrap()
        })
    })
    .await
    .unwrap();
    let data = |command: &kode_lsp::discovery::ResolvedCommand| {
        let index = command.args.iter().position(|arg| arg == "-data").unwrap();
        command.args[index + 1].clone()
    };
    assert_ne!(data(&commands[0]), data(&commands[1]));
    for command in &commands {
        let index = command
            .args
            .iter()
            .position(|arg| arg == "-configuration")
            .unwrap();
        assert_eq!(
            fs::read_to_string(Path::new(&command.args[index + 1]).join("config.ini")).unwrap(),
            "immutable template"
        );
    }
    assert_eq!(
        fs::read_to_string(receipt.directory.join("config_linux/config.ini")).unwrap(),
        "immutable template"
    );
    // An atomic package update replaces the template and archive identity. The
    // next launch must use that template rather than cached OSGi bundle versions.
    fs::write(
        receipt.directory.join("config_linux/config.ini"),
        "updated bundle versions",
    )
    .unwrap();
    let mut manifest = receipt.manifest;
    manifest
        .extra
        .insert("archiveSha256".into(), json!("new archive"));
    let upgraded = tokio::task::spawn_blocking(move || {
        options.resolve_for_launch(
            &entry,
            Some(&manifest),
            Path::new("project one 界"),
            &CancellationToken::new(),
        )
    })
    .await
    .unwrap()
    .unwrap();
    let config = |command: &kode_lsp::discovery::ResolvedCommand| {
        let index = command
            .args
            .iter()
            .position(|arg| arg == "-configuration")
            .unwrap();
        Path::new(&command.args[index + 1]).join("config.ini")
    };
    assert_eq!(data(&upgraded), data(&commands[0]));
    assert_ne!(config(&upgraded), config(&commands[0]));
    assert_eq!(
        fs::read_to_string(config(&upgraded)).unwrap(),
        "updated bundle versions"
    );
    assert_eq!(
        fs::read_to_string(config(&commands[0])).unwrap(),
        "immutable template"
    );
}
#[test]
fn malformed_dependency_plans_are_rejected_before_installation() {
    let root = tempfile::tempdir().unwrap();
    let (job, _) = fixture(root.path());
    let baseline = serde_json::to_value(&job.entry).unwrap();
    for (field, value) in [
        ("directory", json!("../escape")),
        ("directory", json!("")),
        ("sha256", json!("missing")),
        ("url", json!("https://user:secret@example.invalid/a.zip")),
    ] {
        let mut server = baseline.clone();
        server["versions"][0]["downloads"][0]["kodeRsPackage"]["dependencies"][0][field] = value;
        assert!(
            Catalog::parse(&serde_json::to_vec(&json!({"servers":[server]})).unwrap()).is_err()
        );
    }
}
