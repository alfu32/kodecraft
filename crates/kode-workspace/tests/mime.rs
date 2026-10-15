use kode_core::{DomainError, JobTag, RequestId, WorkspaceGeneration, document::encoding::decode};
use kode_platform::jobs::{CancellationToken, MonotonicClock, Notifications, OwnerId, Scheduler};
use kode_workspace::mime::*;
use serde::Deserialize;
use std::{
    fs,
    io::{self, Cursor, Read},
    sync::Arc,
};

#[test]
fn original_detector_tests_filename_signature_file_fallback_override_and_bash_shebang() {
    let mut detector = MimeDetector::compiled();
    let json = detector.detect_filename("example.json");
    assert_eq!(json.mime, "text/json");
    assert_eq!(json.extension, ".json");
    assert_eq!(json.source, DetectionSource::Extension);
    let png = detector.detect(b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR");
    assert_eq!(png.mime, "image/png");
    assert_eq!(png.extension, ".png");
    assert_eq!(png.source, DetectionSource::Signature);
    let directory = tempfile::tempdir().unwrap();
    let pdf = directory.path().join("misleading.bin");
    fs::write(&pdf, b"%PDF-1.7\n").unwrap();
    assert_eq!(
        detector
            .detect_file(&pdf, &CancellationToken::new())
            .unwrap()
            .mime,
        "application/pdf"
    );
    detector
        .register_extension(".foo", "application/x-foo")
        .unwrap();
    let custom = detector.detect_filename("sample.FOO");
    assert_eq!(custom.mime, "application/x-foo");
    assert_eq!(custom.extension, ".foo");
    assert_eq!(custom.source, DetectionSource::Extension);
    let fallback = detector.detect(b"hello world\nthis is text\n");
    assert_eq!(fallback.mime, "text/plain");
    assert_eq!(fallback.extension, ".txt");
    assert_eq!(fallback.source, DetectionSource::Fallback);
    let unknown = directory.path().join("sample.weirdtxt");
    fs::write(&unknown, b"plain text body\nwith multiple lines\n").unwrap();
    assert_eq!(
        detector
            .detect_file(&unknown, &CancellationToken::new())
            .unwrap()
            .category,
        MimeCategory::Text
    );
    let bash = detector.detect(b"#!/usr/bin/env bash -e\necho hello\n");
    assert_eq!(bash.mime, "text/shellscript");
    assert_eq!(bash.language.as_deref(), Some("shellscript"));
    assert_eq!(bash.source, DetectionSource::Shebang);
    let script = directory.path().join("no-extension");
    fs::write(&script, b"#!/bin/sh\nprintf ok\n").unwrap();
    assert_eq!(
        detector
            .detect_file(&script, &CancellationToken::new())
            .unwrap()
            .source,
        DetectionSource::Shebang
    );
}
#[derive(Deserialize)]
struct Matrix {
    samples: Vec<Sample>,
}
#[derive(Deserialize)]
struct Sample {
    filename: String,
    mime: String,
    extension: String,
    language: Option<String>,
    category: MimeCategory,
}
#[test]
fn complete_source_sample_filename_matrix_and_all_curated_rows_preserve_last_wins_semantics() {
    let detector = MimeDetector::compiled();
    let matrix: Matrix =
        serde_json::from_slice(include_bytes!("../../../fixtures/mime/sample-matrix.json"))
            .unwrap();
    assert_eq!(matrix.samples.len(), 225);
    for row in matrix.samples {
        let result = detector.detect_filename(&row.filename);
        assert_eq!(
            (
                result.mime,
                result.extension,
                result.language,
                result.category
            ),
            (row.mime, row.extension, row.language, row.category),
            "{}",
            row.filename
        );
    }
    // This independently checks duplicate resolution and dotless media entries.
    let table: Vec<MimeEntry> =
        serde_json::from_slice(include_bytes!("../../../resources/mime-types.json")).unwrap();
    assert_eq!(table.len(), 281);
    let mut effective = std::collections::BTreeMap::new();
    for row in table {
        effective.insert(
            row.extension.trim_start_matches('.').to_ascii_lowercase(),
            row,
        );
    }
    for (extension, row) in effective {
        if extension.contains('.') {
            continue;
        }
        let result = detector.detect_filename(&format!("sample.{extension}"));
        assert_eq!(result.mime, row.mime);
        assert_eq!(result.language, Some(row.language));
    }
    for extension in [
        "service",
        "socket",
        "target",
        "timer",
        "path",
        "mount",
        "automount",
        "swap",
        "slice",
        "scope",
        "busname",
        "container",
        "volume",
        "network",
        "image",
        "build",
        "pod",
        "kube",
        "artifact",
        "conf",
    ] {
        assert_eq!(
            detector
                .detect_filename(&format!("unit.{extension}"))
                .language
                .as_deref(),
            Some("systemd")
        );
    }
    for (extension, language) in [
        ("cp", "cplus"),
        ("c", "c"),
        ("h", "c"),
        ("hh", "cpp"),
        ("hpp", "cpp"),
        ("hxx", "cpp"),
    ] {
        assert_eq!(
            detector
                .detect_filename(&format!("code.{extension}"))
                .language
                .as_deref(),
            Some(language)
        );
    }
}
#[test]
fn precedence_shebang_known_filename_extension_signatures_deterministic_guess_and_text() {
    let detector = MimeDetector::compiled();
    for shebang in [
        "#!/bin/bash\n",
        "#!/usr/bin/env bash -e\n",
        "\u{feff}#!/usr/bin/env -S X=1 /bin/zsh -e\r\n",
        "#!/usr/bin/env -u ENV fish\n",
    ] {
        let found = detector
            .detect_reader("misleading.json", &mut Cursor::new(shebang.as_bytes()))
            .unwrap();
        // Preserve the source's token skipping: arguments to env options are not
        // interpreted as shell commands. ENV here is deliberately not a shell.
        if shebang.contains("-u ENV") {
            assert_eq!(found.source, DetectionSource::Extension);
        } else {
            assert_eq!(found.source, DetectionSource::Shebang);
        }
    }
    for shell in [
        "ash", "bash", "csh", "dash", "fish", "ksh", "nu", "nushell", "sh", "tcsh", "zsh",
    ] {
        assert_eq!(
            detector
                .detect(format!("#!/bin/{shell}\n").as_bytes())
                .source,
            DetectionSource::Shebang
        );
    }
    assert_eq!(
        detector.detect(b"#!/bin/python3\nprint(1)\n").source,
        DetectionSource::Fallback
    );
    assert_eq!(
        detector
            .detect_reader("image.json", &mut Cursor::new(b"\x89PNG\r\n\x1a\n"))
            .unwrap()
            .mime,
        "text/json"
    );
    for (name, language) in [
        ("Dockerfile", "docker"),
        ("Makefile", "make"),
        ("GNUmakefile", "make"),
        ("CMakeLists.txt", "cmake"),
        ("CODEOWNERS", "codeowners"),
        (".env", "dotenv"),
    ] {
        let result = detector
            .detect_reader(name, &mut Cursor::new(b"text"))
            .unwrap();
        assert_eq!(result.source, DetectionSource::Filename, "{name}");
        assert_eq!(result.language.as_deref(), Some(language));
    }
    let unknown_extension = detector
        .detect_reader("file.avif", &mut Cursor::new(b"no signature"))
        .unwrap();
    assert_eq!(unknown_extension.source, DetectionSource::MimeGuess);
    let pdf = detector
        .detect_reader("file.avif", &mut Cursor::new(b"%PDF-1.7"))
        .unwrap();
    assert_eq!(pdf.source, DetectionSource::Signature);
    assert_eq!(pdf.mime, "application/pdf");
}
#[test]
fn source_signatures_unicode_binary_xml_and_decoder_capabilities_are_explicit() {
    let detector = MimeDetector::compiled();
    let signatures: &[(&[u8], &str)] = &[
        (b"\xff\xd8\xff", "image/jpeg"),
        (b"GIF89a", "image/gif"),
        (b"PK\x03\x04", "application/zip"),
        (b"PK\x05\x06", "application/zip"),
        (b"PK\x07\x08", "application/zip"),
        (b"\x1f\x8b\x08", "application/gzip"),
        (b"\0\0\0\x18ftypabcde", "video/mp4"),
        (b"ID3", "audio/mpeg"),
        (b"\xff\xe0", "audio/mpeg"),
        (
            b"  <svg xmlns='http://www.w3.org/2000/svg'/>",
            "image/svg+xml",
        ),
    ];
    for (bytes, mime) in signatures {
        assert_eq!(detector.detect(bytes).mime, *mime, "{mime}");
    }
    assert_eq!(
        detector.detect(b"<?xml version='1.0'?><config/>").category,
        MimeCategory::Text
    );
    for text in [
        "",
        "😀 Καλημέρα 世界\r\n",
        "\u{feff}Unicode 😀",
        "<svgNotAnImage/>",
    ] {
        assert_eq!(
            detector.detect(text.as_bytes()).category,
            MimeCategory::Text
        );
    }
    for bytes in [
        b"hello\0world".as_slice(),
        b"\xff\xfe\0\0",
        b"\x01\x02\x03\x04",
        b"invalid utf8 \xff",
    ] {
        assert_eq!(detector.detect(bytes).category, MimeCategory::Binary);
    }
    let png = detector.detect(b"\x89PNG\r\n\x1a\n");
    assert_eq!(
        FileClassification::new(png.clone(), &ImageCapabilities::default()).viewer,
        ViewerKind::Hex
    );
    let images = ImageCapabilities::new(["image/png".into()]);
    assert_eq!(
        FileClassification::new(png, &images).viewer,
        ViewerKind::Image
    );
    let svg_name = detector.detect_filename("source.svg");
    assert_eq!(svg_name.category, MimeCategory::Text);
    assert_eq!(
        FileClassification::new(svg_name, &images).viewer,
        ViewerKind::Code
    );
    let invalid = detector.detect_filename("file.json");
    let failure = decode(b"\xff").unwrap_err();
    let classification = FileClassification::new(invalid, &images).decoding_failed(&failure);
    assert_eq!(classification.viewer, ViewerKind::Hex);
    assert!(classification.message.unwrap().contains("UTF-8"));
}
struct CountRead {
    count: usize,
}
impl Read for CountRead {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        buffer.fill(b'x');
        self.count += buffer.len();
        Ok(buffer.len())
    }
}
struct FailRead;
impl Read for FailRead {
    fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
        Err(io::ErrorKind::PermissionDenied.into())
    }
}
#[test]
fn bounded_reads_partial_utf8_limits_missing_special_paths_and_read_failures_stay_visible() {
    let detector = MimeDetector::compiled();
    let mut reader = CountRead { count: 0 };
    detector.detect_reader("file.unknown", &mut reader).unwrap();
    assert_eq!(reader.count, DEFAULT_BYTE_LIMIT + 1);
    assert!(detector.detect_reader("file.json", &mut FailRead).is_err());
    let tiny = detector.clone().with_byte_limit(2).unwrap();
    assert_eq!(tiny.detect("a😀z".as_bytes()).category, MimeCategory::Text);
    assert!(
        detector
            .clone()
            .with_byte_limit(MAX_BYTE_LIMIT + 1)
            .is_err()
    );
    let mut reader = CountRead { count: 0 };
    detector
        .clone()
        .with_byte_limit(0)
        .unwrap()
        .detect_reader("plain", &mut reader)
        .unwrap();
    assert_eq!(reader.count, 4097);
    let directory = tempfile::tempdir().unwrap();
    assert!(
        detector
            .detect_file(directory.path(), &CancellationToken::new())
            .is_err()
    );
    assert!(
        detector
            .detect_file(
                &directory.path().join("missing.json"),
                &CancellationToken::new()
            )
            .is_err()
    );
    let cancel = CancellationToken::new();
    cancel.cancel();
    assert!(matches!(
        detector.detect_file(directory.path(), &cancel),
        Err(DomainError::Cancelled)
    ));
    #[cfg(unix)]
    {
        use nix::{sys::stat::Mode, unistd::mkfifo};
        let fifo = directory.path().join("fifo");
        mkfifo(&fifo, Mode::S_IRUSR | Mode::S_IWUSR).unwrap();
        assert!(
            detector
                .detect_file(&fifo, &CancellationToken::new())
                .is_err()
        );
    }
}
#[tokio::test]
async fn external_table_and_overrides_are_validated_and_worker_classification_is_owned() {
    let mut detector = MimeDetector::compiled();
    detector
        .register_filename(
            "Projectfile",
            MimeEntry {
                language: "custom".into(),
                extension: "".into(),
                mime: "text/project".into(),
                category: MimeCategory::Text,
            },
        )
        .unwrap();
    assert_eq!(detector.detect_filename("Projectfile").mime, "text/project");
    assert!(detector.register_extension("../bad", "text/plain").is_err());
    assert!(MimeDetector::from_table(b"[{}]").is_err());
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("Projectfile");
    fs::write(&path, b"text").unwrap();
    let tag = JobTag {
        workspace: WorkspaceGeneration(3),
        request: RequestId::fresh().unwrap(),
        document: None,
    };
    let mut scheduler = Scheduler::new(Notifications::new(Arc::new(MonotonicClock::default())));
    schedule_classification(
        &mut scheduler,
        OwnerId(4),
        tag,
        path,
        detector,
        ImageCapabilities::default(),
    )
    .unwrap();
    let completion = scheduler.next().await.unwrap();
    assert_eq!(completion.tag, tag);
    let classified = completion.result.unwrap();
    assert_eq!(classified.viewer, ViewerKind::Code);
    assert_eq!(classified.detected.source, DetectionSource::Filename);
    assert!(scheduler.shutdown().await.is_empty());
}
