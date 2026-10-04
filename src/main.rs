mod database;
mod stats;

use std::{env, path::PathBuf};

use anyhow::{Context, Result};
use clap::Parser;

fn main() -> Result<()> {
    let options = stats::StatsOptions::parse();
    let database = match &options.database {
        Some(path) => path.clone(),
        None => PathBuf::from(env::var_os("LOCALAPPDATA").context("specify --db")?)
            .join("Everything/Everything.db"),
    };
    stats::stats(&database, &options)
}
