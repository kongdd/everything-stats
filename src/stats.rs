// Adapted from nasfind/src/stats.rs (MIT); count files, not directory entries.
use std::{
    env, fs,
    io::{self, BufWriter, Write},
    num::NonZeroUsize,
    path::{Component, Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, bail};

#[path = "stats_cache.rs"]
mod cache;

#[derive(clap::Parser, Debug)]
#[command(
    version,
    about = "Rank folders by indexed file count; no disk traversal",
    subcommand_precedence_over_arg = true
)]
pub struct StatsOptions {
    #[command(subcommand)]
    pub command: Option<Command>,
    /// Restrict ranking to this subtree (use the indexed spelling).
    pub path: Option<PathBuf>,
    /// Number of results.
    #[arg(short = 'n', long = "top", default_value = "10")]
    pub top: NonZeroUsize,
    /// Everything.db; defaults to %LOCALAPPDATA%\Everything\Everything.db.
    #[arg(short = 'd', long = "db", global = true)]
    pub database: Option<PathBuf>,
    /// SQLite cache; defaults to %LOCALAPPDATA%\es-stats\stats.db.
    #[arg(long, global = true)]
    pub cache: Option<PathBuf>,
    /// Include descendants; false counts directly contained files only.
    #[arg(long, default_value = "true", action = clap::ArgAction::Set)]
    pub recursive: bool,
}

#[derive(clap::Subcommand, Debug)]
pub enum Command {
    /// Save the running Everything index, then refresh stats.db.
    Update {
        /// Rebuild even when the source fingerprint matches.
        #[arg(short, long)]
        force: bool,
    },
}

pub fn stats(database: &Path, options: &StatsOptions) -> Result<()> {
    if let Some(Command::Update { force }) = options.command {
        return refresh(database, options, force);
    }
    let root = options.path.as_deref().map(absolute_path).transpose()?;
    let cache_path = options.cache.clone().map_or_else(default_cache, Ok)?;
    let mut connection = cache::open(database, &cache_path)?;
    cache::ensure(&mut connection, database, false)?;
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

fn refresh(database: &Path, options: &StatsOptions, force: bool) -> Result<()> {
    // Explicit --db means an offline snapshot: leave Everything alone.
    #[cfg(windows)]
    if options.database.is_none() {
        let executable =
            PathBuf::from(env::var_os("ProgramFiles").context("ProgramFiles is not set")?)
                .join("Everything/Everything.exe");
        let status = std::process::Command::new(&executable)
            .args(["-no-first-instance", "-save-db-now"])
            .status()
            .with_context(|| {
                format!(
                    "cannot run {} (requires Everything 1.5)",
                    executable.display()
                )
            })?;
        anyhow::ensure!(status.success(), "Everything failed to save its database");
    }
    let cache_path = options.cache.clone().map_or_else(default_cache, Ok)?;
    let meta =
        fs::metadata(database).with_context(|| format!("cannot open {}", database.display()))?;
    eprintln!(
        "{}  {}  {} bytes",
        database.display(),
        utc(meta.modified()?)?,
        meta.len()
    );
    let mut connection = cache::open(database, &cache_path)?;
    let rebuilt = cache::ensure(&mut connection, database, force)?;
    eprintln!(
        "{}  {}",
        cache_path.display(),
        if rebuilt { "updated" } else { "current" }
    );
    Ok(())
}

// ponytail: UTC, post-1970; switch to local time if a clock crate is added.
fn utc(time: SystemTime) -> Result<String> {
    let secs = time.duration_since(UNIX_EPOCH)?.as_secs();
    let (y, m, d) = civil(secs / 86400);
    let t = secs % 86400;
    Ok(format!(
        "{y:04}-{m:02}-{d:02} {:02}:{:02}:{:02}Z",
        t / 3600,
        t / 60 % 60,
        t % 60
    ))
}

fn civil(z: u64) -> (i32, u32, u32) {
    let z = z + 719468;
    let era = z / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y as i32, m as u32, d as u32)
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
