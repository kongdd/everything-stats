// Adapted from nasfind/src/stats_cache.rs (MIT); one source, native directory IDs.
use std::{
    fs,
    path::Path,
    time::{Duration, Instant, UNIX_EPOCH},
};

use anyhow::{Context, Result, bail, ensure};
use rusqlite::{Connection, OptionalExtension, params};

use super::{Rank, StatsOptions};
use crate::database::Counts;

const APPLICATION_ID: i64 = 0x45535453; // ESTS; reject unrelated SQLite files.

pub(super) fn open(database: &Path, cache: &Path) -> Result<Connection> {
    let source = fs::canonicalize(database).context("cannot open Everything.db")?;
    ensure!(
        !cache.exists()
            || fs::canonicalize(cache)
                .with_context(|| format!("cannot inspect cache {}", cache.display()))?
                != source,
        "cache must not be Everything.db"
    );
    if let Some(parent) = cache.parent().filter(|p| !p.as_os_str().is_empty()) {
        fs::create_dir_all(parent)
            .with_context(|| format!("cannot create cache directory {}", parent.display()))?;
    }
    let connection = Connection::open(cache)
        .with_context(|| format!("cannot open cache {}", cache.display()))?;
    connection.busy_timeout(Duration::from_secs(30))?;
    schema(&connection)?;
    Ok(connection)
}

pub(super) fn schema(connection: &Connection) -> Result<()> {
    let application: i64 = connection.query_row("PRAGMA application_id", [], |r| r.get(0))?;
    let version: i64 = connection.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    let tables: i64 = connection.query_row(
        "SELECT count(*) FROM sqlite_master WHERE type='table'",
        [],
        |r| r.get(0),
    )?;
    ensure!(
        (application == APPLICATION_ID && matches!(version, 1 | 2))
            || (application == 0 && version == 0 && tables == 0),
        "not an es-stats cache, or unsupported cache version"
    );
    connection.execute_batch("PRAGMA synchronous=NORMAL; PRAGMA cache_size=-32768;")?;
    Ok(())
}

// Stable FNV-1a key. Full-byte comparison handles collisions and preserves WTF-8.
fn path_hash(path: &[u8]) -> i64 {
    path.iter().fold(0xcbf29ce484222325_u64, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x100000001b3)
    }) as i64
}

pub(super) fn fingerprint(database: &Path) -> Result<Vec<u8>> {
    let path = fs::canonicalize(database).context("Everything.db is unavailable")?;
    let metadata = fs::metadata(&path)?;
    // ponytail: path/length/mtime key; add a content hash for timestamp-preserving replacements.
    let mut snapshot = path.as_os_str().as_encoded_bytes().to_vec();
    snapshot.extend(metadata.len().to_le_bytes());
    snapshot.extend(
        metadata
            .modified()?
            .duration_since(UNIX_EPOCH)?
            .as_nanos()
            .to_le_bytes(),
    );
    Ok(snapshot)
}

pub(super) fn metadata(connection: &Connection) -> Result<Option<(Vec<u8>, i64)>> {
    let version: i64 = connection.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    if version != 2 {
        return Ok(None);
    }
    Ok(connection
        .query_row("SELECT snapshot, total FROM metadata", [], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .optional()?)
}

pub(super) fn ensure(
    connection: &mut Connection,
    database: &Path,
    force: bool,
    live: bool,
) -> Result<bool> {
    let snapshot = fingerprint(database)?;
    if !force && metadata(connection)?.is_some_and(|(previous, _)| previous == snapshot) {
        return Ok(false);
    }
    eprintln!("Building file-count cache from {}...", database.display());
    let start = Instant::now();
    let counts = crate::database::collect(database, live, None)?;
    if fingerprint(database)? != snapshot {
        bail!("Everything.db changed while building statistics; retry the command");
    }
    let directories = counts.nodes.len();
    store(connection, &snapshot, counts)?;
    eprintln!(
        "Cached {directories} directories in {:.2}s",
        start.elapsed().as_secs_f64()
    );
    Ok(true)
}

pub(super) fn store(connection: &mut Connection, snapshot: &[u8], counts: Counts) -> Result<()> {
    let transaction = connection.transaction()?;
    let version: i64 = transaction.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    if version != 2 {
        // Replace the owned v1 cache only after parsing succeeds, within this transaction.
        transaction.execute_batch(&format!(
            "DROP TABLE IF EXISTS counts;
            DROP TABLE IF EXISTS directories;
            DROP TABLE IF EXISTS selections;
            CREATE TABLE metadata(snapshot BLOB NOT NULL, total INTEGER NOT NULL);
            CREATE TABLE counts(
                id INTEGER PRIMARY KEY, parent INTEGER, path BLOB NOT NULL,
                path_hash INTEGER NOT NULL, direct_count INTEGER NOT NULL,
                recursive_count INTEGER NOT NULL
            );
            CREATE INDEX path_lookup ON counts(path_hash);
            CREATE INDEX children ON counts(parent);
            CREATE INDEX direct_rank ON counts(direct_count DESC);
            CREATE INDEX recursive_rank ON counts(recursive_count DESC);
            PRAGMA application_id={APPLICATION_ID}; PRAGMA user_version=2;"
        ))?;
    }
    transaction.execute_batch("DELETE FROM counts; DELETE FROM metadata;")?;
    {
        let mut insert =
            transaction.prepare("INSERT INTO counts VALUES (?1, ?2, ?3, ?4, ?5, ?6)")?;
        for (id, node) in counts.nodes.iter().enumerate() {
            insert.execute(params![
                id as i64,
                node.parent.map(|p| p as i64),
                &node.path,
                path_hash(&node.path),
                node.direct,
                node.recursive
            ])?;
        }
    }
    transaction.execute(
        "INSERT INTO metadata VALUES (?1, ?2)",
        params![snapshot, counts.total],
    )?;
    transaction.commit()?;
    Ok(())
}

pub(super) fn total(connection: &Connection, root: &[u8]) -> Result<i64> {
    Ok(connection.query_row(
        "SELECT COALESCE(sum(recursive_count), 0) FROM counts WHERE path_hash=?1 AND path=?2",
        params![path_hash(root), root],
        |r| r.get(0),
    )?)
}

pub(super) fn ranked(
    connection: &Connection,
    options: &StatsOptions,
    root: Option<&[u8]>,
) -> Result<Vec<Rank>> {
    let column = if options.recursive {
        "recursive_count"
    } else {
        "direct_count"
    };
    // Integer parent links scope the query without repeating path strings in indexes.
    let sql = format!(
        "WITH RECURSIVE subtree(id) AS (
            SELECT id FROM counts WHERE path_hash=?3 AND path=?2
            UNION ALL SELECT c.id FROM counts c JOIN subtree s ON c.parent=s.id
        ) SELECT path, {column} FROM counts WHERE {column}>0
            AND (?2 IS NULL OR id IN (SELECT id FROM subtree))
        ORDER BY {column} DESC, path ASC LIMIT ?1"
    );
    let top = i64::try_from(options.top.get()).unwrap_or(i64::MAX);
    Ok(connection
        .prepare(&sql)?
        .query_map(params![top, root, root.map(path_hash)], |r| {
            Ok(Rank {
                path: r.get(0)?,
                count: r.get(1)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?)
}
