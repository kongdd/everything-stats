mod database;
#[cfg(windows)]
mod sdk;
mod stats;

use std::{env, path::PathBuf};

use anyhow::{Context, Result};
use clap::Parser;

fn main() -> Result<()> {
    let options = stats::StatsOptions::parse();
    if matches!(options.command, Some(stats::Command::CleanRecycleBin)) {
        return clean_recycle_bin();
    }
    let database = match &options.database {
        Some(path) => path.clone(),
        None => PathBuf::from(env::var_os("LOCALAPPDATA").context("specify --db")?)
            .join("Everything/Everything.db"),
    };
    stats::stats(&database, &options)
}

fn clean_recycle_bin() -> Result<()> {
    #[cfg(windows)]
    {
        #[link(name = "shell32")]
        unsafe extern "system" {
            fn SHEmptyRecycleBinW(
                window: *mut std::ffi::c_void,
                root: *const u16,
                flags: u32,
            ) -> i32;
        }

        // SAFETY: null root selects all drives for the current user; flags=0 keeps confirmation.
        let status = unsafe { SHEmptyRecycleBinW(std::ptr::null_mut(), std::ptr::null(), 0) };
        anyhow::ensure!(
            status >= 0,
            "Recycle Bin cleanup failed: HRESULT {status:#010x}"
        );
        eprintln!("Recycle Bin cleanup completed.");
        Ok(())
    }
    #[cfg(not(windows))]
    anyhow::bail!("Recycle Bin cleanup is only supported on Windows")
}
