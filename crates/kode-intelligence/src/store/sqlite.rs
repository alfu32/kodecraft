use super::*;
use crate::{ids::*, model::*};
use kode_core::{DomainError, Result};
use kode_platform::jobs::CancellationToken;
use rusqlite::{Connection, OpenFlags, OptionalExtension, Transaction, params};
use serde::{Serialize, de::DeserializeOwned};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};
const APPLICATION_ID: i32 = 0x4b4f4452;
const SCHEMA_VERSION: u32 = 1;
const MAX_ROW: usize = 64 * 1024 * 1024;
const MAX_TOTAL: usize = 512 * 1024 * 1024;
const TABLES: &[&str] = &[
    "compatibility_usages",
    "compatibility_definitions",
    "generations",
    "dependencies",
    "hints",
    "imports",
    "unresolved",
    "lexical",
    "expressions",
    "types",
    "relations",
    "occurrences",
    "symbols",
    "scopes",
    "files",
    "meta",
];
pub(super) struct SqliteStore {
    pub(super) db: Connection,
}
pub(super) fn sql_error(error: rusqlite::Error) -> DomainError {
    let code = if let rusqlite::Error::SqliteFailure(error, _) = &error {
        error.extended_code
    } else {
        0
    };
    DomainError::Sql {
        sqlstate: None,
        vendor_code: code,
        message: format!("native semantic cache: {error}"),
    }
}
fn invalid(message: impl Into<String>) -> DomainError {
    DomainError::Parse {
        format: "semantic SQLite cache",
        message: message.into(),
    }
}
fn json<T: Serialize>(value: &T) -> Result<String> {
    let value = serde_json::to_string(value).map_err(|e| invalid(e.to_string()))?;
    if value.len() > MAX_ROW {
        return Err(invalid("record exceeds 64 MiB cache row limit"));
    }
    Ok(value)
}
fn decode<T: DeserializeOwned>(value: &str) -> Result<T> {
    serde_json::from_str(value).map_err(|e| invalid(e.to_string()))
}
fn io(operation: &'static str, path: &Path, source: std::io::Error) -> DomainError {
    DomainError::Io {
        operation,
        path: path.into(),
        source,
    }
}
fn regular_or_absent(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => Err(invalid(
            format!("cache path is not a regular file: {}", path.display()),
        )),
        Ok(metadata) if metadata.len() > 1024 * 1024 * 1024 => {
            Err(invalid("cache file exceeds 1 GiB loading limit"))
        }
        Ok(_) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(io("inspect native cache", path, e)),
    }
}
fn project_path(root: &Path) -> Result<PathBuf> {
    let root = fs::canonicalize(root).map_err(|e| io("resolve cache project", root, e))?;
    if !root.is_dir() {
        return Err(invalid("cache project must be a directory"));
    }
    let directory = root.join(".kode-rs");
    match fs::symlink_metadata(&directory) {
        Ok(meta) if meta.file_type().is_symlink() || !meta.is_dir() => {
            return Err(invalid(
                ".kode-rs cache directory must not be a symlink or non-directory",
            ));
        }
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let mut builder = fs::DirBuilder::new();
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                builder.mode(0o700);
            }
            builder
                .create(&directory)
                .map_err(|e| io("create native cache directory", &directory, e))?;
        }
        Err(e) => return Err(io("inspect native cache directory", &directory, e)),
    }
    for name in [
        "index.sqlite",
        "index.sqlite-journal",
        "index.sqlite-wal",
        "index.sqlite-shm",
    ] {
        regular_or_absent(&directory.join(name))?;
    }
    Ok(directory.join("index.sqlite"))
}
impl SqliteStore {
    pub(super) fn open(location: &StoreLocation, rebuild: bool) -> Result<(Self, CacheStatus)> {
        let mut db = match location {
            StoreLocation::Memory => Connection::open_in_memory(),
            StoreLocation::Project(root) => {
                let path = project_path(root)?;
                Connection::open_with_flags(
                    path,
                    OpenFlags::SQLITE_OPEN_READ_WRITE
                        | OpenFlags::SQLITE_OPEN_CREATE
                        | OpenFlags::SQLITE_OPEN_NO_MUTEX
                        | OpenFlags::SQLITE_OPEN_NOFOLLOW,
                )
            }
        }
        .map_err(sql_error)?;
        db.busy_timeout(Duration::from_secs(1)).map_err(sql_error)?;
        use rusqlite::limits::Limit;
        for (limit, value) in [
            (Limit::SQLITE_LIMIT_LENGTH, MAX_ROW as i32),
            (Limit::SQLITE_LIMIT_SQL_LENGTH, 65536),
            (Limit::SQLITE_LIMIT_ATTACHED, 0),
            (Limit::SQLITE_LIMIT_EXPR_DEPTH, 100),
        ] {
            db.set_limit(limit, value).map_err(sql_error)?;
        }
        let application: i32 = db
            .pragma_query_value(None, "application_id", |row| row.get(0))
            .map_err(sql_error)?;
        let version: u32 = db
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .map_err(sql_error)?;
        let tables:i64 = db.query_row("SELECT COUNT(*) FROM sqlite_schema WHERE type='table' AND name NOT LIKE 'sqlite_%'",[],|row|row.get(0)).map_err(sql_error)?;
        let fresh = application == 0 && version == 0 && tables == 0;
        if !fresh && application != APPLICATION_ID {
            return Err(invalid(
                "existing database is not an owned Kode Rust cache; refusing to overwrite",
            ));
        }
        let format = format!(
            "utf8-semantic-v2/{:x}",
            Sha256::digest(include_bytes!(
                "../../../../resources/grammars/manifest.json"
            ))
        );
        let stored_format: Option<String> = if !fresh && version == SCHEMA_VERSION && !rebuild {
            db.query_row("SELECT value FROM meta WHERE key='format'", [], |row| {
                row.get(0)
            })
            .optional()
            .map_err(sql_error)?
        } else {
            None
        };
        let incompatible =
            version != SCHEMA_VERSION || stored_format.as_deref() != Some(format.as_str());
        let status = if fresh {
            CacheStatus::Fresh
        } else if incompatible || rebuild {
            CacheStatus::Rebuilt {
                previous_version: version,
            }
        } else {
            CacheStatus::Ready
        };
        if fresh || incompatible || rebuild {
            let tx = db.transaction().map_err(sql_error)?;
            if !fresh {
                for table in TABLES {
                    tx.execute_batch(&format!("DROP TABLE IF EXISTS {table}"))
                        .map_err(sql_error)?;
                }
            }
            tx.execute_batch(include_str!("schema-v1.sql"))
                .map_err(sql_error)?;
            tx.execute("INSERT INTO meta VALUES ('format',?)", [format])
                .map_err(sql_error)?;
            tx.commit().map_err(sql_error)?;
        }
        db.execute_batch("PRAGMA foreign_keys=ON; PRAGMA trusted_schema=OFF; PRAGMA journal_mode=DELETE; PRAGMA synchronous=FULL;").map_err(sql_error)?;
        let integrity: String = db
            .query_row("PRAGMA quick_check(1)", [], |row| row.get(0))
            .map_err(sql_error)?;
        if integrity != "ok" {
            return Err(invalid(format!(
                "cache integrity check failed: {integrity}"
            )));
        }
        Ok((Self { db }, status))
    }
    pub(super) fn revision(&self) -> Result<u64> {
        let value: String = self
            .db
            .query_row("SELECT value FROM meta WHERE key='revision'", [], |row| {
                row.get(0)
            })
            .map_err(sql_error)?;
        value
            .parse()
            .map_err(|_| invalid("invalid persisted semantic revision"))
    }
    pub(super) fn with_operation<T>(
        &mut self,
        cancel: &CancellationToken,
        operation: impl FnOnce(&mut Self) -> Result<T>,
    ) -> Result<T> {
        if cancel.is_cancelled() {
            return Err(DomainError::Cancelled);
        }
        let token = cancel.clone();
        let started = Instant::now();
        self.db
            .progress_handler(
                1000,
                Some(move || token.is_cancelled() || started.elapsed() > Duration::from_secs(10)),
            )
            .map_err(sql_error)?;
        let result = operation(self);
        self.db
            .progress_handler(0, None::<fn() -> bool>)
            .map_err(sql_error)?;
        match result {
            Err(DomainError::Sql { vendor_code, .. })
                if cancel.is_cancelled() && vendor_code & 255 == 9 =>
            {
                Err(DomainError::Cancelled)
            }
            value => value,
        }
    }
    fn records<T: DeserializeOwned>(&self, table: &str) -> Result<Vec<T>> {
        let mut statement = self
            .db
            .prepare(&format!("SELECT record_json FROM {table} ORDER BY rowid"))
            .map_err(sql_error)?;
        let mut rows = statement.query([]).map_err(sql_error)?;
        let mut bytes = 0usize;
        let mut records = vec![];
        while let Some(row) = rows.next().map_err(sql_error)? {
            let text: String = row.get(0).map_err(sql_error)?;
            bytes = bytes.saturating_add(text.len());
            if text.len() > MAX_ROW || bytes > MAX_TOTAL || records.len() >= 2_000_000 {
                return Err(invalid("cache records exceed loading limits"));
            }
            records.push(decode(&text)?);
        }
        Ok(records)
    }
    pub(super) fn load(&self) -> Result<StoredIndex> {
        let tx = self.db.unchecked_transaction().map_err(sql_error)?;
        let loaded = self.load_transaction()?;
        tx.commit().map_err(sql_error)?;
        Ok(loaded)
    }
    fn load_transaction(&self) -> Result<StoredIndex> {
        let mut statement = self
            .db
            .prepare("SELECT delta_json FROM files ORDER BY file_id")
            .map_err(sql_error)?;
        let mut rows = statement.query([]).map_err(sql_error)?;
        let mut deltas = vec![];
        let mut bytes = 0usize;
        while let Some(row) = rows.next().map_err(sql_error)? {
            let value: String = row.get(0).map_err(sql_error)?;
            bytes = bytes.saturating_add(value.len());
            if bytes > MAX_TOTAL || deltas.len() >= 100_000 {
                return Err(invalid("cached file facts exceed loading limits"));
            }
            deltas.push(decode::<FileSemanticDelta>(&value)?);
        }
        let mut project = ResolvedSemanticProject::from_deltas(deltas)?;
        project.symbols = self.records("symbols")?;
        project.scopes = self.records("scopes")?;
        project.occurrences = self.records("occurrences")?;
        project.relations = self.records("relations")?;
        project.expression_types = self.records("expressions")?;
        project.dependencies = self.records("dependencies")?;
        project.types = self
            .records::<TypeRecord>("types")?
            .into_iter()
            .map(|t| (t.id, t))
            .collect();
        let mut generations = self
            .db
            .prepare("SELECT file_id,generation FROM generations")
            .map_err(sql_error)?;
        let values = generations
            .query_map([], |row| {
                Ok((FileId(row.get(0)?), row.get::<_, String>(1)?))
            })
            .map_err(sql_error)?;
        project.resolution_generations.clear();
        for value in values {
            let (id, value) = value.map_err(sql_error)?;
            project.resolution_generations.insert(
                id,
                value
                    .parse()
                    .map_err(|_| invalid("invalid resolution generation"))?,
            );
        }
        let compatibility = self.load_compatibility(&project)?;
        Ok(StoredIndex {
            snapshot: SemanticSnapshot::new(self.revision()?, project)?,
            compatibility,
        })
    }
    fn load_compatibility(
        &self,
        project: &ResolvedSemanticProject,
    ) -> Result<Vec<CompatibilityFile>> {
        let mut files: BTreeMap<_, _> = project
            .deltas
            .iter()
            .map(|(id, d)| {
                (
                    *id,
                    CompatibilityFile {
                        path: d.file.path.clone(),
                        ..Default::default()
                    },
                )
            })
            .collect();
        for (table, usage) in [
            ("compatibility_definitions", false),
            ("compatibility_usages", true),
        ] {
            let mut statement = self
                .db
                .prepare(&format!(
                    "SELECT file_id,record_json FROM {table} ORDER BY file_id,ordinal"
                ))
                .map_err(sql_error)?;
            let mut rows = statement.query([]).map_err(sql_error)?;
            let mut bytes = 0usize;
            while let Some(row) = rows.next().map_err(sql_error)? {
                let Some(file) = files.get_mut(&FileId(row.get(0).map_err(sql_error)?)) else {
                    continue;
                };
                let value: String = row.get(1).map_err(sql_error)?;
                bytes = bytes.saturating_add(value.len());
                if bytes > MAX_TOTAL {
                    return Err(invalid("compatibility cache exceeds loading limits"));
                }
                if usage {
                    file.usages.push(decode(&value)?);
                } else {
                    file.definitions.push(decode(&value)?);
                }
            }
        }
        Ok(files
            .into_values()
            .filter(|f| !f.definitions.is_empty() || !f.usages.is_empty())
            .collect())
    }
    pub(super) fn persist(
        &mut self,
        snapshot: SemanticSnapshot,
        compatibility: Option<Vec<CompatibilityFile>>,
        cancel: &CancellationToken,
    ) -> Result<(CommitReceipt, SemanticSnapshot)> {
        let snapshot = canonical(snapshot)?;
        let project = snapshot.project();
        let old_revision = self.revision()?;
        if snapshot.version() < old_revision {
            return Err(invalid("semantic revision moved backwards"));
        }
        let supplied_compatibility = compatibility.is_some();
        let compatibility = match compatibility {
            Some(files) => files,
            None => {
                let hashes: BTreeMap<FileId, String> = self
                    .db
                    .prepare("SELECT file_id,content_hash FROM files")
                    .map_err(sql_error)?
                    .query_map([], |row| Ok((FileId(row.get(0)?), row.get(1)?)))
                    .map_err(sql_error)?
                    .collect::<rusqlite::Result<_>>()
                    .map_err(sql_error)?;
                self.load_compatibility(project)?
                    .into_iter()
                    .filter(|file| {
                        let id = SemanticIds::file(&file.path);
                        project
                            .deltas
                            .get(&id)
                            .is_some_and(|delta| hashes.get(&id) == Some(&delta.file.content_hash))
                    })
                    .collect()
            }
        };
        let mut compatibility_by_file: BTreeMap<_, _> = BTreeMap::new();
        for mut file in compatibility {
            let id = SemanticIds::file(&file.path);
            if !project.deltas.contains_key(&id) {
                if supplied_compatibility {
                    return Err(invalid(
                        "compatibility file is absent from semantic project",
                    ));
                }
                continue;
            } // Removed files lose their compatibility facts.
            let mut seen = BTreeSet::new();
            file.definitions = file
                .definitions
                .into_iter()
                .filter_map(|definition| {
                    let key = match json(&definition) {
                        Ok(key) => key,
                        Err(error) => return Some(Err(error)),
                    };
                    seen.insert(key).then_some(Ok(definition))
                })
                .collect::<Result<_>>()?;
            seen.clear();
            file.usages = file
                .usages
                .into_iter()
                .filter_map(|usage| {
                    if usage.start_utf16_column > usage.end_utf16_column {
                        return Some(Err(invalid("compatibility usage columns are reversed")));
                    }
                    let key = match json(&usage) {
                        Ok(key) => key,
                        Err(error) => return Some(Err(error)),
                    };
                    seen.insert(key).then_some(Ok(usage))
                })
                .collect::<Result<_>>()?;
            if file.definitions.is_empty() && file.usages.is_empty() {
                continue;
            }
            if compatibility_by_file.insert(id, file).is_some() {
                return Err(invalid("duplicate compatibility file"));
            }
        }
        let old: BTreeMap<FileId, String> = self
            .db
            .prepare("SELECT file_id,fingerprint FROM files")
            .map_err(sql_error)?
            .query_map([], |row| Ok((FileId(row.get(0)?), row.get(1)?)))
            .map_err(sql_error)?
            .collect::<rusqlite::Result<_>>()
            .map_err(sql_error)?;
        let bundles = bundles(project, &compatibility_by_file, cancel)?;
        let mut changed = vec![];
        let mut removed = vec![];
        let mut size = 0usize;
        for (id, bundle) in &bundles {
            check(cancel)?;
            let value = json(bundle)?;
            size = size.saturating_add(value.len());
            if size > MAX_TOTAL {
                return Err(invalid("semantic cache transaction exceeds 512 MiB"));
            }
            let fingerprint = format!("{:x}", Sha256::digest(value.as_bytes()));
            if old.get(id) != Some(&fingerprint) {
                changed.push((*id, fingerprint));
            }
        }
        for id in old.keys() {
            if !project.deltas.contains_key(id) {
                removed.push(*id);
            }
        }
        let type_values: BTreeMap<TypeId, String> = project
            .types
            .iter()
            .map(|(id, t)| Ok((*id, json(t)?)))
            .collect::<Result<_>>()?;
        let old_types: BTreeMap<TypeId, String> = self
            .db
            .prepare("SELECT type_id,record_json FROM types")
            .map_err(sql_error)?
            .query_map([], |row| Ok((TypeId(row.get(0)?), row.get(1)?)))
            .map_err(sql_error)?
            .collect::<rusqlite::Result<_>>()
            .map_err(sql_error)?;
        let identical = changed.is_empty() && removed.is_empty() && type_values == old_types;
        if snapshot.version() == old_revision {
            if !identical {
                return Err(invalid(
                    "semantic facts changed without advancing their revision",
                ));
            }
            return Ok((
                CommitReceipt {
                    revision: old_revision,
                    changed_files: vec![],
                    removed_files: vec![],
                    reused: true,
                },
                snapshot,
            ));
        }
        let tx = self
            .db
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(sql_error)?;
        let actual_revision: String = tx
            .query_row("SELECT value FROM meta WHERE key='revision'", [], |row| {
                row.get(0)
            })
            .map_err(sql_error)?;
        if actual_revision != old_revision.to_string() {
            return Err(invalid(
                "cache changed while preparing a transaction; retry from current facts",
            ));
        }
        // Delete all changed owners before reinserting paths: renames/swaps never
        // collide with a stale UNIQUE path in another changed file.
        for id in changed.iter().map(|(id, _)| id).chain(removed.iter()) {
            check(cancel)?;
            tx.execute("DELETE FROM files WHERE file_id=?", [id.0])
                .map_err(sql_error)?;
        }
        for (id, fingerprint) in &changed {
            check(cancel)?;
            write_file(&tx, &bundles[id], fingerprint)?;
        }
        for id in old_types.keys().filter(|id| !type_values.contains_key(id)) {
            tx.execute("DELETE FROM types WHERE type_id=?", [id.0])
                .map_err(sql_error)?;
        }
        for (id, value) in type_values {
            check(cancel)?;
            if old_types.get(&id) != Some(&value) {
                tx.execute("INSERT INTO types VALUES (?,?) ON CONFLICT(type_id) DO UPDATE SET record_json=excluded.record_json",params![id.0,value]).map_err(sql_error)?;
            }
        }
        tx.execute(
            "UPDATE meta SET value=? WHERE key='revision'",
            [snapshot.version().to_string()],
        )
        .map_err(sql_error)?;
        check(cancel)?;
        tx.commit().map_err(sql_error)?;
        Ok((
            CommitReceipt {
                revision: snapshot.version(),
                changed_files: changed.into_iter().map(|(id, _)| id).collect(),
                removed_files: removed,
                reused: false,
            },
            snapshot,
        ))
    }
}
fn check(cancel: &CancellationToken) -> Result<()> {
    if cancel.is_cancelled() {
        Err(DomainError::Cancelled)
    } else {
        Ok(())
    }
}
fn canonical(snapshot: SemanticSnapshot) -> Result<SemanticSnapshot> {
    let mut project = snapshot.project().clone();
    let mut deltas = vec![];
    for delta in project.deltas.values() {
        let mut delta = delta.as_ref().clone();
        let mut expressions: BTreeMap<_, ExpressionTypeRecord> = BTreeMap::new();
        for expression in delta.expression_types {
            if expression.range.is_empty() {
                continue;
            }
            let key = (expression.range.start, expression.range.end);
            if expressions
                .get(&key)
                .is_none_or(|old| old.confidence.value() < expression.confidence.value())
            {
                expressions.insert(key, expression);
            }
        }
        delta.expression_types = expressions.into_values().collect();
        let mut lexical: BTreeMap<_, LexicalTokenRecord> = BTreeMap::new();
        for token in delta.lexical_tokens {
            let key = (token.range.start, token.range.end, token.kind);
            if lexical
                .get(&key)
                .is_none_or(|old| old.confidence.value() < token.confidence.value())
            {
                lexical.insert(key, token);
            }
        }
        delta.lexical_tokens = lexical.into_values().collect();
        deltas.push(delta);
    }
    project.deltas = ResolvedSemanticProject::from_deltas(deltas)?.deltas;
    SemanticSnapshot::new(snapshot.version(), project)
}
#[derive(Serialize)]
struct Bundle<'a> {
    delta: &'a FileSemanticDelta,
    symbols: Vec<&'a SymbolRecord>,
    scopes: Vec<&'a ScopeRecord>,
    occurrences: Vec<&'a OccurrenceRecord>,
    relations: Vec<&'a RelationRecord>,
    expressions: Vec<&'a ExpressionTypeRecord>,
    dependencies: Vec<&'a FileDependencyRecord>,
    generation: u64,
    compatibility: Option<&'a CompatibilityFile>,
}
fn bundles<'a>(
    project: &'a ResolvedSemanticProject,
    compatibility: &'a BTreeMap<FileId, CompatibilityFile>,
    cancel: &CancellationToken,
) -> Result<BTreeMap<FileId, Bundle<'a>>> {
    let mut bundles: BTreeMap<_, _> = project
        .deltas
        .iter()
        .map(|(id, d)| {
            (
                *id,
                Bundle {
                    delta: d,
                    symbols: vec![],
                    scopes: vec![],
                    occurrences: vec![],
                    relations: vec![],
                    expressions: vec![],
                    dependencies: vec![],
                    generation: project
                        .resolution_generations
                        .get(id)
                        .copied()
                        .unwrap_or(d.file.dependency_generation),
                    compatibility: compatibility.get(id),
                },
            )
        })
        .collect();
    for symbol in &project.symbols {
        check(cancel)?;
        bundles
            .get_mut(&symbol.file_id)
            .ok_or_else(|| invalid("symbol file absent"))?
            .symbols
            .push(symbol);
    }
    for scope in &project.scopes {
        bundles
            .get_mut(&scope.file_id)
            .ok_or_else(|| invalid("scope file absent"))?
            .scopes
            .push(scope);
    }
    for occurrence in &project.occurrences {
        check(cancel)?;
        bundles
            .get_mut(&occurrence.file_id)
            .ok_or_else(|| invalid("occurrence file absent"))?
            .occurrences
            .push(occurrence);
    }
    let symbol_files: BTreeMap<_, _> = project.symbols.iter().map(|s| (s.id, s.file_id)).collect();
    for relation in &project.relations {
        let file = symbol_files
            .get(&relation.from)
            .ok_or_else(|| invalid("relation source absent"))?;
        bundles
            .get_mut(file)
            .ok_or_else(|| invalid("relation file absent"))?
            .relations
            .push(relation);
    }
    for expression in &project.expression_types {
        bundles
            .get_mut(&expression.file_id)
            .ok_or_else(|| invalid("expression file absent"))?
            .expressions
            .push(expression);
    }
    for dependency in &project.dependencies {
        bundles
            .get_mut(&dependency.from_file_id)
            .ok_or_else(|| invalid("dependency file absent"))?
            .dependencies
            .push(dependency);
    }
    // Canonical ordering makes restart/query order irrelevant to replacement.
    for bundle in bundles.values_mut() {
        bundle.symbols.sort_by_key(|s| s.id);
        bundle.scopes.sort_by_key(|s| s.id);
        bundle.occurrences.sort_by_key(|s| s.id);
        bundle.relations.sort_by_key(|s| (s.from, s.to, s.kind));
        bundle
            .expressions
            .sort_by_key(|s| (s.range.start, s.range.end));
        bundle.dependencies.sort_by_key(|s| (s.to_file_id, s.kind));
    }
    Ok(bundles)
}
fn write_file(tx: &Transaction<'_>, b: &Bundle<'_>, fingerprint: &str) -> Result<()> {
    let d = b.delta;
    let f = &d.file;
    let id = f.id.0;
    tx.execute(
        "INSERT INTO files VALUES (?,?,?,?,?,?,?,?,?)",
        params![
            id,
            f.path,
            f.content_hash,
            f.parse_version.to_string(),
            f.semantic_version.to_string(),
            f.dependency_generation.to_string(),
            d.exported_surface_hash,
            fingerprint,
            json(d)?
        ],
    )
    .map_err(sql_error)?;
    for s in &b.scopes {
        tx.execute(
            "INSERT INTO scopes VALUES (?,?,?,?,?,?,?)",
            params![
                s.id.0,
                id,
                s.parent_id.map(|id| id.0),
                s.owner_symbol_id.map(|id| id.0),
                s.range.start.0 as i64,
                s.range.end.0 as i64,
                json(s)?
            ],
        )
        .map_err(sql_error)?;
    }
    for s in &b.symbols {
        tx.execute(
            "INSERT INTO symbols VALUES (?,?,?,?,?,?,?,?)",
            params![
                s.id.0,
                id,
                s.name,
                s.qualified_name,
                s.scope_id.0,
                s.owner_symbol_id.map(|id| id.0),
                s.kind.wire_name(),
                json(s)?
            ],
        )
        .map_err(sql_error)?;
    }
    for o in &b.occurrences {
        tx.execute(
            "INSERT INTO occurrences VALUES (?,?,?,?,?,?,?)",
            params![
                o.id.0,
                id,
                o.range.start.0 as i64,
                o.range.end.0 as i64,
                o.resolved_symbol_id.map(|id| id.0),
                o.receiver_occurrence_id.map(|id| id.0),
                json(o)?
            ],
        )
        .map_err(sql_error)?;
    }
    for r in &b.relations {
        tx.execute(
            "INSERT INTO relations VALUES (?,?,?,?,?)",
            params![id, r.from.0, r.to.0, r.kind.wire_name(), json(r)?],
        )
        .map_err(sql_error)?;
    }
    for e in &b.expressions {
        tx.execute(
            "INSERT INTO expressions VALUES (?,?,?,?)",
            params![id, e.range.start.0 as i64, e.range.end.0 as i64, json(e)?],
        )
        .map_err(sql_error)?;
    }
    for t in &d.lexical_tokens {
        tx.execute(
            "INSERT INTO lexical VALUES (?,?,?,?,?)",
            params![
                id,
                t.range.start.0 as i64,
                t.range.end.0 as i64,
                t.kind.wire_name(),
                json(t)?
            ],
        )
        .map_err(sql_error)?;
    }
    for (i, t) in d.unresolved_types.iter().enumerate() {
        tx.execute(
            "INSERT INTO unresolved VALUES (?,?,?,?,?,?,?)",
            params![
                id,
                i as i64,
                t.owner_symbol_id.0,
                t.scope_id.0,
                t.name,
                t.role.wire_name(),
                json(t)?
            ],
        )
        .map_err(sql_error)?;
    }
    for (i, t) in d.imports.iter().enumerate() {
        tx.execute(
            "INSERT INTO imports VALUES (?,?,?,?,?)",
            params![id, i as i64, t.scope_id.0, t.path, json(t)?],
        )
        .map_err(sql_error)?;
    }
    for (i, t) in d.type_hints.iter().enumerate() {
        tx.execute(
            "INSERT INTO hints VALUES (?,?,?,?,?)",
            params![id, i as i64, t.target_symbol_id.0, t.scope_id.0, json(t)?],
        )
        .map_err(sql_error)?;
    }
    for t in &b.dependencies {
        tx.execute(
            "INSERT INTO dependencies VALUES (?,?,?,?,?)",
            params![
                id,
                t.to_file_id.0,
                t.kind.wire_name(),
                t.generation.to_string(),
                json(t)?
            ],
        )
        .map_err(sql_error)?;
    }
    tx.execute(
        "INSERT INTO generations VALUES (?,?)",
        params![id, b.generation.to_string()],
    )
    .map_err(sql_error)?;
    if let Some(file) = b.compatibility {
        let mut seen = BTreeSet::new();
        for (i, d) in file.definitions.iter().enumerate() {
            let value = json(d)?;
            if seen.insert(value.clone()) {
                tx.execute(
                    "INSERT INTO compatibility_definitions VALUES (?,?,?,?,?,?)",
                    params![
                        id,
                        i as i64,
                        d.name.to_lowercase(),
                        d.range_bytes.start.0 as i64,
                        d.range_bytes.end.0 as i64,
                        value
                    ],
                )
                .map_err(sql_error)?;
            }
        }
        seen.clear();
        for (i, u) in file.usages.iter().enumerate() {
            let value = json(u)?;
            if seen.insert(value.clone()) {
                tx.execute(
                    "INSERT INTO compatibility_usages VALUES (?,?,?,?,?,?)",
                    params![
                        id,
                        i as i64,
                        u.name.to_lowercase(),
                        u.range_bytes.start.0 as i64,
                        u.range_bytes.end.0 as i64,
                        value
                    ],
                )
                .map_err(sql_error)?;
            }
        }
    }
    Ok(())
}
