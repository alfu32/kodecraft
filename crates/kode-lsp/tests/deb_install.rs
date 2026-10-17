use kode_core::{DomainError, Result};
use kode_lsp::{
    catalog::{Catalog, Platform},
    discovery::DiscoveryOptions,
    install::{InstallJob, PackageFetcher},
};
use kode_platform::jobs::CancellationToken;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, fs, io::Write, path::Path};

struct Bytes(Vec<u8>);
impl PackageFetcher for Bytes {
    fn fetch(
        &self,
        _: &str,
        output: &mut fs::File,
        cancel: &CancellationToken,
        _: &mut dyn FnMut(u64, Option<u64>),
    ) -> Result<()> {
        if cancel.is_cancelled() {
            return Err(DomainError::Cancelled);
        }
        output.write_all(&self.0).unwrap();
        Ok(())
    }
}
fn member(output: &mut Vec<u8>, name: &str, bytes: &[u8]) {
    let header = format!(
        "{:<16}{:<12}{:<6}{:<6}{:<8}{:<10}`\n",
        format!("{name}/"),
        0,
        0,
        0,
        "100644",
        bytes.len()
    );
    assert_eq!(header.len(), 60);
    output.extend(header.as_bytes());
    output.extend(bytes);
    if !bytes.len().is_multiple_of(2) {
        output.push(b'\n');
    }
}
fn deb(data: &[u8], kind: &str) -> Vec<u8> {
    let mut output = b"!<arch>\n".to_vec();
    member(&mut output, "debian-binary", b"2.0\n");
    // The control payload is intentionally not a valid archive. It is opaque
    // metadata, never unpacked, interpreted or executed by the installer.
    member(
        &mut output,
        "control.tar.gz",
        b"MUST-NOT-EXECUTE maintainer script",
    );
    member(&mut output, kind, data);
    output
}
fn tar(link: bool, traversal: bool) -> Vec<u8> {
    let mut output = tar::Builder::new(vec![]);
    for (name, data) in [
        ("usr/lib/server/clangd", "server executable"),
        ("usr/share/doc/license", "license"),
        ("unselected/ignored", "ignored"),
    ] {
        let mut header = tar::Header::new_gnu();
        header.set_mode(0o755);
        header.set_size(data.len() as u64);
        header.set_path(name).unwrap();
        header.set_cksum();
        output.append(&header, data.as_bytes()).unwrap();
    }
    if link {
        let mut header = tar::Header::new_gnu();
        header.set_mode(0o777);
        header.set_size(0);
        header.set_entry_type(tar::EntryType::Symlink);
        header.set_link_name("../lib/server/clangd").unwrap();
        header.set_cksum();
        output
            .append_data(&mut header, "usr/bin/clangd", &[][..])
            .unwrap();
    }
    if traversal {
        let mut header = tar::Header::new_gnu();
        header.set_mode(0o644);
        header.set_size(1);
        header.as_mut_bytes()[..100].fill(0);
        header.as_mut_bytes()[..9].copy_from_slice(b"../escape");
        header.set_cksum();
        output.append(&header, &b"x"[..]).unwrap();
    }
    output.into_inner().unwrap()
}
fn job(root: &Path, bytes: &[u8], paths: Option<Vec<&str>>) -> InstallJob {
    let mut download = json!({"url":"https://example.invalid/clangd.deb", "sha256":format!("{:x}",Sha256::digest(bytes)), "command":["usr/lib/server/clangd"]});
    if let Some(paths) = paths {
        download["kodeRsExtract"] = json!({"version":1,"paths":paths});
    }
    let catalog=Catalog::parse(&serde_json::to_vec(&json!({"servers":[{"id":"fixture","name":"Fixture","versions":[{"version":"1","downloads":[download]}]}]})).unwrap()).unwrap();
    InstallJob {
        entry: catalog.servers.into_iter().next().unwrap(),
        options: DiscoveryOptions {
            platform: Platform::detect(),
            install_root: root.join("install with spaces 界"),
            search_path: vec![],
            configured_commands: BTreeMap::new(),
        },
    }
}
fn zstd(data: &[u8]) -> Vec<u8> {
    ruzstd::encoding::compress_to_vec(data, ruzstd::encoding::CompressionLevel::Fastest)
}

#[test]
fn debian_data_formats_install_selected_files_without_controls_or_unselected_aliases() {
    let data = tar(true, false);
    let mut gz = flate2::write::GzEncoder::new(vec![], flate2::Compression::default());
    gz.write_all(&data).unwrap();
    for (kind, data) in [
        ("data.tar", data.clone()),
        ("data.tar.gz", gz.finish().unwrap()),
        ("data.tar.zst", zstd(&data)),
    ] {
        let root = tempfile::tempdir().unwrap();
        let bytes = deb(&data, kind);
        let job = job(
            root.path(),
            &bytes,
            Some(vec!["./usr/lib/server", "usr/share/doc"]),
        );
        let receipt = job
            .run(&Bytes(bytes), &CancellationToken::new(), |_| {})
            .unwrap();
        assert_eq!(
            fs::read_to_string(receipt.directory.join("usr/lib/server/clangd")).unwrap(),
            "server executable"
        );
        assert!(receipt.directory.join("usr/share/doc/license").is_file());
        assert!(!receipt.directory.join("usr/bin").exists());
        assert!(!receipt.directory.join("unselected").exists());
        assert!(!receipt.directory.join("control.tar.gz").exists());
        assert!(!root.path().join("MUST-NOT-EXECUTE").exists());
        assert!(
            !job.options
                .install_root
                .join(".kode-rs-install.lock")
                .exists()
        );
    }
}
#[test]
fn corrupt_containers_compression_and_selected_links_leave_previous_install_untouched() {
    let root = tempfile::tempdir().unwrap();
    let bytes = deb(&tar(false, false), "data.tar");
    let job = job(root.path(), &bytes, None);
    let receipt = job
        .run(&Bytes(bytes.clone()), &CancellationToken::new(), |_| {})
        .unwrap();
    let before = fs::read(receipt.directory.join("installed.json")).unwrap();
    let mut truncated = bytes.clone();
    truncated.truncate(truncated.len() - 1);
    let mut duplicate = bytes.clone();
    member(&mut duplicate, "data.tar", &[]);
    let mut bad_version = bytes.clone();
    bad_version[68..72].copy_from_slice(b"1.0\n");
    let huge_window = deb(&[0x28, 0xb5, 0x2f, 0xfd, 0, 0xa8], "data.tar.zst");
    let selected_link = deb(&tar(true, false), "data.tar");
    let traversal = deb(&tar(false, true), "data.tar");
    let mut trailing = zstd(&tar(false, false));
    trailing.push(0);
    for data in [
        truncated,
        duplicate,
        bad_version,
        huge_window,
        selected_link,
        traversal,
        deb(b"broken", "data.tar.xz"),
        deb(&trailing, "data.tar.zst"),
    ] {
        let mut candidate = job.clone();
        candidate.entry.versions[0].downloads[0].sha256 =
            Some(format!("{:x}", Sha256::digest(&data)));
        assert!(
            candidate
                .run(&Bytes(data), &CancellationToken::new(), |_| {})
                .is_err()
        );
        assert_eq!(
            fs::read(receipt.directory.join("installed.json")).unwrap(),
            before
        );
        assert_eq!(
            fs::read_to_string(receipt.directory.join("usr/lib/server/clangd")).unwrap(),
            "server executable"
        );
        assert!(!root.path().join("escape").exists());
        assert!(fs::read_dir(&job.options.install_root).unwrap().all(|p| {
            !p.unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".kode-rs-lsp-stage-")
        }));
        assert!(
            !job.options
                .install_root
                .join(".kode-rs-install.lock")
                .exists()
        );
    }
}
#[test]
fn selection_never_hides_traversal_and_selected_aliases_remain_forbidden() {
    for (data, paths) in [
        (tar(false, true), vec!["usr/lib/server"]),
        (tar(true, false), vec!["usr/lib/server", "usr/bin"]),
    ] {
        let root = tempfile::tempdir().unwrap();
        let bytes = deb(&data, "data.tar");
        let job = job(root.path(), &bytes, Some(paths));
        assert!(
            job.run(&Bytes(bytes), &CancellationToken::new(), |_| {})
                .is_err()
        );
        assert!(!job.options.directory("fixture").unwrap().exists());
        assert!(!root.path().join("escape").exists());
    }
}

#[test]
fn oversized_install_manifest_cannot_publish_an_installation_that_discovery_would_reject() {
    let root = tempfile::tempdir().unwrap();
    let bytes = deb(&tar(false, false), "data.tar");
    let mut job = job(root.path(), &bytes, None);
    let receipt = job
        .run(&Bytes(bytes.clone()), &CancellationToken::new(), |_| {})
        .unwrap();
    let before = fs::read(receipt.directory.join("installed.json")).unwrap();
    job.entry.versions[0].downloads[0]
        .command
        .as_mut()
        .unwrap()
        .extend((0..5).map(|_| "x".repeat(32_000)));
    let error = job
        .run(&Bytes(bytes), &CancellationToken::new(), |_| {})
        .err()
        .unwrap();
    assert!(error.to_string().contains("128 KiB"));
    assert_eq!(
        fs::read(receipt.directory.join("installed.json")).unwrap(),
        before
    );
    assert!(
        !job.options
            .install_root
            .join(".kode-rs-install.lock")
            .exists()
    );
}

#[test]
fn catalog_validation_rejects_unsafe_or_unbounded_archive_selections() {
    let root = tempfile::tempdir().unwrap();
    let bytes = deb(&tar(false, false), "data.tar");
    let entry = job(root.path(), &bytes, None).entry;
    for plan in [
        json!({"version":1,"paths":["../escape"]}),
        json!({"version":1,"paths":["/etc"]}),
        json!({"version":1,"paths":[]}),
        json!({"version":1,"paths":vec!["path";65]}),
        json!({"version":2,"paths":["usr"]}),
        json!({"version":1,"paths":["usr"],"runScript":"forbidden"}),
    ] {
        let mut entry = entry.clone();
        entry.versions[0].downloads[0]
            .extra
            .insert(kode_lsp::package::EXTRACT_KEY.into(), plan);
        assert!(Catalog::parse(&serde_json::to_vec(&json!({"servers":[entry]})).unwrap()).is_err());
    }
    assert!(!root.path().join("install with spaces 界").exists());
}

#[test]
fn declared_aliases_copy_only_extracted_regular_files_and_never_follow_archive_links() {
    let root = tempfile::tempdir().unwrap();
    let bytes = deb(&zstd(&tar(true, false)), "data.tar.zst");
    let mut job = job(
        root.path(),
        &bytes,
        Some(vec!["usr/lib/server", "usr/share/doc"]),
    );
    job.entry.versions[0].downloads[0].extra[kode_lsp::package::EXTRACT_KEY]["copies"] =
        json!([{"source":"./usr/lib/server/clangd","target":"usr/bin/clangd"}]);
    let receipt = job
        .run(&Bytes(bytes.clone()), &CancellationToken::new(), |_| {})
        .unwrap();
    let alias = receipt.directory.join("usr/bin/clangd");
    assert!(fs::symlink_metadata(&alias).unwrap().is_file());
    assert_eq!(fs::read_to_string(alias).unwrap(), "server executable");
    assert_eq!(
        receipt.manifest.extra[kode_lsp::package::EXTRACT_KEY]["copies"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    let before = fs::read(receipt.directory.join("installed.json")).unwrap();
    for copy in [
        json!({"source":"usr/share/doc/license","target":"usr/lib/server/clangd"}),
        json!({"source":"usr/bin/clangd","target":"new-alias"}),
        json!({"source":"absent","target":"new-alias"}),
    ] {
        let mut candidate = job.clone();
        candidate.entry.versions[0].downloads[0].extra[kode_lsp::package::EXTRACT_KEY]["copies"] =
            json!([copy]);
        assert!(
            candidate
                .run(&Bytes(bytes.clone()), &CancellationToken::new(), |_| {})
                .is_err()
        );
        assert_eq!(
            fs::read(receipt.directory.join("installed.json")).unwrap(),
            before
        );
        assert!(
            !job.options
                .install_root
                .join(".kode-rs-install.lock")
                .exists()
        );
    }
}

#[test]
fn invalid_copy_paths_and_duplicate_targets_are_rejected_before_installation() {
    let root = tempfile::tempdir().unwrap();
    let bytes = deb(&tar(false, false), "data.tar");
    let baseline = job(root.path(), &bytes, Some(vec!["usr"])).entry;
    for copies in [
        json!([{"source":"../escape","target":"alias"}]),
        json!([{"source":"usr/lib/server/clangd","target":"../escape"}]),
        json!([{"source":"usr/lib/server/clangd","target":"installed.json"}]),
        json!([{"source":"./same","target":"same"}]),
        json!([{"source":"one","target":"alias"},{"source":"two","target":"./alias"}]),
        json!(vec![json!({"source":"one","target":"alias"}); 65]),
    ] {
        let mut entry = baseline.clone();
        entry.versions[0].downloads[0].extra[kode_lsp::package::EXTRACT_KEY]["copies"] = copies;
        assert!(Catalog::parse(&serde_json::to_vec(&json!({"servers":[entry]})).unwrap()).is_err());
    }
    assert!(!root.path().join("install with spaces 界").exists());
}
