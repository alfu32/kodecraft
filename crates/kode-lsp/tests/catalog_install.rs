use kode_core::{DomainError, Result};
use kode_lsp::{catalog::*, discovery::*, install::*};
use kode_platform::jobs::CancellationToken;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    io::{Cursor, Read, Write},
    path::Path,
    sync::Arc,
    time::Duration,
};
fn entry(url: &str, command: &[&str]) -> ServerEntry {
    Catalog::parse(&serde_json::to_vec(&serde_json::json!({"servers":[{"id":"test-server","name":"Test Server","languages":["rust"],"preferredCommand":command,"versions":[{"version":"2","downloads":[{"url":url}]}]}]})).unwrap()).unwrap().servers.remove(0)
}
fn options(root: &Path) -> DiscoveryOptions {
    DiscoveryOptions {
        platform: Platform::new("linux", "amd64"),
        install_root: root.join("install root"),
        search_path: vec![],
        configured_commands: BTreeMap::new(),
    }
}

#[test]
fn compiled_clangd_catalog_selects_the_pinned_arm64_closure_without_changing_other_targets() {
    let catalog = Catalog::compiled().unwrap();
    let clangd = catalog.entry("clangd").unwrap();

    let (arm_version, arm_download) = clangd
        .download(&Platform::new("linux", "aarch64"))
        .expect("Linux ARM64 must select its pinned clangd closure");
    assert_eq!(arm_version.version, "23.1.4~20261014054259+0cd28174a1f6");
    assert_eq!(arm_download.arch, "arm64");
    assert_eq!(
        arm_download.sha256.as_deref(),
        Some("ea1848f21c9e306f13fd854bf2233b47df60e25f1bfc31cbf5f9a654fded1ee6")
    );
    assert!(arm_download.extra.contains_key("kodeRsPackage"));

    for (platform, expected_version) in [
        (Platform::new("linux", "x86_64"), "23.1.0"),
        (Platform::new("windows", "x86_64"), "23.1.0"),
        (Platform::new("macos", "arm64"), "23.1.0"),
    ] {
        let (version, _) = clangd
            .download(&platform)
            .unwrap_or_else(|| panic!("missing clangd asset for {platform:?}"));
        assert_eq!(version.version, expected_version, "{platform:?}");
    }
}

fn executable(path: &Path) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, b"#!/bin/sh\nexit 0\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
    }
}
struct Bytes(Vec<u8>);
impl PackageFetcher for Bytes {
    fn fetch(
        &self,
        _: &str,
        file: &mut fs::File,
        cancel: &CancellationToken,
        progress: &mut dyn FnMut(u64, Option<u64>),
    ) -> Result<()> {
        if cancel.is_cancelled() {
            return Err(DomainError::Cancelled);
        }
        file.write_all(&self.0).unwrap();
        progress(self.0.len() as u64, Some(self.0.len() as u64));
        Ok(())
    }
}
fn zip(files: &[(&str, &[u8])]) -> Vec<u8> {
    let mut writer = zip::ZipWriter::new(Cursor::new(vec![]));
    for (name, bytes) in files {
        writer
            .start_file(
                *name,
                zip::write::SimpleFileOptions::default().unix_permissions(0o644),
            )
            .unwrap();
        writer.write_all(bytes).unwrap();
    }
    writer.finish().unwrap().into_inner()
}
fn gzip(bytes: &[u8]) -> Vec<u8> {
    let mut writer = flate2::write::GzEncoder::new(vec![], flate2::Compression::default());
    writer.write_all(bytes).unwrap();
    writer.finish().unwrap()
}
fn tar() -> Vec<u8> {
    let mut builder = tar::Builder::new(vec![]);
    let bytes = b"#!/bin/sh\nexit 0\n";
    let mut header = tar::Header::new_gnu();
    header.set_size(bytes.len() as u64);
    header.set_mode(0o644);
    header.set_cksum();
    builder
        .append_data(&mut header, "bin/server", &bytes[..])
        .unwrap();
    builder.into_inner().unwrap()
}

#[test]
fn frozen_catalog_unknown_fields_platform_aliases_and_preference_order_are_preserved() {
    let catalog = Catalog::compiled().unwrap();
    assert_eq!(catalog.servers.len(), 11);
    assert_eq!(
        catalog.for_language("TYPEscript").next().unwrap().id,
        "typescript-language-server"
    );
    let download = catalog
        .entry("typescript-language-server")
        .unwrap()
        .download(&Platform::detect())
        .unwrap()
        .1;
    assert!(download.sha256.is_some());
    assert_eq!(
        kode_lsp::package::plan(&download.extra)
            .unwrap()
            .unwrap()
            .runtime
            .unwrap()
            .minimum,
        "22.22.2"
    );
    for (language, id) in [
        ("html", "vscode-langservers-extracted"),
        ("css", "vscode-css-language-server"),
        ("json", "vscode-json-language-server"),
    ] {
        assert_eq!(catalog.for_language(language).next().unwrap().id, id);
        assert!(
            catalog
                .entry(id)
                .unwrap()
                .download(&Platform::detect())
                .is_some()
        );
    }
    for (os, arch, expected) in [
        ("GNU/Linux", "AMD64", ("linux", "x86_64")),
        ("Darwin", "aarch64", ("macos", "arm64")),
        ("Mac OS X", "x64", ("macos", "x86_64")),
        ("Windows 11", "i686", ("windows", "x86")),
    ] {
        let platform = Platform::new(os, arch);
        assert_eq!((&*platform.os, &*platform.arch), expected);
        assert!(platform.matches(expected.0, expected.1));
        assert!(platform.matches("any", "any"));
    }
    for (os, arch) in [
        ("linux", "x86_64"),
        ("macos", "arm64"),
        ("windows", "x86_64"),
    ] {
        assert!(
            catalog
                .entry("rust-analyzer")
                .unwrap()
                .download(&Platform::new(os, arch))
                .is_some()
        );
    }
    let mut original = serde_json::to_value(&catalog).unwrap();
    original["future"] = "kept".into();
    original["servers"][0]["extra"] = "field".into();
    assert_eq!(
        serde_json::to_value(Catalog::parse(&serde_json::to_vec(&original).unwrap()).unwrap())
            .unwrap(),
        original
    );
    for id in ["..", "../other", "C:\\escape", "bad/id", ""] {
        let mut value = original.clone();
        value["servers"][0]["id"] = id.into();
        assert!(Catalog::parse(&serde_json::to_vec(&value).unwrap()).is_err());
    }
    let mut server = entry("https://example.invalid/server.zip", &["server"]);
    server.versions.insert(
        0,
        ServerVersion {
            version: "new but unsupported".into(),
            downloads: vec![],
            install_instructions: Extra::new(),
            extra: Extra::new(),
        },
    );
    assert_eq!(server.download(&Platform::detect()).unwrap().0.version, "2");
}

#[test]
fn catalog_resource_priority_and_bounds_reject_bad_paths_hashes_and_command_arrays() {
    use kode_platform::paths::ResourcePaths;
    let root = tempfile::tempdir().unwrap();
    let rust = root.path().join("rust");
    let legacy = root.path().join("legacy");
    fs::create_dir_all(rust.join("lsp")).unwrap();
    fs::create_dir_all(legacy.join("lsp")).unwrap();
    let mut catalog = Catalog::compiled().unwrap();
    catalog.extra.insert("origin".into(), "legacy".into());
    fs::write(
        legacy.join("lsp/servers.json"),
        serde_json::to_vec(&catalog).unwrap(),
    )
    .unwrap();
    let mut paths = ResourcePaths {
        explicit: None,
        rust_home: Some(rust.clone()),
        legacy_home: Some(legacy),
        executable_dir: root.path().join("executable"),
        config_dir: root.path().join("config"),
    };
    assert!(
        Catalog::load(&paths, None).unwrap().1.is_none(),
        "explicit Rust resource root excludes the legacy root"
    );
    catalog.extra.insert("origin".into(), "rust".into());
    fs::write(
        rust.join("lsp/servers.json"),
        serde_json::to_vec(&catalog).unwrap(),
    )
    .unwrap();
    assert_eq!(
        Catalog::load(&paths, None).unwrap().0.extra["origin"],
        "rust"
    );
    let explicit = root.path().join("custom.json");
    catalog.extra.insert("origin".into(), "explicit".into());
    fs::write(&explicit, serde_json::to_vec(&catalog).unwrap()).unwrap();
    assert_eq!(
        Catalog::load(&paths, Some(&explicit)).unwrap().0.extra["origin"],
        "explicit"
    );
    fs::write(&explicit, vec![b' '; MAX_CATALOG_BYTES + 1]).unwrap();
    assert!(Catalog::load(&paths, Some(&explicit)).is_err());
    fs::remove_file(rust.join("lsp/servers.json")).unwrap();
    paths.rust_home = None;
    assert_eq!(
        Catalog::load(&paths, None).unwrap().0.extra["origin"],
        "legacy"
    );
    let mut value =
        serde_json::to_value(entry("https://example.invalid/server.zip", &["server"])).unwrap();
    for (field, invalid_value) in [
        ("sha256", serde_json::json!("short")),
        ("unpackSubdir", serde_json::json!("../outside")),
        ("command", serde_json::json!([])),
        (
            "url",
            serde_json::json!("https://user:password@example.invalid/server.zip"),
        ),
    ] {
        let old = value["versions"][0]["downloads"][0][field].clone();
        value["versions"][0]["downloads"][0][field] = invalid_value;
        assert!(
            Catalog::parse(
                &serde_json::to_vec(&serde_json::json!({"servers":[value.clone()]})).unwrap()
            )
            .is_err(),
            "{field}"
        );
        value["versions"][0]["downloads"][0][field] = old;
    }
    for path in [
        "../outside",
        "/absolute",
        "C:/escape",
        "..\\escape",
        "directory/../../escape",
    ] {
        assert!(relative_path(path).is_err(), "{path}");
    }
    assert!(relative_path("./bin/server").is_ok());
    assert!(relative_path(".").is_ok());
}

#[test]
fn discovery_finds_existing_manifests_manual_local_path_and_windows_commands_without_download_or_mutation()
 {
    let root = tempfile::tempdir().unwrap();
    let mut options = options(root.path());
    let server = entry(
        "https://example.invalid/server.gz",
        &["server", "--stdio", "an argument with spaces", ""],
    );
    let before = options.status(&server, false);
    assert_eq!(before.install_state, InstallState::Missing);
    assert!(before.command.is_none());
    assert!(!options.install_root.exists());
    options.search_path.push(root.path().join("path tools"));
    executable(&options.search_path[0].join("server"));
    let status = options.status(&server, false);
    assert_eq!(status.install_state, InstallState::Installed);
    assert_eq!(
        status.command.unwrap().args[1..],
        server.preferred_command.as_ref().unwrap()[1..]
    );
    assert!(!options.install_root.exists());
    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;
        let command = options.search_path[0].join("server");
        fs::rename(&command, options.search_path[0].join("multiplexer")).unwrap();
        symlink("multiplexer", &command).unwrap();
        assert!(
            options.status(&server, false).command.unwrap().args[0].ends_with("/server"),
            "discovery must retain a symlink's invocation name"
        );
    }
    executable(&options.install_root.join("test-server/server"));
    assert_eq!(
        options.status(&server, false).install_state,
        InstallState::Manual
    );
    let manifest = InstallManifest {
        id: server.id.clone(),
        version: "1".into(),
        installed_at: 1,
        source_url: None,
        platform_os: None,
        platform_arch: None,
        command: Some(vec!["server".into(), "--manifest-argument".into()]),
        unpack_subdir: None,
        extra: Extra::new(),
    };
    let path = options.install_root.join("test-server/installed.json");
    fs::write(&path, serde_json::to_vec(&manifest).unwrap()).unwrap();
    let status = options.status(&server, true);
    assert_eq!(status.install_state, InstallState::UpdateAvailable);
    assert!(status.running);
    assert_eq!(status.command.unwrap().args[1], "--manifest-argument");
    options.configured_commands.insert(
        server.id.clone(),
        vec![
            options.search_path[0]
                .join("server")
                .to_str()
                .unwrap()
                .into(),
            "--configured".into(),
        ],
    );
    assert_eq!(
        options.status(&server, false).command.unwrap().args[1],
        "--configured"
    );
    fs::write(&path, "{broken").unwrap();
    assert_eq!(
        options.status(&server, false).install_state,
        InstallState::Error
    );
    let mut windows = options.clone();
    windows.configured_commands.clear();
    windows.platform = Platform::new("windows", "x86_64");
    let external = root.path().join("path tools/windows-server.cmd");
    fs::write(&external, b"@exit /b 0").unwrap();
    let mut win_entry = server.clone();
    win_entry.id = "windows-server".into();
    win_entry.preferred_command = Some(vec!["windows-server".into(), "--stdio".into()]);
    assert!(
        windows.resolve_command(&win_entry, None).unwrap().args[0].ends_with("windows-server.cmd")
    );
    let mut manual = server;
    manual.id = "manual".into();
    manual.preferred_command = Some(vec!["missing-manual-server".into()]);
    manual.versions[0].downloads.clear();
    assert_eq!(
        options.status(&manual, false).install_state,
        InstallState::Manual
    );
}

#[test]
fn zip_tar_tgz_gzip_and_raw_installations_publish_working_commands_and_preserve_all_arguments() {
    for (name, bytes, command) in [
        (
            "server.zip",
            zip(&[("pkg/bin/server", b"#!/bin/sh\nexit 0\n")]),
            vec!["bin/server", "--stdio", "two words", ""],
        ),
        ("server.tar", tar(), vec!["server", "--stdio"]),
        ("server.tgz", gzip(&tar()), vec!["server", "--stdio"]),
        (
            "arch-specific-name.gz",
            gzip(b"#!/bin/sh\nexit 0\n"),
            vec!["server", "--stdio"],
        ),
        (
            "server",
            b"#!/bin/sh\nexit 0\n".to_vec(),
            vec!["server", "--stdio"],
        ),
    ] {
        let root = tempfile::tempdir().unwrap();
        let options = options(root.path());
        let mut server = entry(
            &format!("https://example.invalid/{name}?download=1"),
            &command,
        );
        if name.ends_with(".zip") {
            server.versions[0].downloads[0].unpack_subdir = Some("pkg".into());
        }
        server.versions[0].downloads[0].sha256 = Some(format!("{:X}", Sha256::digest(&bytes)));
        let mut phases = vec![];
        let receipt = InstallJob {
            entry: server.clone(),
            options: options.clone(),
        }
        .run(&Bytes(bytes), &CancellationToken::new(), |p| {
            phases.push(p.phase)
        })
        .unwrap();
        assert!(receipt.previous.is_none());
        assert!(
            phases.contains(&"Downloading")
                && phases.contains(&"Extracting")
                && phases.contains(&"Installed")
        );
        let status = options.status(&server, false);
        assert_eq!(
            status.install_state,
            InstallState::Installed,
            "{name}: {:?}",
            status.message
        );
        let command = status.command.unwrap();
        assert_eq!(
            command.args[1..],
            server.preferred_command.unwrap()[1..],
            "{name}"
        );
        assert!(Path::new(&command.args[0]).is_file());
        assert!(!status.running);
        if name.ends_with(".zip") {
            assert!(command.cwd.unwrap().ends_with("test-server/pkg"));
        }
        assert!(!options.install_root.join(".kode-rs-install.lock").exists());
        assert!(!fs::read_dir(&options.install_root).unwrap().any(|entry| {
            entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".kode-rs-lsp-stage-")
        }));
    }
}

#[test]
fn checksum_extraction_cancellation_and_invalid_command_failures_keep_previous_install_and_clean_staging()
 {
    let root = tempfile::tempdir().unwrap();
    let options = options(root.path());
    let mut server = entry("https://example.invalid/server.zip", &["server"]);
    let job = InstallJob {
        entry: server.clone(),
        options: options.clone(),
    };
    job.run(
        &Bytes(zip(&[("server", b"old bytes")])),
        &CancellationToken::new(),
        |_| {},
    )
    .unwrap();
    let old = fs::read(options.install_root.join("test-server/server")).unwrap();
    for mode in ["hash", "traversal", "cancel", "missing-command", "bad-tar"] {
        let mut bytes = zip(&[("server", b"replacement bytes")]);
        let cancel = CancellationToken::new();
        let mut entry = server.clone();
        if mode == "hash" {
            entry.versions[0].downloads[0].sha256 = Some("0".repeat(64));
        }
        if mode == "traversal" {
            bytes = zip(&[("../escape", b"escape")]);
        }
        if mode == "missing-command" {
            bytes = zip(&[("unrelated", b"not a server")]);
        }
        if mode == "bad-tar" {
            entry.versions[0].downloads[0].url = "https://example.invalid/server.tar".into();
            bytes = vec![0x42; 512];
        }
        let result = InstallJob {
            entry,
            options: options.clone(),
        }
        .run(&Bytes(bytes), &cancel, |p| {
            if mode == "cancel" && p.phase == "Extracting" && p.completed > 0 {
                cancel.cancel();
            }
        });
        assert!(result.is_err(), "{mode}");
        assert_eq!(
            fs::read(options.install_root.join("test-server/server")).unwrap(),
            old,
            "{mode}"
        );
        assert!(!root.path().join("escape").exists());
        assert!(!options.install_root.join("escape").exists());
        assert!(!options.install_root.join(".kode-rs-install.lock").exists());
        assert!(!fs::read_dir(&options.install_root).unwrap().any(|e| {
            e.unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".kode-rs-lsp-stage-")
        }));
    }
    fs::write(options.install_root.join("unrelated-user-file"), b"keep").unwrap();
    server.versions[0].version = "3".into();
    let receipt = InstallJob {
        entry: server,
        options: options.clone(),
    }
    .run(
        &Bytes(zip(&[("server", b"new bytes")])),
        &CancellationToken::new(),
        |_| {},
    )
    .unwrap();
    assert_eq!(
        fs::read(receipt.previous.unwrap().join("server")).unwrap(),
        old
    );
    assert!(uninstall(&options, "test-server", &CancellationToken::new()).unwrap());
    assert!(!uninstall(&options, "test-server", &CancellationToken::new()).unwrap());
    assert_eq!(
        fs::read(options.install_root.join("unrelated-user-file")).unwrap(),
        b"keep"
    );
    assert!(options.install_root.is_dir());
}

#[cfg(unix)]
#[test]
fn symlink_install_targets_and_archive_links_cannot_touch_another_directory() {
    use std::os::unix::fs::symlink;
    let root = tempfile::tempdir().unwrap();
    let options = options(root.path());
    let outside = root.path().join("outside");
    fs::create_dir(&outside).unwrap();
    fs::write(outside.join("keep"), b"keep").unwrap();
    fs::create_dir_all(&options.install_root).unwrap();
    symlink(&outside, options.install_root.join("test-server")).unwrap();
    let server = entry("https://example.invalid/server.zip", &["server"]);
    assert!(
        InstallJob {
            entry: server.clone(),
            options: options.clone()
        }
        .run(
            &Bytes(zip(&[("server", b"bad")])),
            &CancellationToken::new(),
            |_| {}
        )
        .is_err()
    );
    assert!(uninstall(&options, "test-server", &CancellationToken::new()).is_err());
    assert_eq!(
        options.status(&server, false).install_state,
        InstallState::Error
    );
    assert_eq!(fs::read(outside.join("keep")).unwrap(), b"keep");
    fs::remove_file(options.install_root.join("test-server")).unwrap();
    let mut builder = tar::Builder::new(vec![]);
    let mut header = tar::Header::new_gnu();
    header.set_entry_type(tar::EntryType::Symlink);
    header.set_size(0);
    header.set_mode(0o777);
    header.set_link_name("../../outside").unwrap();
    header.set_cksum();
    builder.append_data(&mut header, "link", &[][..]).unwrap();
    let mut server = server;
    server.versions[0].downloads[0].url = "https://example.invalid/server.tar".into();
    assert!(
        InstallJob {
            entry: server,
            options: options.clone()
        }
        .run(
            &Bytes(builder.into_inner().unwrap()),
            &CancellationToken::new(),
            |_| {}
        )
        .is_err()
    );
    assert_eq!(fs::read(outside.join("keep")).unwrap(), b"keep");
    assert!(!options.install_root.join("test-server").exists());
}

#[tokio::test]
async fn actual_http_fetcher_streams_http_errors_and_cancellation_without_leaking_a_child_or_private_runtime()
 {
    for mode in ["success", "http-error", "cancel", "too-large"] {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            let mut buffer = [0; 4096];
            let _ = stream.read(&mut buffer);
            let response = match mode {
                "http-error" => {
                    "HTTP/1.1 503 Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                        .to_owned()
                }
                "too-large" => format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    MAX_DOWNLOAD_BYTES + 1
                ),
                "cancel" => {
                    "HTTP/1.1 200 OK\r\nContent-Length: 1000000\r\nConnection: close\r\n\r\npartial"
                        .to_owned()
                }
                _ => "HTTP/1.1 200 OK\r\nContent-Length: 7\r\nConnection: close\r\n\r\npayload"
                    .to_owned(),
            };
            let _ = stream.write_all(response.as_bytes());
            if mode == "cancel" {
                let _ = stream.read(&mut buffer);
            }
        });
        let fetcher = Arc::new(HttpFetcher::new(tokio::runtime::Handle::current()).unwrap());
        let cancel = CancellationToken::new();
        let worker_cancel = cancel.clone();
        let result = tokio::task::spawn_blocking(move || {
            let mut file = tempfile::tempfile().unwrap();
            let result = fetcher.fetch(
                &format!("http://{address}/server"),
                &mut file,
                &worker_cancel,
                &mut |_, _| {
                    if mode == "cancel" {
                        worker_cancel.cancel();
                    }
                },
            );
            (result, file)
        })
        .await
        .unwrap();
        server.join().unwrap();
        if mode == "success" {
            assert!(result.0.is_ok());
            assert_eq!(result.1.metadata().unwrap().len(), 7);
        } else if mode == "cancel" {
            assert!(matches!(result.0, Err(DomainError::Cancelled)));
        } else {
            assert!(result.0.is_err());
        }
    }
}
