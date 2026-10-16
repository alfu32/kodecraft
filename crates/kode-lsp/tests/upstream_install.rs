//! Explicit upstream qualification; never runs network installs in the ordinary suite.
use kode_lsp::{
    catalog::{Catalog, Platform},
    client::ClientState,
    discovery::DiscoveryOptions,
    install::{HttpFetcher, InstallJob, InstallReceipt},
    service::LspService,
};
use kode_platform::jobs::CancellationToken;
use serde_json::json;
use std::{collections::BTreeMap, fs, path::PathBuf, sync::Arc, time::Instant};
#[path = "support/process.rs"]
mod process;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires explicit network access, external Java 25 and Node >=22.22.2"]
async fn actual_upstream_packages_install_initialize_and_shutdown_in_isolated_roots() {
    let work = PathBuf::from(
        std::env::var_os("KODE_TEST_LSP_ROOT").expect("set an owned KODE_TEST_LSP_ROOT"),
    );
    fs::create_dir_all(&work).unwrap();
    let catalog = Catalog::compiled().unwrap();
    // Explicitly diagnose one already installed server without redownloading its
    // closure. This mode writes separate evidence and never claims a fresh install.
    let replay = std::env::var("KODE_TEST_LSP_REPLAY").ok();
    if let Some(id) = &replay {
        assert!(catalog.entry(id).is_some(), "unknown replay server");
    }
    let mut search = vec![PathBuf::from(
        std::env::var_os("KODE_TEST_NODE_BIN")
            .expect("set KODE_TEST_NODE_BIN to the external Node bin directory"),
    )];
    if let Some(java) = std::env::var_os("KODE_TEST_JAVA_BIN") {
        search.insert(0, PathBuf::from(java));
    }
    search.extend(std::env::split_paths(
        &std::env::var_os("PATH").unwrap_or_default(),
    ));
    let options = DiscoveryOptions {
        platform: Platform::detect(),
        install_root: work.join("install roots with spaces 界"),
        search_path: search,
        configured_commands: BTreeMap::new(),
    };
    assert!(
        replay.is_some() || !options.install_root.exists(),
        "qualification requires a fresh owned install root"
    );
    let fetcher = Arc::new(HttpFetcher::new(tokio::runtime::Handle::current()).unwrap());
    let mut evidence = vec![];
    let report = work.join(replay.as_ref().map_or_else(
        || "qualification.json".into(),
        |id| format!("replay-{id}.json"),
    ));
    for (id, file, text, language) in [
        (
            "typescript-language-server",
            "example.ts",
            "const selected: number = 42;\n",
            "typescript",
        ),
        (
            "vscode-langservers-extracted",
            "example.html",
            "<html><body>Hello</body></html>\n",
            "html",
        ),
        (
            "vscode-css-language-server",
            "example.css",
            "body { color: red; }\n",
            "css",
        ),
        (
            "vscode-json-language-server",
            "example.json",
            "{\"selected\":42}\n",
            "json",
        ),
        (
            "jdtls",
            "Main.java",
            "class Main { int selected = 42; }\n",
            "java",
        ),
        ("clangd", "example.c", "int selected = 42;\n", "c"),
    ] {
        if replay.as_deref().is_some_and(|selected| selected != id) {
            continue;
        }
        let entry = catalog.entry(id).unwrap().clone();
        let job = InstallJob {
            entry: entry.clone(),
            options: options.clone(),
        };
        let fetcher = fetcher.clone();
        let receipt = if replay.is_some() {
            InstallReceipt {
                manifest: options
                    .read_manifest(id)
                    .unwrap()
                    .expect("installed replay manifest"),
                directory: options.directory(id).unwrap(),
                previous: None,
            }
        } else {
            tokio::task::spawn_blocking(move || {
                job.run(fetcher.as_ref(), &CancellationToken::new(), |progress| {
                    if progress.phase == "Publishing" {
                        eprintln!("{id}: verified package ready for publication");
                    }
                })
            })
            .await
            .unwrap()
            .unwrap_or_else(|error| panic!("{id}: install: {error}"))
        };
        assert!(
            receipt.previous.is_none(),
            "qualification needs a fresh owned root"
        );
        let workspace = work.join(format!("workspace {id} 界"));
        fs::create_dir_all(&workspace).unwrap();
        let path = workspace.join(file);
        fs::write(&path, text).unwrap();
        let discovery = options.clone();
        let manifest = receipt.manifest.clone();
        let server = entry.clone();
        let root = workspace.clone();
        let command = tokio::task::spawn_blocking(move || {
            discovery.resolve_for_launch(&server, Some(&manifest), &root, &CancellationToken::new())
        })
        .await
        .unwrap()
        .unwrap();
        assert!(PathBuf::from(&command.args[0]).is_file());
        let service = LspService::new(workspace, None);
        let start = Instant::now();
        let client = service
            .start(entry, command.clone(), &CancellationToken::new())
            .await
            .unwrap_or_else(|error| panic!("{id}: initialize: {error}"));
        let startup_millis = start.elapsed().as_millis();
        assert_eq!(client.state(), ClientState::Running);
        let pid = client.process_id();
        let info = client.info().unwrap();
        let doc = kode_core::document::Document::new(
            kode_core::DocumentId(1),
            kode_core::DocumentOrigin::File(path),
            Some(language.into()),
            text,
        );
        client
            .sync_document(doc.snapshot(), false, &CancellationToken::new())
            .await
            .unwrap();
        service.shutdown().await.unwrap();
        assert_eq!(client.state(), ClientState::Stopped);
        assert!(!process::running(pid), "{id}: child was not reaped");
        evidence.push(json!({"id":id,"version":receipt.manifest.version,"platform":options.platform.os,"arch":options.platform.arch,"commandProgram":PathBuf::from(command.args[0].clone()).file_name().unwrap().to_str().unwrap(),"serverName":info.server_name,"serverVersion":info.server_version,"installed":replay.is_none(),"startupMillis":startup_millis,"clientOptions":"production LspService defaults","initialize":true,"didOpen":true,"shutdown":true,"reaped":true,"archiveSha256":receipt.manifest.extra["archiveSha256"]}));
        // A later failure retains completed cases. Only the final write marks
        // the complete selected run as passed.
        fs::write(&report, serde_json::to_vec_pretty(&json!({"status":"in-progress","mode":if replay.is_some() {"installed-replay"} else {"fresh-upstream-install"},"cases":evidence})).unwrap()).unwrap();
        eprintln!(
            "{id}: {}, initialized, opened document, shut down and reaped",
            if replay.is_some() {
                "replayed existing install"
            } else {
                "installed"
            }
        );
    }
    assert!(
        !evidence.is_empty(),
        "no reported server selected for qualification"
    );
    fs::write(
        report,
        serde_json::to_vec_pretty(&json!({"status":"passed","mode":if replay.is_some() {"installed-replay"} else {"fresh-upstream-install"},"cases":evidence})).unwrap(),
    )
    .unwrap();
}
