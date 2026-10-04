// Adapted from nasfind/src/stats.rs (MIT); count files, not directory entries.
use std::{
    env,
    io::{self, BufWriter, Write},
    num::NonZeroUsize,
    path::{Component, Path, PathBuf},
};

use anyhow::{Context, Result, bail};

#[path = "stats_cache.rs"]
mod cache;

#[derive(clap::Parser, Debug)]
#[command(
    version,
    about = "Rank folders by indexed file count; no disk traversal"
)]
pub struct StatsOptions {
    /// Restrict ranking to this subtree (use the indexed spelling).
    pub path: Option<PathBuf>,
    /// Number of results.
    #[arg(short = 'n', long = "top", default_value = "10")]
    pub top: NonZeroUsize,
    /// Everything.db; defaults to %LOCALAPPDATA%\Everything\Everything.db.
    #[arg(short = 'd', long = "db")]
    pub database: Option<PathBuf>,
    /// SQLite cache; defaults to %LOCALAPPDATA%\es-stats\stats.db.
    #[arg(long)]
    pub cache: Option<PathBuf>,
    /// Include descendants; false counts directly contained files only.
    #[arg(long, default_value = "true", action = clap::ArgAction::Set)]
    pub recursive: bool,
}

pub fn stats(database: &Path, options: &StatsOptions) -> Result<()> {
    let root = options.path.as_deref().map(absolute_path).transpose()?;
    let cache_path = options.cache.clone().map_or_else(default_cache, Ok)?;
    let mut connection = cache::open(database, &cache_path)?;
    cache::ensure(&mut connection, database)?;
    // Keep the totals and rankings in one SQLite read snapshot during refreshes.
    let transaction = connection.transaction()?;
    let (snapshot, all_files) = cache::metadata(&transaction)?.context("empty statistics cache")?;
    if cache::fingerprint(database)? != snapshot {
        bail!("Everything.db changed while querying statistics; retry the command");
    }
    let total = root
        .as_deref()
        .map(|path| cache::total(&transaction, path))
        .transpose()?
        .unwrap_or(all_files);
    let rows = cache::ranked(&transaction, options, root.as_deref())?;
    let mut output = BufWriter::new(io::stdout().lock());
    writeln!(output, "{:>12}  DIRECTORY", "FILES")?;
    for row in rows {
        write!(output, "{:>12}  ", separated(row.count))?;
        output.write_all(&row.path)?;
        output.write_all(b"\n")?;
    }
    output.flush()?;
    let mode = if options.recursive {
        "Recursive file counts; parent/child rankings overlap."
    } else {
        "Direct files only (directories excluded)."
    };
    eprintln!("Total: {} indexed file records. {mode}", separated(total));
    Ok(())
}

#[derive(Debug, Eq, PartialEq)]
struct Rank {
    count: i64,
    path: Vec<u8>,
}

fn default_cache() -> Result<PathBuf> {
    Ok(
        PathBuf::from(env::var_os("LOCALAPPDATA").context("specify --cache")?)
            .join("es-stats/stats.db"),
    )
}

fn is_root(path: &[u8]) -> bool {
    if path.len() == 2 && path[0].is_ascii_alphabetic() && path[1] == b':' {
        return true;
    }
    path.starts_with(b"\\\\") && path[2..].split(|byte| *byte == b'\\').count() == 2
}

fn absolute_path(path: &Path) -> Result<Vec<u8>> {
    let path = if let Ok(relative) = path.strip_prefix("~") {
        PathBuf::from(env::var_os("USERPROFILE").context("USERPROFILE is not set")?).join(relative)
    } else {
        path.to_path_buf()
    };
    let text = path.to_str().context("query path must be valid Unicode")?;
    // C: is an indexed volume root here, not Windows' per-drive current directory.
    let path = if is_root(text.as_bytes()) || path.is_absolute() {
        path
    } else {
        env::current_dir()?.join(path)
    };
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            other => normalized.push(other),
        }
    }
    let mut bytes = normalized
        .to_str()
        .context("query path must be valid Unicode")?
        .replace('/', "\\")
        .trim_end_matches('\\')
        .as_bytes()
        .to_vec();
    if bytes.get(1) == Some(&b':') {
        bytes[0].make_ascii_uppercase();
    }
    Ok(bytes)
}

fn separated(count: i64) -> String {
    let digits = count.to_string();
    let mut result = String::new();
    for (position, digit) in digits.chars().enumerate() {
        if position > 0 && (digits.len() - position).is_multiple_of(3) {
            result.push(',');
        }
        result.push(digit);
    }
    result
}

#[cfg(test)]
#[path = "../tests/stats.rs"]
mod tests;
