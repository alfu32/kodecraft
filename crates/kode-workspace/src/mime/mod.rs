//! Deterministic curated mappings plus bounded signature and content helpers.
use kode_core::{DomainError, JobTag, Result};
use kode_platform::jobs::{CancellationToken, Delivery, OwnerId, WorkerScheduler};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    io::Read,
    path::Path,
};

pub const DEFAULT_BYTE_LIMIT: usize = 3_072;
pub const MAX_BYTE_LIMIT: usize = 65_536;
const OCTET_STREAM: &str = "application/octet-stream";
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum MimeCategory {
    Text,
    Image,
    Binary,
    Unknown,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DetectionSource {
    Filename,
    Extension,
    Signature,
    Shebang,
    MimeGuess,
    Fallback,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MimeResult {
    pub mime: String,
    pub extension: String,
    pub language: Option<String>,
    pub category: MimeCategory,
    pub source: DetectionSource,
}
#[derive(Debug, Clone, Deserialize)]
pub struct MimeEntry {
    pub language: String,
    pub extension: String,
    pub mime: String,
    pub category: MimeCategory,
}
#[derive(Debug, Clone)]
pub struct MimeDetector {
    entries: BTreeMap<String, MimeEntry>,
    filenames: BTreeMap<String, MimeEntry>,
    byte_limit: usize,
}
fn result(
    mime: &str,
    extension: &str,
    language: Option<&str>,
    category: MimeCategory,
    source: DetectionSource,
) -> MimeResult {
    MimeResult {
        mime: mime.into(),
        extension: extension.into(),
        language: language.map(str::to_owned),
        category,
        source,
    }
}
fn entry_result(entry: &MimeEntry, source: DetectionSource) -> MimeResult {
    result(
        &entry.mime,
        &entry.extension,
        Some(&entry.language),
        entry.category,
        source,
    )
}
pub fn category_for_mime(mime: &str) -> MimeCategory {
    let lower = mime.to_ascii_lowercase();
    if lower.starts_with("image/") {
        MimeCategory::Image
    } else if lower.starts_with("text/")
        || ["json", "xml", "yaml", "yml", "markdown", "asciidoc", "toml"]
            .iter()
            .any(|part| lower.contains(part))
    {
        MimeCategory::Text
    } else {
        MimeCategory::Binary
    }
}
impl MimeDetector {
    pub fn compiled() -> Self {
        Self::from_table(include_bytes!("../../../../resources/mime-types.json"))
            .expect("validated compiled MIME table")
    }
    /// Call in a worker after ResourcePaths resolves the F02 resource. Later
    /// duplicate rows win, exactly like Kotlin SampleMimeTable.associateBy.
    pub fn from_table(bytes: &[u8]) -> Result<Self> {
        let entries: Vec<MimeEntry> =
            serde_json::from_slice(bytes).map_err(|error| DomainError::Parse {
                format: "MIME table",
                message: error.to_string(),
            })?;
        let mut detector = Self {
            entries: BTreeMap::new(),
            filenames: BTreeMap::new(),
            byte_limit: DEFAULT_BYTE_LIMIT,
        };
        for entry in entries {
            let key = entry.extension.trim_start_matches('.').to_ascii_lowercase();
            if key.is_empty() || !entry.mime.contains('/') {
                return Err(DomainError::Parse {
                    format: "MIME table",
                    message: "invalid extension or MIME".into(),
                });
            }
            detector.entries.insert(key, entry);
        }
        for (filename, extension) in [
            ("Dockerfile", "dockerfile"),
            ("Makefile", "makefile"),
            ("GNUmakefile", "makefile"),
            ("CMakeLists.txt", "cmake"),
            ("CODEOWNERS", "codeowners"),
            (".gitignore", "txt"),
            (".gitattributes", "txt"),
            (".editorconfig", "ini"),
            (".env", "env"),
        ] {
            if let Some(entry) = detector.entries.get(extension).cloned() {
                detector.filenames.insert(filename.into(), entry);
            }
        }
        Ok(detector)
    }
    pub fn with_byte_limit(mut self, limit: usize) -> Result<Self> {
        let limit = if limit == 0 { 4_096 } else { limit };
        if limit > MAX_BYTE_LIMIT {
            return Err(DomainError::Unsupported(
                "MIME read limit exceeds 64 KiB".into(),
            ));
        }
        self.byte_limit = limit;
        Ok(self)
    }
    pub fn register_extension(&mut self, extension: &str, mime: &str) -> Result<()> {
        let extension = extension.trim_start_matches('.').to_ascii_lowercase();
        if extension.is_empty()
            || extension.contains(['/', '\\'])
            || !mime.contains('/')
            || mime.chars().any(char::is_control)
        {
            return Err(DomainError::Unsupported(
                "invalid extension or MIME override".into(),
            ));
        }
        self.entries.insert(
            extension.clone(),
            MimeEntry {
                language: format!("custom-{extension}"),
                extension: format!(".{extension}"),
                mime: mime.into(),
                category: category_for_mime(mime),
            },
        );
        Ok(())
    }
    /// Decoder defaults may fill an absent extension, while explicit resource
    /// mappings (including user text overrides) retain precedence.
    pub fn register_image_extension_if_absent(
        &mut self,
        extension: &str,
        mime: &str,
    ) -> Result<bool> {
        let key = extension.trim_start_matches('.').to_ascii_lowercase();
        if self.entries.contains_key(&key) {
            return Ok(false);
        }
        self.register_extension(extension, mime)?;
        self.entries
            .get_mut(&key)
            .expect("registered extension")
            .category = MimeCategory::Image;
        Ok(true)
    }
    pub fn register_filename(&mut self, filename: &str, entry: MimeEntry) -> Result<()> {
        if filename.is_empty() || filename.contains(['/', '\\']) || !entry.mime.contains('/') {
            return Err(DomainError::Unsupported(
                "invalid filename MIME override".into(),
            ));
        }
        self.filenames.insert(filename.into(), entry);
        Ok(())
    }
    pub fn detect_filename(&self, name: &str) -> MimeResult {
        let basename = name.rsplit(['/', '\\']).next().unwrap_or(name);
        if let Some(entry) = self.filenames.get(basename) {
            return entry_result(entry, DetectionSource::Filename);
        }
        let extension = basename
            .rsplit_once('.')
            .map_or("", |(_, extension)| extension)
            .to_ascii_lowercase();
        // Preserve the source's last-suffix behavior (including .blade.php).
        if let Some(entry) = self.entries.get(&extension) {
            return entry_result(entry, DetectionSource::Extension);
        }
        result(
            OCTET_STREAM,
            &if extension.is_empty() {
                String::new()
            } else {
                format!(".{extension}")
            },
            None,
            MimeCategory::Binary,
            DetectionSource::Fallback,
        )
    }
    pub fn detect(&self, bytes: &[u8]) -> MimeResult {
        let truncated = bytes.len() > self.byte_limit;
        self.detect_sample(&bytes[..bytes.len().min(self.byte_limit)], truncated)
    }
    fn detect_sample(&self, bytes: &[u8], truncated: bool) -> MimeResult {
        if let Some(result) = shell_shebang(bytes) {
            return result;
        }
        if let Some(signature) = signature(bytes) {
            return signature;
        }
        if looks_like_text(bytes, truncated) {
            result(
                "text/plain",
                ".txt",
                Some("plain-text"),
                MimeCategory::Text,
                DetectionSource::Fallback,
            )
        } else {
            result(
                OCTET_STREAM,
                "",
                None,
                MimeCategory::Binary,
                DetectionSource::Fallback,
            )
        }
    }
    /// Consumes at most limit+1 bytes: one extra byte detects a cut UTF-8 scalar.
    pub fn detect_reader(&self, name: &str, reader: &mut dyn Read) -> Result<MimeResult> {
        let mut bytes = vec![];
        reader
            .take((self.byte_limit + 1) as u64)
            .read_to_end(&mut bytes)
            .map_err(|source| DomainError::Io {
                operation: "read MIME sample",
                path: name.into(),
                source,
            })?;
        let truncated = bytes.len() > self.byte_limit;
        bytes.truncate(self.byte_limit);
        let named = self.detect_filename(name);
        if let Some(result) = shell_shebang(&bytes) {
            return Ok(result);
        }
        if named.source != DetectionSource::Fallback {
            return Ok(named);
        }
        let content = self.detect_sample(&bytes, truncated);
        if content.source != DetectionSource::Fallback {
            return Ok(content);
        }
        // Deterministic replacement for JVM platform probing. It follows bounded
        // signature sniffing, and cannot replace an explicit Kode mapping.
        if let Some(mime) = mime_guess::from_path(name).first_raw() {
            return Ok(result(
                mime,
                &named.extension,
                content.language.as_deref(),
                category_for_mime(mime),
                DetectionSource::MimeGuess,
            ));
        }
        Ok(MimeResult {
            extension: if content.extension.is_empty() {
                named.extension
            } else {
                content.extension
            },
            ..content
        })
    }
    pub fn detect_file(&self, path: &Path, cancel: &CancellationToken) -> Result<MimeResult> {
        if cancel.is_cancelled() {
            return Err(DomainError::Cancelled);
        }
        let mut file = kode_platform::files::open_regular_file(path)?;
        let result = self.detect_reader(&path.to_string_lossy(), &mut file)?;
        if cancel.is_cancelled() {
            return Err(DomainError::Cancelled);
        }
        Ok(result)
    }
}
impl Default for MimeDetector {
    fn default() -> Self {
        Self::compiled()
    }
}
fn shell_shebang(bytes: &[u8]) -> Option<MimeResult> {
    let bytes = bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(bytes);
    let line = bytes.split(|byte| *byte == b'\n').next()?;
    let line = std::str::from_utf8(line)
        .ok()?
        .trim_end_matches('\r')
        .strip_prefix("#!")?;
    let mut tokens = line.split_whitespace();
    let first = tokens.next()?;
    let interpreter = if first.rsplit('/').next()? == "env" {
        tokens.find(|token| !token.starts_with('-') && !token.contains('='))?
    } else {
        first
    };
    let name = interpreter.rsplit('/').next()?.to_ascii_lowercase();
    if ![
        "ash", "bash", "csh", "dash", "fish", "ksh", "nu", "nushell", "sh", "tcsh", "zsh",
    ]
    .contains(&name.as_str())
    {
        return None;
    }
    Some(result(
        "text/shellscript",
        ".sh",
        Some("shellscript"),
        MimeCategory::Text,
        DetectionSource::Shebang,
    ))
}
fn signature(bytes: &[u8]) -> Option<MimeResult> {
    // Match the small source's permissive partial signatures before infer's
    // broader format helpers. Explicit filename mappings still take precedence.
    let pair = if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some(("image/png", ".png"))
    } else if bytes.starts_with(b"\xff\xd8\xff") {
        Some(("image/jpeg", ".jpg"))
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some(("image/gif", ".gif"))
    } else if bytes.starts_with(b"%PDF-") {
        Some(("application/pdf", ".pdf"))
    } else if [b"PK\x03\x04", b"PK\x05\x06", b"PK\x07\x08"]
        .iter()
        .any(|magic| bytes.starts_with(*magic))
    {
        Some(("application/zip", ".zip"))
    } else if bytes.starts_with(b"\x1f\x8b\x08") {
        Some(("application/gzip", ".gz"))
    } else if bytes.len() > 12 && bytes.get(4..8) == Some(b"ftyp") {
        Some(("video/mp4", ".mp4"))
    } else if bytes.starts_with(b"ID3")
        || (bytes.len() >= 2 && bytes[0] == 0xff && bytes[1] & 0xe0 == 0xe0)
    {
        Some(("audio/mpeg", ".mp3"))
    } else {
        None
    };
    if let Some((mime, extension)) = pair {
        return Some(result(
            mime,
            extension,
            None,
            category_for_mime(mime),
            DetectionSource::Signature,
        ));
    }
    if let Ok(text) = std::str::from_utf8(bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(bytes)) {
        let text = text.trim_start();
        let svg_tag = |index: usize| {
            text.as_bytes()
                .get(index + 4)
                .is_none_or(|byte| byte.is_ascii_whitespace() || b">/".contains(byte))
        };
        let svg = text.starts_with("<svg") && svg_tag(0)
            || text.starts_with("<?xml")
                && text.match_indices("<svg").any(|(index, _)| svg_tag(index));
        if svg {
            return Some(result(
                "image/svg+xml",
                ".svg",
                None,
                MimeCategory::Image,
                DetectionSource::Signature,
            ));
        }
        if text.starts_with("<?xml") {
            return Some(result(
                "application/xml",
                ".xml",
                Some("xml"),
                MimeCategory::Text,
                DetectionSource::Signature,
            ));
        }
    }
    infer::get(bytes)
        .filter(|kind| kind.mime_type() != "text/x-shellscript")
        .map(|kind| {
            result(
                kind.mime_type(),
                &format!(".{}", kind.extension()),
                None,
                category_for_mime(kind.mime_type()),
                DetectionSource::Signature,
            )
        })
}
fn looks_like_text(bytes: &[u8], truncated: bool) -> bool {
    let bytes = bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(bytes);
    let text = match std::str::from_utf8(bytes) {
        Ok(text) => text,
        Err(error) if truncated && error.error_len().is_none() => {
            match std::str::from_utf8(&bytes[..error.valid_up_to()]) {
                Ok(text) => text,
                Err(_) => return false,
            }
        }
        Err(_) => return false,
    };
    if text.contains('\0') {
        return false;
    }
    let mut count = 0;
    let mut printable = 0;
    for character in text.chars() {
        count += 1;
        if !character.is_control() || ['\t', '\n', '\u{b}', '\u{c}', '\r'].contains(&character) {
            printable += 1;
        }
    }
    printable * 10 >= count * 9
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ViewerKind {
    Code,
    Image,
    Hex,
}
/// Decoder services publish the formats they actually enable/qualify. Empty
/// means no image decoder; an image MIME alone does not promise a working viewer.
#[derive(Debug, Default, Clone)]
pub struct ImageCapabilities {
    mimes: BTreeSet<String>,
}
impl ImageCapabilities {
    pub fn new(mimes: impl IntoIterator<Item = String>) -> Self {
        Self {
            mimes: mimes
                .into_iter()
                .map(|mime| mime.to_ascii_lowercase())
                .collect(),
        }
    }
    pub fn supports(&self, mime: &str) -> bool {
        self.mimes.contains(&mime.to_ascii_lowercase())
    }
}
#[derive(Debug, Clone)]
pub struct FileClassification {
    pub detected: MimeResult,
    pub viewer: ViewerKind,
    pub message: Option<String>,
}
impl FileClassification {
    pub fn new(detected: MimeResult, images: &ImageCapabilities) -> Self {
        let (viewer, message) = match detected.category {
            MimeCategory::Text => (ViewerKind::Code, None),
            MimeCategory::Image if images.supports(&detected.mime) => (ViewerKind::Image, None),
            MimeCategory::Image => (
                ViewerKind::Hex,
                Some(format!(
                    "No decoder enabled for {}; open as bytes",
                    detected.mime
                )),
            ),
            MimeCategory::Binary | MimeCategory::Unknown => (ViewerKind::Hex, None),
        };
        Self {
            detected,
            viewer,
            message,
        }
    }
    pub fn decoding_failed(mut self, error: &DomainError) -> Self {
        self.viewer = ViewerKind::Hex;
        self.message = Some(format!("{error}; inspect file as bytes"));
        self
    }
}
pub fn schedule_classification(
    scheduler: &mut dyn WorkerScheduler<FileClassification>,
    owner: OwnerId,
    tag: JobTag,
    path: std::path::PathBuf,
    detector: MimeDetector,
    images: ImageCapabilities,
) -> Result<()> {
    scheduler.schedule_blocking(
        owner,
        tag,
        Delivery::Cancellable,
        Box::new(move |cancel| {
            detector
                .detect_file(&path, &cancel)
                .map(|detected| FileClassification::new(detected, &images))
        }),
    )
}
