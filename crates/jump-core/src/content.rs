// Copyright 2026 entro314-labs
// SPDX-License-Identifier: MPL-2.0

//! Full-text search inside files, on SQLite FTS5.
//!
//! ## Why this is a separate index from [`crate::files`]
//!
//! The two have opposite lifecycles. The path index is cheap to throw away and
//! rebuild wholesale — 8.75 s for a home directory — so it is simply rebuilt on
//! a timer. Content is the reverse: extracting text is expensive, so it must be
//! *incremental*, touching only files whose `(mtime, size)` changed since last
//! time. Sharing one store would force the expensive half to follow the cheap
//! half's refresh strategy.
//!
//! ## Why FTS5 rather than a vector index
//!
//! Launcher queries are overwhelmingly known-item: the user knows roughly what
//! the file is called or what phrase is in it. That is lexical retrieval, which
//! BM25 does well and cheaply. Embeddings answer a different question —
//! semantic recall — and would put a model inference on every keystroke, which
//! is irreconcilable with a sub-millisecond result path. If semantic ranking is
//! ever wanted, the shape is a reranker over the top ~50 rows *of this index*,
//! not a replacement for it.
//!
//! ## Bounds
//!
//! Indexing user documents can pull in an unbounded amount of data, so every
//! axis is capped: file size, extensions, and total documents. Extraction is
//! plain-text only — binary formats need external tools, which are a
//! configuration and failure surface this does not take on.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use rusqlite::{Connection, OptionalExtension, params};

use crate::model::{Icon, Item, ItemKey, Source};

/// Extensions whose contents are indexed.
///
/// Text-like formats only. Anything requiring an external extractor is out of
/// scope: a launcher that shells out to `pdftotext` inherits its failure modes.
pub const DEFAULT_EXTENSIONS: &[&str] = &[
    "md",
    "txt",
    "rst",
    "org",
    "adoc",
    "tex",
    "log",
    "csv",
    "tsv",
    "json",
    "toml",
    "yaml",
    "yml",
    "ini",
    "conf",
    "cfg",
    "xml",
    "html",
    "css",
    "scss",
    "js",
    "ts",
    "jsx",
    "tsx",
    "rs",
    "py",
    "go",
    "rb",
    "sh",
    "bash",
    "zsh",
    "fish",
    "c",
    "h",
    "cpp",
    "hpp",
    "java",
    "kt",
    "swift",
    "sql",
    "lua",
    "vim",
    "nix",
    "dockerfile",
    "gradle",
    "properties",
    "env",
];

/// Size limits for the content index.
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    /// Stop indexing once the database reaches this size.
    pub max_index_bytes: u64,
    /// Skip files larger than this.
    ///
    /// Beyond a couple of megabytes a text file is almost always generated — a
    /// log, a dump, a minified bundle — and indexing it costs far more than it
    /// returns.
    pub max_file_bytes: u64,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_index_bytes: 512 * 1024 * 1024,
            max_file_bytes: 2 * 1024 * 1024,
        }
    }
}

/// Characters of extracted text stored per document.
const MAX_TEXT_CHARS: usize = 200_000;

/// Ceiling on indexed documents, so a pathological tree cannot grow the
/// database without limit.
const MAX_DOCUMENTS: usize = 200_000;

/// Results returned per query.
const RESULTS: usize = 8;

/// Documents written per transaction. See [`Content::index`].
const BATCH: usize = 400;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("could not open the content index")]
    Open(#[source] rusqlite::Error),
    #[error("content index query failed")]
    Query(#[source] rusqlite::Error),
}

/// Full-text index over file contents.
pub struct Content {
    connection: Connection,
    extensions: Vec<String>,
    limits: Limits,
    path: PathBuf,
    /// Reused across documents so a full pass does not allocate a fresh buffer
    /// per file. Sized to the largest file accepted, growing at most once.
    buffer: Vec<u8>,
}

impl std::fmt::Debug for Content {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Content").finish_non_exhaustive()
    }
}

impl Content {
    /// Open, creating the schema if needed.
    pub fn open(path: &Path, extensions: Vec<String>, limits: Limits) -> Result<Self, Error> {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }

        let connection = Connection::open(path).map_err(Error::Open)?;

        // WAL keeps a background reindex from blocking queries, which is the
        // whole point of indexing off the interactive path.
        let _ = connection.pragma_update(None, "journal_mode", "WAL");
        let _ = connection.pragma_update(None, "synchronous", "NORMAL");

        connection
            .execute_batch(
                "CREATE TABLE IF NOT EXISTS documents (
                     path       TEXT PRIMARY KEY,
                     mtime      INTEGER NOT NULL,
                     size       INTEGER NOT NULL
                 );
                 -- `path` is UNINDEXED: it is carried for retrieval, and the
                 -- path index already handles matching on names.
                 CREATE VIRTUAL TABLE IF NOT EXISTS search USING fts5 (
                     path UNINDEXED,
                     body,
                     tokenize = 'unicode61 remove_diacritics 2'
                 );",
            )
            .map_err(Error::Open)?;

        let extensions = if extensions.is_empty() {
            DEFAULT_EXTENSIONS.iter().map(|e| (*e).to_owned()).collect()
        } else {
            extensions
                .iter()
                .map(|e| e.trim_start_matches('.').to_lowercase())
                .collect()
        };

        Ok(Self {
            connection,
            extensions,
            limits,
            path: path.to_path_buf(),
            buffer: Vec::new(),
        })
    }

    /// Current on-disk size, including the write-ahead log.
    ///
    /// The WAL is counted because it is real space the index is using; ignoring
    /// it lets the database sit well over its cap between checkpoints.
    fn size_on_disk(&self) -> u64 {
        let main = std::fs::metadata(&self.path).map(|m| m.len()).unwrap_or(0);
        let wal = std::fs::metadata(self.path.with_extension("db-wal"))
            .map(|m| m.len())
            .unwrap_or(0);
        main + wal
    }

    /// Whether the index has reached its configured size ceiling.
    #[must_use]
    pub fn is_full(&self) -> bool {
        self.size_on_disk() >= self.limits.max_index_bytes
    }

    /// Whether this path is a candidate for content indexing.
    #[must_use]
    pub fn accepts(&self, path: &Path) -> bool {
        accepts(&self.extensions, path)
    }

    /// Number of indexed documents.
    pub fn len(&self) -> usize {
        self.connection
            .query_row("SELECT count(*) FROM documents", [], |row| {
                row.get::<_, i64>(0)
            })
            .map_or(0, |count| count as usize)
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Index `paths`, skipping anything unchanged since last time.
    ///
    /// Returns how many documents were newly extracted. Callers run this off the
    /// interactive path.
    pub fn index(&mut self, paths: &[PathBuf]) -> usize {
        let mut indexed = 0;
        let existing = self.len();

        // Batched rather than one transaction per document. With WAL every
        // commit is an fsync, so committing per file made a first-run index of
        // ~24 000 documents take minutes; a few hundred documents per
        // transaction turns that into one fsync per batch.
        for chunk in paths.chunks(BATCH) {
            if existing + indexed >= MAX_DOCUMENTS {
                tracing::warn!(limit = MAX_DOCUMENTS, "content index full; stopping");
                break;
            }
            // Checked per batch rather than per document: `stat` on the database
            // is cheap but not free, and a batch cannot overshoot the cap by
            // more than one batch of documents.
            if self.is_full() {
                tracing::info!(
                    limit_mb = self.limits.max_index_bytes / (1024 * 1024),
                    "content index reached its size limit; stopping"
                );
                break;
            }

            let Content {
                connection,
                buffer,
                limits,
                extensions,
                ..
            } = self;
            let Ok(transaction) = connection.transaction() else {
                break;
            };

            for path in chunk {
                if !accepts(extensions, path) {
                    continue;
                }

                let Ok(metadata) = std::fs::metadata(path) else {
                    continue;
                };
                if !metadata.is_file() || metadata.len() > limits.max_file_bytes {
                    continue;
                }

                let size = metadata.len() as i64;
                let mtime = metadata
                    .modified()
                    .ok()
                    .and_then(|time| time.duration_since(SystemTime::UNIX_EPOCH).ok())
                    .map_or(0, |age| age.as_secs() as i64);

                if is_current(&transaction, path, mtime, size) {
                    continue;
                }
                if upsert(&transaction, buffer, path, mtime, size) {
                    indexed += 1;
                }
            }

            if transaction.commit().is_err() {
                break;
            }
        }

        if indexed > 0 {
            tracing::info!(indexed, total = self.len(), "content index updated");
        }
        indexed
    }

    /// Drop documents whose files no longer exist.
    pub fn prune_missing(&mut self) {
        let Ok(mut statement) = self.connection.prepare("SELECT path FROM documents") else {
            return;
        };
        let Ok(rows) = statement.query_map([], |row| row.get::<_, String>(0)) else {
            return;
        };

        let gone: Vec<String> = rows
            .flatten()
            .filter(|path| !Path::new(path).exists())
            .collect();
        drop(statement);

        for path in &gone {
            let _ = self
                .connection
                .execute("DELETE FROM search WHERE path = ?1", params![path]);
            let _ = self
                .connection
                .execute("DELETE FROM documents WHERE path = ?1", params![path]);
        }
        if !gone.is_empty() {
            tracing::info!(removed = gone.len(), "pruned deleted documents");
        }
    }

    /// Search document contents, ranked by BM25.
    pub fn search(&self, query: &str) -> Result<Vec<Item>, Error> {
        let Some(expression) = fts_query(query) else {
            return Ok(Vec::new());
        };

        let mut statement = self
            .connection
            .prepare(
                // bm25() returns a *negative* score where more negative is more
                // relevant, so ascending order puts the best match first.
                "SELECT path,
                        snippet(search, 1, '', '', '…', 12) AS excerpt,
                        bm25(search) AS score
                 FROM search
                 WHERE search MATCH ?1
                 ORDER BY score
                 LIMIT ?2",
            )
            .map_err(Error::Query)?;

        let rows = statement
            .query_map(params![expression, RESULTS as i64], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, f64>(2)?,
                ))
            })
            .map_err(Error::Query)?;

        Ok(rows
            .flatten()
            .filter_map(|(path, excerpt, score)| {
                let path = PathBuf::from(path);
                // The index can lag the filesystem; never offer a file that is
                // no longer there.
                path.is_file().then(|| item(&path, &excerpt, score))
            })
            .collect())
    }
}

/// Whether `path`'s extension is one we index.
fn accepts(extensions: &[String], path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            let extension = extension.to_lowercase();
            extensions.contains(&extension)
        })
}

/// Whether the stored copy already matches what is on disk.
fn is_current(connection: &Connection, path: &Path, mtime: i64, size: i64) -> bool {
    let stored: Option<(i64, i64)> = connection
        .query_row(
            "SELECT mtime, size FROM documents WHERE path = ?1",
            params![path.to_string_lossy()],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .ok()
        .flatten();

    stored == Some((mtime, size))
}

/// Extract and store one document, replacing any previous copy.
fn upsert(
    connection: &Connection,
    buffer: &mut Vec<u8>,
    path: &Path,
    mtime: i64,
    size: i64,
) -> bool {
    let Some(text) = extract(buffer, path, size as u64) else {
        return false;
    };
    let path_text = path.to_string_lossy().into_owned();

    // FTS5 has no upsert; the old row has to go first or the document is
    // matched twice, once with stale content.
    let removed = connection.execute("DELETE FROM search WHERE path = ?1", params![path_text]);
    let inserted = connection.execute(
        "INSERT INTO search (path, body) VALUES (?1, ?2)",
        params![path_text, text],
    );
    let recorded = connection.execute(
        "INSERT INTO documents (path, mtime, size) VALUES (?1, ?2, ?3)
         ON CONFLICT(path) DO UPDATE SET mtime = excluded.mtime, size = excluded.size",
        params![path_text, mtime, size],
    );

    removed.is_ok() && inserted.is_ok() && recorded.is_ok()
}

/// Build an FTS5 MATCH expression from user input.
///
/// Every token is quoted and the quotes inside are doubled, because FTS5 MATCH
/// is a query language: a bare `AND`, `*`, `"` or `:` from the user would either
/// change the meaning of the query or raise a syntax error mid-keystroke.
fn fts_query(query: &str) -> Option<String> {
    let tokens: Vec<String> = query
        .split_whitespace()
        .filter(|token| token.chars().any(char::is_alphanumeric))
        .map(|token| format!("\"{}\"", token.replace('"', "\"\"")))
        .collect();

    if tokens.is_empty() {
        return None;
    }

    // All terms required, and the last one is a prefix so results appear while
    // the user is still typing it.
    let last = tokens.len() - 1;
    let mut expression = tokens;
    expression[last].push('*');
    Some(expression.join(" AND "))
}

/// Read a file as text, if it looks like text.
///
/// Reads into a caller-owned buffer that is reused across the whole pass, so a
/// 24 000-document index performs one allocation growth rather than 24 000
/// allocations. `std::fs::read` was doing the latter, plus a second allocation
/// for the lossy conversion and a third for the truncating `collect`.
fn extract(buffer: &mut Vec<u8>, path: &Path, size: u64) -> Option<String> {
    use std::io::Read;

    let file = std::fs::File::open(path).ok()?;
    buffer.clear();
    buffer.reserve(size as usize);
    // Bounded at the reader so a file that grows between `stat` and `read`
    // cannot pull in more than expected.
    file.take(size).read_to_end(buffer).ok()?;

    // A NUL byte in the first block is the standard heuristic for "not text",
    // and catches files whose extension lies.
    if buffer[..buffer.len().min(8192)].contains(&0) {
        return None;
    }

    // Borrow rather than allocate when the bytes are already valid UTF-8, which
    // is the overwhelmingly common case for the extensions indexed here. The
    // binding keeps the lossy fallback alive without relying on temporary
    // lifetime extension inside the match.
    let lossy;
    let text = match std::str::from_utf8(buffer) {
        Ok(text) => text,
        Err(_) => {
            lossy = String::from_utf8_lossy(buffer);
            &lossy
        }
    };

    Some(match text.char_indices().nth(MAX_TEXT_CHARS) {
        Some((cutoff, _)) => text[..cutoff].to_owned(),
        None => text.to_owned(),
    })
}

/// Build a result row from a content hit.
fn item(path: &Path, excerpt: &str, score: f64) -> Item {
    let title = path.file_name().map_or_else(
        || path.display().to_string(),
        |name| name.to_string_lossy().into_owned(),
    );

    let excerpt = excerpt.split_whitespace().collect::<Vec<_>>().join(" ");

    // The same filename often exists in several projects — four copies of
    // CLAUDE.md produced four identical-looking rows. The directory is what
    // tells them apart, so it leads the subtitle and the excerpt follows.
    let location = path
        .parent()
        .map(|parent| {
            let text = parent.display().to_string();
            dirs::home_dir()
                .and_then(|home| {
                    text.strip_prefix(home.to_str()?)
                        .map(|rest| format!("~{rest}"))
                })
                .unwrap_or(text)
        })
        .unwrap_or_default();

    Item {
        key: ItemKey(format!("content:{}", path.display())),
        id: 0,
        title,
        // Location plus the matching text: the excerpt explains why the result
        // is here, the directory says which file it is.
        subtitle: if location.is_empty() {
            excerpt
        } else {
            format!("{location}  ·  {excerpt}")
        },
        icon: Some(Icon::Name("text-x-generic".to_owned())),
        category_icon: Some(Icon::Name("edit-find-symbolic".to_owned())),
        window: None,
        source: Source::File {
            path: path.to_path_buf(),
        },
        // bm25 is negative-better and unbounded; map it onto the positive scale
        // the rest of the ranking uses. -10 or better is treated as excellent.
        autocomplete: None,
        score: (((-score) / 10.0).clamp(0.0, 1.0) as f32) * 3.5,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn index() -> Content {
        // A file-backed database in a temporary directory, because FTS5 and WAL
        // behave differently in memory and the tests should exercise the real
        // configuration.
        let dir = std::env::temp_dir().join(format!("jump-content-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        Content::open(&dir.join("content.db"), Vec::new(), Limits::default()).expect("open index")
    }

    #[test]
    fn quotes_tokens_so_fts_syntax_cannot_leak() {
        // `AND` and `*` are FTS5 operators; unquoted they would change meaning.
        let expression = fts_query("AND value").expect("expression");
        assert!(expression.starts_with("\"AND\""));
        assert!(expression.contains("AND \"value\"*"));
    }

    #[test]
    fn embedded_quotes_are_escaped() {
        let expression = fts_query("say \"hello\"").expect("expression");
        assert!(expression.contains("\"\"hello\"\""));
    }

    #[test]
    fn punctuation_only_queries_are_rejected() {
        assert!(fts_query("!!! ???").is_none());
        assert!(fts_query("   ").is_none());
    }

    #[test]
    fn extension_filter_matches_case_insensitively() {
        let content = index();
        assert!(content.accepts(Path::new("/tmp/a.MD")));
        assert!(content.accepts(Path::new("/tmp/a.rs")));
        assert!(!content.accepts(Path::new("/tmp/a.png")));
    }

    #[test]
    fn indexes_and_finds_by_content() {
        let dir = std::env::temp_dir().join("jump-content-find");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create dir");

        let file = dir.join("notes.md");
        std::fs::write(&file, "the quick brown fox jumps over the lazy dog").expect("write");

        let mut content = Content::open(&dir.join("content.db"), Vec::new(), Limits::default())
            .expect("open index");
        assert_eq!(content.index(std::slice::from_ref(&file)), 1);

        let results = content.search("brown fox").expect("search");
        assert_eq!(results.len(), 1);
        assert!(results[0].subtitle.contains("brown"));

        // Re-indexing an unchanged file must do nothing.
        assert_eq!(content.index(std::slice::from_ref(&file)), 0);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn changed_files_are_reindexed_without_duplicating() {
        let dir = std::env::temp_dir().join("jump-content-change");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create dir");

        let file = dir.join("doc.txt");
        std::fs::write(&file, "original contents here").expect("write");

        let mut content = Content::open(&dir.join("content.db"), Vec::new(), Limits::default())
            .expect("open index");
        content.index(std::slice::from_ref(&file));

        // A different size guarantees the change is detected regardless of
        // filesystem timestamp granularity.
        std::fs::write(&file, "replacement contents entirely different now").expect("rewrite");
        assert_eq!(content.index(std::slice::from_ref(&file)), 1);
        assert_eq!(content.len(), 1, "the document is replaced, not duplicated");

        assert!(content.search("original").expect("search").is_empty());
        assert_eq!(content.search("replacement").expect("search").len(), 1);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn binary_files_are_skipped() {
        let dir = std::env::temp_dir().join("jump-content-binary");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create dir");

        // A .txt containing NUL bytes: the extension lies, the probe catches it.
        let file = dir.join("fake.txt");
        std::fs::write(&file, [0x00, 0x01, 0x02, b'h', b'i']).expect("write");

        let mut content = Content::open(&dir.join("content.db"), Vec::new(), Limits::default())
            .expect("open index");
        assert_eq!(content.index(&[file]), 0);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn deleted_files_are_pruned() {
        let dir = std::env::temp_dir().join("jump-content-prune");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create dir");

        let file = dir.join("temp.md");
        std::fs::write(&file, "some indexed words").expect("write");

        let mut content = Content::open(&dir.join("content.db"), Vec::new(), Limits::default())
            .expect("open index");
        content.index(std::slice::from_ref(&file));
        assert_eq!(content.len(), 1);

        std::fs::remove_file(&file).expect("remove");
        content.prune_missing();
        assert_eq!(content.len(), 0);

        let _ = std::fs::remove_dir_all(&dir);
    }
}
