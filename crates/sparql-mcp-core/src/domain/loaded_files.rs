//! The vocabulary of the loaded-file ledger: which file (by SHA-256) went into
//! which graph. Shared by the store adapter, which writes it when it loads a
//! whole file, and by `ingest_file`, which writes it with its last batch.

/// Named graph holding SHA-256 records for idempotent file loading.
pub const META_GRAPH: &str = "urn:hkb:meta:loaded-files";
pub const META_SHA256: &str = "urn:hkb:meta:sha256";
pub const META_PATH: &str = "urn:hkb:meta:path";
pub const META_FILE: &str = "urn:hkb:meta:LoadedFile";
/// The graph a loaded file went into (empty string for the default graph).
pub const META_TARGET: &str = "urn:hkb:meta:graph";
