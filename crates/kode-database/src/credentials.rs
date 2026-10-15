use crate::config::{ConfigLock, JsonFile, failure};
use kode_core::{Result, services::DiskIdentity};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};
#[derive(Clone, PartialEq, Eq)]
pub struct Secret(String);
impl Secret {
    pub fn new(value: String) -> Self {
        Self(value)
    }
    pub fn expose(&self) -> &str {
        &self.0
    }
}
impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("[REDACTED]")
    }
}
impl std::fmt::Display for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("[REDACTED]")
    }
}
#[derive(Deserialize, Serialize)]
struct CredentialData {
    version: u32,
    values: BTreeMap<String, String>,
    #[serde(flatten)]
    extra: crate::model::Extra,
}
impl Default for CredentialData {
    fn default() -> Self {
        Self {
            version: 1,
            values: BTreeMap::new(),
            extra: Default::default(),
        }
    }
}
#[derive(Debug, Clone)]
pub struct CredentialStore {
    pub(crate) root: PathBuf,
    file: JsonFile,
}
impl CredentialStore {
    pub fn new(root: PathBuf) -> Self {
        Self {
            file: JsonFile::new(&root, "credentials-v1.json"),
            root,
        }
    }
    fn read(&self) -> Result<(CredentialData, Option<DiskIdentity>)> {
        let Some((bytes, identity)) = self.file.read(1024 * 1024, true)? else {
            return Ok((CredentialData::default(), None));
        };
        let data: CredentialData = serde_json::from_slice(&bytes)
            .map_err(|_| failure("malformed private credential store; original file retained"))?;
        if data.version != 1 || data.values.len() > 4096 {
            return Err(failure("unsupported credential version or entry count"));
        }
        Ok((data, Some(identity)))
    }
    fn publish(&self, data: &CredentialData, expected: Option<&DiskIdentity>) -> Result<()> {
        if data.values.len() > 4096 {
            return Err(failure("credential count exceeds 4,096"));
        }
        if data.values.iter().any(|(reference, value)| {
            reference.trim().is_empty() || reference.len() > 1024 || value.len() > 64 * 1024
        }) {
            return Err(failure(
                "credential reference/value exceeds supported bounds",
            ));
        }
        let bytes =
            serde_json::to_vec(data).map_err(|_| failure("cannot encode credential store"))?;
        if bytes.len() > 1024 * 1024 {
            return Err(failure("credential store exceeds 1 MiB"));
        }
        self.file.write(bytes, expected)
    }
    pub fn get(&self, reference: Option<&str>) -> Result<Option<Secret>> {
        let Some(reference) = reference.filter(|reference| !reference.trim().is_empty()) else {
            return Ok(None);
        };
        Ok(self.read()?.0.values.remove(reference).map(Secret))
    }
    pub fn put(&self, reference: &str, secret: &Secret) -> Result<()> {
        let _lock = ConfigLock::acquire(&self.root)?;
        let (mut data, identity) = self.read()?;
        data.values.insert(reference.into(), secret.expose().into());
        self.publish(&data, identity.as_ref())
    }
    pub fn remove(&self, reference: Option<&str>) -> Result<()> {
        let Some(reference) = reference.filter(|reference| !reference.trim().is_empty()) else {
            return Ok(());
        };
        let _lock = ConfigLock::acquire(&self.root)?;
        let (mut data, identity) = self.read()?;
        if data.values.remove(reference).is_none() {
            return Ok(());
        }
        self.publish(&data, identity.as_ref())
    }
    pub(crate) fn import_values(&self, values: BTreeMap<String, String>) -> Result<usize> {
        let (mut data, identity) = self.read()?;
        let mut imported = 0;
        for (key, value) in values {
            if let Some(old) = data.values.get(&key) {
                if old != &value {
                    return Err(failure(
                        "credential reference collision; original private store retained",
                    ));
                }
            } else {
                data.values.insert(key, value);
                imported += 1;
            }
        }
        if imported > 0 {
            self.publish(&data, identity.as_ref())?;
        }
        Ok(imported)
    }
    pub fn import_java_properties(&self, path: &Path) -> Result<usize> {
        let file = JsonFile { path: path.into() };
        let Some((bytes, _)) = file.read(1024 * 1024, false)? else {
            return Ok(0);
        };
        let values = parse_java_properties(&bytes)?;
        let _lock = ConfigLock::acquire(&self.root)?;
        self.import_values(values)
    }
}
/// java.util.Properties.load(InputStream): Latin-1 bytes, logical continuation
/// lines, escaped keys/separators and UTF-16 \uXXXX escapes. Unpaired surrogates
/// cannot be represented losslessly in Rust and are rejected without echoing data.
pub fn parse_java_properties(bytes: &[u8]) -> Result<BTreeMap<String, String>> {
    if bytes.len() > 1024 * 1024 {
        return Err(failure("legacy properties exceed 1 MiB"));
    }
    let mut logical = Vec::new();
    let mut line = String::new();
    let mut continuation = false;
    let text: String = bytes.iter().map(|byte| char::from(*byte)).collect();
    let physical = text.replace("\r\n", "\n").replace('\r', "\n");
    for part in physical.split('\n') {
        if !continuation
            && part
                .trim_start_matches([' ', '\t', '\x0c'])
                .starts_with(['#', '!'])
        {
            continue;
        }
        let part = if continuation {
            part.trim_start_matches([' ', '\t', '\x0c'])
        } else {
            part
        };
        line.push_str(part);
        let slashes = line.bytes().rev().take_while(|byte| *byte == b'\\').count();
        continuation = slashes % 2 == 1;
        if continuation {
            line.pop();
        } else {
            logical.push(std::mem::take(&mut line));
        }
    }
    if !line.is_empty() {
        logical.push(line);
    }
    let mut values = BTreeMap::new();
    for line in logical {
        let line = line.trim_start_matches([' ', '\t', '\x0c']);
        if line.is_empty() || line.starts_with(['#', '!']) {
            continue;
        }
        let mut escaped = false;
        let mut split = line.len();
        let mut value_start = line.len();
        for (index, ch) in line.char_indices() {
            if !escaped && matches!(ch, '=' | ':' | ' ' | '\t' | '\x0c') {
                split = index;
                value_start = index;
                break;
            }
            escaped = ch == '\\' && !escaped;
        }
        let rest = line[value_start..].trim_start_matches([' ', '\t', '\x0c']);
        let rest = rest
            .strip_prefix(['=', ':'])
            .unwrap_or(rest)
            .trim_start_matches([' ', '\t', '\x0c']);
        values.insert(unescape(&line[..split])?, unescape(rest)?);
        if values.len() > 4096 {
            return Err(failure("too many legacy credential properties"));
        }
    }
    Ok(values)
}
fn unescape(value: &str) -> Result<String> {
    let mut units = Vec::new();
    let mut chars = value.chars();
    while let Some(ch) = chars.next() {
        let ch = if ch == '\\' {
            match chars.next() {
                None => break,
                Some('t') => '\t',
                Some('n') => '\n',
                Some('r') => '\r',
                Some('f') => '\x0c',
                Some('u') => {
                    let mut unit = 0u16;
                    for _ in 0..4 {
                        let digit = chars
                            .next()
                            .and_then(|ch| ch.to_digit(16))
                            .ok_or_else(|| failure("malformed Java properties Unicode escape"))?;
                        unit = unit * 16 + digit as u16;
                    }
                    units.push(unit);
                    continue;
                }
                Some(ch) => ch,
            }
        } else {
            ch
        };
        let mut encoded = [0u16; 2];
        units.extend_from_slice(ch.encode_utf16(&mut encoded));
    }
    String::from_utf16(&units).map_err(|_| {
        failure("unpaired UTF-16 surrogate in legacy properties; original file retained")
    })
}
