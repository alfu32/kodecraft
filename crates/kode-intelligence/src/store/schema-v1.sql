PRAGMA application_id = 1263486034;
PRAGMA user_version = 1;
CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
INSERT INTO meta VALUES ('revision','0');
CREATE TABLE files (
 file_id INTEGER PRIMARY KEY, path TEXT NOT NULL UNIQUE,
 content_hash TEXT NOT NULL, parse_version TEXT NOT NULL, semantic_version TEXT NOT NULL,
 dependency_generation TEXT NOT NULL, exported_surface_hash TEXT NOT NULL,
 fingerprint TEXT NOT NULL, delta_json TEXT NOT NULL
);
CREATE TABLE scopes (scope_id INTEGER PRIMARY KEY, file_id INTEGER NOT NULL REFERENCES files ON DELETE CASCADE,
 parent_id INTEGER, owner_symbol_id INTEGER, start_byte INTEGER NOT NULL, end_byte INTEGER NOT NULL, record_json TEXT NOT NULL);
CREATE INDEX scopes_range ON scopes(file_id,start_byte,end_byte);
CREATE TABLE symbols (symbol_id INTEGER PRIMARY KEY, file_id INTEGER NOT NULL REFERENCES files ON DELETE CASCADE,
 name TEXT NOT NULL, qualified_name TEXT, scope_id INTEGER NOT NULL, owner_symbol_id INTEGER, kind TEXT NOT NULL, record_json TEXT NOT NULL);
CREATE INDEX symbols_name ON symbols(name);
CREATE INDEX symbols_qualified ON symbols(qualified_name);
CREATE INDEX symbols_scope ON symbols(scope_id);
CREATE INDEX symbols_file ON symbols(file_id);
CREATE TABLE occurrences (occurrence_id INTEGER PRIMARY KEY, file_id INTEGER NOT NULL REFERENCES files ON DELETE CASCADE,
 start_byte INTEGER NOT NULL, end_byte INTEGER NOT NULL, resolved_symbol_id INTEGER, receiver_occurrence_id INTEGER, record_json TEXT NOT NULL);
CREATE INDEX occurrences_range ON occurrences(file_id,start_byte,end_byte);
CREATE INDEX occurrences_symbol ON occurrences(resolved_symbol_id);
CREATE TABLE relations (file_id INTEGER NOT NULL REFERENCES files ON DELETE CASCADE,
 from_symbol INTEGER NOT NULL, to_symbol INTEGER NOT NULL, kind TEXT NOT NULL, record_json TEXT NOT NULL,
 PRIMARY KEY(from_symbol,to_symbol,kind));
CREATE INDEX relations_to ON relations(to_symbol,kind);
CREATE TABLE types (type_id INTEGER PRIMARY KEY, record_json TEXT NOT NULL);
CREATE TABLE expressions (file_id INTEGER NOT NULL REFERENCES files ON DELETE CASCADE,
 start_byte INTEGER NOT NULL,end_byte INTEGER NOT NULL,record_json TEXT NOT NULL,PRIMARY KEY(file_id,start_byte,end_byte));
CREATE TABLE lexical (file_id INTEGER NOT NULL REFERENCES files ON DELETE CASCADE,
 start_byte INTEGER NOT NULL,end_byte INTEGER NOT NULL,kind TEXT NOT NULL,record_json TEXT NOT NULL,PRIMARY KEY(file_id,start_byte,end_byte,kind));
CREATE TABLE unresolved (file_id INTEGER NOT NULL REFERENCES files ON DELETE CASCADE, ordinal INTEGER NOT NULL,
 owner_symbol_id INTEGER NOT NULL,scope_id INTEGER NOT NULL,name TEXT NOT NULL,role TEXT NOT NULL,record_json TEXT NOT NULL,PRIMARY KEY(file_id,ordinal));
CREATE TABLE imports (file_id INTEGER NOT NULL REFERENCES files ON DELETE CASCADE,ordinal INTEGER NOT NULL,
 scope_id INTEGER NOT NULL,path TEXT NOT NULL,record_json TEXT NOT NULL,PRIMARY KEY(file_id,ordinal));
CREATE TABLE hints (file_id INTEGER NOT NULL REFERENCES files ON DELETE CASCADE,ordinal INTEGER NOT NULL,
 target_symbol_id INTEGER NOT NULL,scope_id INTEGER NOT NULL,record_json TEXT NOT NULL,PRIMARY KEY(file_id,ordinal));
CREATE TABLE dependencies (from_file_id INTEGER NOT NULL REFERENCES files ON DELETE CASCADE,
 to_file_id INTEGER NOT NULL,kind TEXT NOT NULL,generation TEXT NOT NULL,record_json TEXT NOT NULL,PRIMARY KEY(from_file_id,to_file_id,kind));
CREATE INDEX dependencies_to ON dependencies(to_file_id,kind);
CREATE TABLE generations (file_id INTEGER PRIMARY KEY REFERENCES files ON DELETE CASCADE,generation TEXT NOT NULL);
CREATE TABLE compatibility_definitions (file_id INTEGER NOT NULL REFERENCES files ON DELETE CASCADE,ordinal INTEGER NOT NULL,
 name_lc TEXT NOT NULL,start_byte INTEGER NOT NULL,end_byte INTEGER NOT NULL,record_json TEXT NOT NULL,PRIMARY KEY(file_id,ordinal));
CREATE INDEX compatibility_definitions_name ON compatibility_definitions(name_lc);
CREATE TABLE compatibility_usages (file_id INTEGER NOT NULL REFERENCES files ON DELETE CASCADE,ordinal INTEGER NOT NULL,
 name_lc TEXT NOT NULL,start_byte INTEGER NOT NULL,end_byte INTEGER NOT NULL,record_json TEXT NOT NULL,PRIMARY KEY(file_id,ordinal));
CREATE INDEX compatibility_usages_name ON compatibility_usages(name_lc);
