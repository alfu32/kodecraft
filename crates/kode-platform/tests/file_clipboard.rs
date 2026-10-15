use kode_core::{DocumentId, DocumentOrigin, DomainError, JobTag, RequestId, WorkspaceGeneration};
use kode_core::{
    document::{Document, encoding::decode},
    services::{Clipboard, DocumentIo},
};
use kode_platform::{
    clipboard::{ClipboardService, NativeClipboard},
    files::NativeDocumentIo,
};
use std::{
    fs,
    sync::{Arc, Mutex},
};

fn tag() -> JobTag {
    JobTag {
        workspace: WorkspaceGeneration(1),
        request: RequestId(1),
        document: None,
    }
}
#[test]
fn document_io_rejects_directories_and_fifos_before_opening_or_hashing() {
    let temp = tempfile::tempdir().unwrap();
    assert!(
        NativeDocumentIo
            .load(&DocumentOrigin::File(temp.path().into()), tag())
            .is_err()
    );
    assert!(kode_platform::files::disk_identity(temp.path()).is_err());
    #[cfg(unix)]
    {
        use nix::{sys::stat::Mode, unistd::mkfifo};
        let path = temp.path().join("fifo");
        mkfifo(&path, Mode::S_IRUSR | Mode::S_IWUSR).unwrap();
        assert!(
            NativeDocumentIo
                .load(&DocumentOrigin::File(path.clone()), tag())
                .is_err()
        );
        assert!(kode_platform::files::disk_identity(&path).is_err());
    }
}

#[test]
fn atomic_load_save_preserves_bom_crlf_and_missing_files_are_errors() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("unicode.txt");
    let bytes = b"\xef\xbb\xbfone\r\ntwo";
    fs::write(&path, bytes).unwrap();
    let origin = DocumentOrigin::File(path.clone());
    let loaded = NativeDocumentIo.load(&origin, tag()).unwrap();
    let disk = loaded.disk_identity.unwrap();
    let doc = Document::from_decoded(DocumentId(1), origin, None, decode(&loaded.bytes).unwrap());
    let identity = NativeDocumentIo
        .save(&path, &doc.snapshot().to_service_snapshot(), Some(&disk))
        .unwrap();
    assert_eq!(fs::read(&path).unwrap(), bytes);
    assert_eq!(identity.length, bytes.len() as u64);
    assert!(
        NativeDocumentIo
            .load(&DocumentOrigin::File(temp.path().join("missing")), tag())
            .is_err()
    );
    assert!(
        NativeDocumentIo
            .save(
                &temp.path().join("missing-parent/file"),
                &doc.snapshot().to_service_snapshot(),
                None
            )
            .is_err()
    );
}

#[test]
fn changed_disk_identity_prevents_overwrite_and_virtual_labels_never_become_paths() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("file");
    fs::write(&path, "original").unwrap();
    let origin = DocumentOrigin::File(path.clone());
    let loaded = NativeDocumentIo.load(&origin, tag()).unwrap();
    let doc = Document::new(DocumentId(1), origin, None, "replacement");
    fs::write(&path, "external").unwrap();
    assert!(matches!(
        NativeDocumentIo.save(
            &path,
            &doc.snapshot().to_service_snapshot(),
            loaded.disk_identity.as_ref()
        ),
        Err(DomainError::SaveConflict(_))
    ));
    assert_eq!(fs::read_to_string(&path).unwrap(), "external");
    let virtual_doc = Document::new(
        DocumentId(2),
        DocumentOrigin::Virtual {
            owner: "rest".into(),
            key: "response".into(),
            label: "response.txt".into(),
        },
        None,
        "data",
    );
    assert!(matches!(
        NativeDocumentIo.save(
            &temp.path().join("response.txt"),
            &virtual_doc.snapshot().to_service_snapshot(),
            None
        ),
        Err(DomainError::NoSaveTarget)
    ));
    assert!(!temp.path().join("response.txt").exists());
}

#[cfg(unix)]
#[test]
fn saving_through_a_symlink_preserves_the_link_target_and_permissions() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let temp = tempfile::tempdir().unwrap();
    let target = temp.path().join("target");
    let link = temp.path().join("link");
    fs::write(&target, "old").unwrap();
    fs::set_permissions(&target, fs::Permissions::from_mode(0o640)).unwrap();
    symlink(&target, &link).unwrap();
    let origin = DocumentOrigin::File(link.clone());
    let loaded = NativeDocumentIo.load(&origin, tag()).unwrap();
    let doc = Document::new(DocumentId(1), origin, None, "new");
    NativeDocumentIo
        .save(
            &link,
            &doc.snapshot().to_service_snapshot(),
            loaded.disk_identity.as_ref(),
        )
        .unwrap();
    assert!(
        fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert_eq!(fs::read_to_string(&target).unwrap(), "new");
    assert_eq!(
        fs::metadata(&target).unwrap().permissions().mode() & 0o777,
        0o640
    );
    let identity = kode_platform::files::disk_identity(&link).unwrap();
    fs::set_permissions(&target, fs::Permissions::from_mode(0o440)).unwrap();
    assert!(
        NativeDocumentIo
            .save(
                &link,
                &doc.snapshot().to_service_snapshot(),
                Some(&identity)
            )
            .is_err()
    );
    assert_eq!(fs::read_to_string(&target).unwrap(), "new");
    let other = temp.path().join("other");
    fs::write(&other, "new").unwrap();
    fs::remove_file(&link).unwrap();
    symlink(other, &link).unwrap();
    assert!(matches!(
        NativeDocumentIo.save(
            &link,
            &doc.snapshot().to_service_snapshot(),
            Some(&identity)
        ),
        Err(DomainError::SaveConflict(_))
    ));
}

struct Failing;
impl NativeClipboard for Failing {
    fn get_text(&mut self) -> std::result::Result<String, String> {
        Err("headless".into())
    }
    fn set_text(&mut self, _: &str) -> std::result::Result<(), String> {
        Err("headless".into())
    }
}
#[test]
fn unavailable_clipboard_retains_raw_unicode_text_without_terminal_queries() {
    let clipboard = ClipboardService::new(Box::new(Failing));
    clipboard.write("\t😀\nraw").unwrap();
    assert_eq!(clipboard.read().unwrap(), "\t😀\nraw");
    assert!(!clipboard.native_available());
    let internal = ClipboardService::internal();
    internal.write("shared hosts").unwrap();
    assert_eq!(internal.read().unwrap(), "shared hosts");
    let system = ClipboardService::system();
    assert!(!system.native_available());
}

struct FakeNative(Arc<Mutex<String>>);
impl NativeClipboard for FakeNative {
    fn get_text(&mut self) -> std::result::Result<String, String> {
        Ok(self.0.lock().unwrap().clone())
    }
    fn set_text(&mut self, text: &str) -> std::result::Result<(), String> {
        *self.0.lock().unwrap() = text.into();
        Ok(())
    }
}
#[test]
fn late_copy_jobs_cannot_overwrite_newer_local_copy_and_external_clipboard_is_read() {
    let native = Arc::new(Mutex::new("external".into()));
    let clipboard = ClipboardService::new(Box::new(FakeNative(native.clone())));
    assert_eq!(clipboard.read().unwrap(), "external");
    let old = clipboard.remember("A").unwrap();
    let fresh = clipboard.remember("B").unwrap();
    assert_eq!(clipboard.read().unwrap(), "B");
    assert!(!clipboard.flush_native(old));
    assert!(clipboard.flush_native(fresh));
    assert_eq!(*native.lock().unwrap(), "B");
    *native.lock().unwrap() = "outside change".into();
    assert_eq!(clipboard.read().unwrap(), "outside change");
}

#[test]
fn a_slow_native_read_cannot_replace_a_new_copy_epoch() {
    use std::sync::Barrier;
    struct Delayed {
        entered: Arc<Barrier>,
        released: Arc<Barrier>,
    }
    impl NativeClipboard for Delayed {
        fn get_text(&mut self) -> std::result::Result<String, String> {
            self.entered.wait();
            self.released.wait();
            Ok("old external".into())
        }
        fn set_text(&mut self, _: &str) -> std::result::Result<(), String> {
            Ok(())
        }
    }
    let entered = Arc::new(Barrier::new(2));
    let released = Arc::new(Barrier::new(2));
    let clipboard = Arc::new(ClipboardService::new(Box::new(Delayed {
        entered: entered.clone(),
        released: released.clone(),
    })));
    let worker_clipboard = clipboard.clone();
    let reader = std::thread::spawn(move || worker_clipboard.read().unwrap());
    entered.wait();
    clipboard.remember("new copy").unwrap();
    released.wait();
    assert_eq!(reader.join().unwrap(), "new copy");
}
