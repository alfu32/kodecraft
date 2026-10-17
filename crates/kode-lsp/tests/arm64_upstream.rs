//! Explicit ARM64 emulation qualification, separate from native hardware evidence.
#![cfg(target_os = "linux")]
use kode_lsp::{
    catalog::{Catalog, Platform},
    client::{ClientOptions, ClientState},
    discovery::DiscoveryOptions,
    install::{HttpFetcher, InstallJob},
    service::LspService,
};
use kode_platform::{
    jobs::CancellationToken,
    process::{ProcessFactory, ProcessSpec, SystemProcessFactory, run_capture},
};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, fs, path::PathBuf, sync::Arc, time::Instant};
#[path = "support/process.rs"]
mod process;

struct Emulator(PathBuf);
impl ProcessFactory for Emulator {
    fn spawn(&self, spec: &ProcessSpec) -> kode_core::Result<tokio::process::Child> {
        let mut emulated = spec.clone();
        emulated.program = self.0.clone();
        emulated
            .args
            .insert(0, spec.program.to_str().unwrap().into());
        SystemProcessFactory.spawn(&emulated)
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "explicit public downloads and private KODE_TEST_QEMU_AARCH64; not native hardware qualification"]
async fn production_catalog_arm64_clangd_installs_resolves_libraries_and_runs_lsp_under_emulation()
{
    let work = PathBuf::from(std::env::var_os("KODE_TEST_ARM64_ROOT").expect("fresh owned root"));
    fs::create_dir_all(&work).unwrap();
    let emulator = Arc::new(Emulator(PathBuf::from(
        std::env::var_os("KODE_TEST_QEMU_AARCH64").expect("private emulator executable"),
    )));
    let catalog_bytes = include_bytes!("../../../resources/lsp/servers.json");
    let catalog = Catalog::parse(catalog_bytes).unwrap();
    let entry = catalog.entry("clangd").unwrap().clone();
    let (catalog_version, selected_download) = entry
        .download(&Platform::new("linux", "arm64"))
        .expect("production catalog has a Linux ARM64 clangd download");
    assert_eq!(
        catalog_version.version,
        "23.1.4~20261014054259+0cd28174a1f6"
    );
    assert_eq!(selected_download.arch, "arm64");
    assert_eq!(
        selected_download.sha256.as_deref(),
        Some("ea1848f21c9e306f13fd854bf2233b47df60e25f1bfc31cbf5f9a654fded1ee6")
    );
    let catalog_version = catalog_version.version.clone();
    let catalog_sha256 = format!("{:x}", Sha256::digest(catalog_bytes));
    let options = DiscoveryOptions {
        platform: Platform::new("linux", "arm64"),
        install_root: work.join("isolated: install roots 界"),
        search_path: vec![],
        configured_commands: BTreeMap::new(),
    };
    assert!(
        !options.install_root.exists(),
        "requires a fresh owned root"
    );
    let job = InstallJob {
        entry: entry.clone(),
        options: options.clone(),
    };
    let fetcher = HttpFetcher::new(tokio::runtime::Handle::current()).unwrap();
    let install_emulator = emulator.clone();
    let receipt = tokio::task::spawn_blocking(move || {
        job.run_with_process_factory(
            &fetcher,
            &CancellationToken::new(),
            |progress| {
                if progress.phase == "Publishing" {
                    eprintln!("ARM64 verified closure ready for publication");
                }
            },
            install_emulator.as_ref(),
        )
    })
    .await
    .unwrap()
    .unwrap();
    let launch_options = options.clone();
    let launch_entry = entry.clone();
    let launch_manifest = receipt.manifest.clone();
    let launch_emulator = emulator.clone();
    let launch_workspace = work.join("workspace 界");
    let command = tokio::task::spawn_blocking(move || {
        launch_options.resolve_for_launch_with_process_factory(
            &launch_entry,
            Some(&launch_manifest),
            &launch_workspace,
            &CancellationToken::new(),
            launch_emulator.as_ref(),
        )
    })
    .await
    .unwrap()
    .unwrap();
    let mut list = command.process_spec();
    list.args.truncate(4);
    list.args.insert(0, "--list".into());
    let libraries = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        run_capture(list, vec![], CancellationToken::new(), emulator.as_ref()),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(libraries.status.success(), "{}", libraries.stderr);
    assert!(!libraries.truncated);
    let libraries = String::from_utf8(libraries.stdout).unwrap();
    assert!(libraries.contains("libLLVM.so.23.1"));
    for line in libraries.lines() {
        if let Some((_, target)) = line.split_once(" => ") {
            let relative = target.rsplit_once(" (0x").unwrap().0;
            let resolved = fs::canonicalize(relative).unwrap();
            assert!(
                resolved.starts_with(&receipt.directory),
                "external dependency: {line}"
            );
        }
    }
    let workspace = work.join("workspace 界");
    fs::create_dir_all(&workspace).unwrap();
    let path = workspace.join("example.c");
    let uri = kode_lsp::documents::file_uri(&path).unwrap();
    let text = "#include <stddef.h>\nsize_t selected = 42;\n";
    fs::write(&path, text).unwrap();
    let service = LspService::with_factory(ClientOptions::new(workspace), emulator, None);
    let start = Instant::now();
    let client = service
        .start(entry, command, &CancellationToken::new())
        .await
        .unwrap();
    let startup = start.elapsed().as_millis();
    assert_eq!(client.state(), ClientState::Running);
    let pid = client.process_id();
    let info = client.info().unwrap();
    let document = kode_core::document::Document::new(
        kode_core::DocumentId(1),
        kode_core::DocumentOrigin::File(path),
        Some("c".into()),
        text,
    );
    client
        .sync_document(document.snapshot(), false, &CancellationToken::new())
        .await
        .unwrap();
    // Wait for actual parsing, then resolve size_t into the bundled Clang headers.
    let mut diagnostics = vec![];
    let parsed = tokio::time::timeout(std::time::Duration::from_secs(15), async {
        loop {
            let mut ready = false;
            for event in client.take_events() {
                if event.method == "textDocument/publishDiagnostics"
                    && event.params["uri"] == uri.as_str()
                {
                    ready |= event.params["diagnostics"]
                        .as_array()
                        .is_some_and(Vec::is_empty);
                    diagnostics.push(event.params);
                }
            }
            if ready {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await;
    assert!(
        parsed.is_ok(),
        "clangd must parse the bundled stddef.h without diagnostics: {diagnostics:?}"
    );
    let definitions = client
        .definitions(
            &uri,
            lsp_types::Position::new(1, 2),
            &CancellationToken::new(),
        )
        .await
        .unwrap()
        .unwrap();
    let locations = match definitions {
        lsp_types::GotoDefinitionResponse::Scalar(location) => vec![location.uri],
        lsp_types::GotoDefinitionResponse::Array(locations) => {
            locations.into_iter().map(|location| location.uri).collect()
        }
        lsp_types::GotoDefinitionResponse::Link(locations) => locations
            .into_iter()
            .map(|location| location.target_uri)
            .collect(),
    };
    assert!(!locations.is_empty());
    let header_root = receipt
        .directory
        .join("deps/libclang-common-23-dev/usr/lib/llvm-23/lib/clang/23/include");
    for location in &locations {
        assert!(
            kode_lsp::documents::uri_path(location)
                .unwrap()
                .starts_with(&header_root),
            "header outside bundle: {location:?}"
        );
    }
    service.shutdown().await.unwrap();
    assert_eq!(client.state(), ClientState::Stopped);
    assert!(!process::running(pid), "emulator child was not reaped");
    let report = json!({"status":"passed","mode":"emulated-arm64","nativeHardwareQualified":false,"installed":true,"architecture":"arm64","catalogVersion":catalog_version,"catalogSha256":catalog_sha256,"catalogSource":"resources/lsp/servers.json","serverName":info.server_name,"serverVersion":info.server_version,"startupMillis":startup,"clientOptions":"production defaults","initialize":true,"didOpen":true,"parsedWithoutDiagnostics":true,"bundledHeaderDefinitions":locations,"shutdown":true,"reaped":true,"hostLibrariesUsed":false,"prepublicationLibraryCheck":true,"prelaunchLibraryCheck":true,"loaderList":libraries,"archiveSha256":receipt.manifest.extra["archiveSha256"]});
    fs::write(
        work.join("qualification.json"),
        serde_json::to_vec_pretty(&report).unwrap(),
    )
    .unwrap();
}
