use kode_lsp::{
    catalog::{Catalog, Platform},
    discovery::DiscoveryOptions,
    install::{HttpFetcher, InstallJob},
};
use kode_platform::jobs::CancellationToken;
use std::{collections::BTreeMap, fs, io::Read, path::PathBuf};

#[tokio::test]
#[ignore = "explicit public download; set a fresh owned KODE_TEST_DEB_ROOT; does not qualify ARM64 execution"]
async fn official_arm64_debian_archive_installs_selected_binary_without_executing_metadata() {
    let work =
        PathBuf::from(std::env::var_os("KODE_TEST_DEB_ROOT").expect("set KODE_TEST_DEB_ROOT"));
    fs::create_dir_all(&work).unwrap();
    let install = work.join("Debian selected install 界");
    assert!(
        !install.exists(),
        "requires a fresh owned installation root"
    );
    let catalog = Catalog::parse(include_bytes!(
        "../../../fixtures/lsp/clangd-arm64-deb.json"
    ))
    .unwrap();
    let entry = catalog.servers.into_iter().next().unwrap();
    let job = InstallJob {
        entry,
        options: DiscoveryOptions {
            platform: Platform::new("linux", "arm64"),
            install_root: install,
            search_path: vec![],
            configured_commands: BTreeMap::new(),
        },
    };
    let fetcher = HttpFetcher::new(tokio::runtime::Handle::current()).unwrap();
    let receipt =
        tokio::task::spawn_blocking(move || job.run(&fetcher, &CancellationToken::new(), |_| {}))
            .await
            .unwrap()
            .unwrap();
    let binary = receipt.directory.join("usr/lib/llvm-23/bin/clangd");
    let mut header = [0; 20];
    fs::File::open(&binary)
        .unwrap()
        .read_exact(&mut header)
        .unwrap();
    assert_eq!(&header[..5], b"\x7fELF\x02");
    assert_eq!(
        &header[18..20],
        &[183, 0],
        "requires the actual ARM64 ELF architecture"
    );
    assert!(
        receipt
            .directory
            .join("usr/share/doc/clangd-23/copyright")
            .is_file()
    );
    assert!(!receipt.directory.join("usr/bin/clangd-23").exists());
    assert!(
        receipt
            .manifest
            .extra
            .contains_key(kode_lsp::package::EXTRACT_KEY)
    );
    let report = serde_json::json!({"status":"archive-qualified","runtimeQualified":false,"architecture":"arm64","format":"Debian 2.0 ar / data.tar.zst","downloadSha256":receipt.manifest.extra["archiveSha256"],"binaryBytes":fs::metadata(binary).unwrap().len(),"canonicalBinary":"usr/lib/llvm-23/bin/clangd","copyrightIncluded":true,"unselectedAliasesMaterialized":false,"controlScriptsExecuted":false,"remaining":"Isolated shared-library/resource closure, native launcher and actual ARM64 LSP lifecycle"});
    fs::write(
        work.join("qualification.json"),
        serde_json::to_vec_pretty(&report).unwrap(),
    )
    .unwrap();
}
