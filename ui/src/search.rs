//! Shared search model. Everything lives in `es`; plocate and locate.db will sit beside it.
use std::{
    env,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, ensure};

pub use crate::es::{export, search};

pub const PAGE_SIZE: usize = 500;

#[derive(Clone, Copy, Default, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
pub enum Filter {
    #[default]
    All,
    Files,
    Folders,
    Documents,
    Pictures,
    Audio,
    Video,
}

impl Filter {
    pub const ALL: [Self; 7] = [
        Self::All,
        Self::Files,
        Self::Folders,
        Self::Documents,
        Self::Pictures,
        Self::Audio,
        Self::Video,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::All => "全部",
            Self::Files => "文件",
            Self::Folders => "文件夹",
            Self::Documents => "文档",
            Self::Pictures => "图片",
            Self::Audio => "音频",
            Self::Video => "视频",
        }
    }
}

#[derive(Clone, Default, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
pub struct Query {
    pub text: String,
    pub filter: Filter,
    pub match_case: bool,
    pub match_path: bool,
    pub whole_word: bool,
    pub regex: bool,
    pub sort_column: usize,
    pub descending: bool,
    pub offset: usize,
}

#[derive(Clone, Debug)]
pub struct Entry {
    pub name: String,
    pub directory: String,
    pub size: Option<u64>,
    pub modified: String,
    pub folder: bool,
}

impl Entry {
    pub fn path(&self) -> PathBuf {
        Path::new(&self.directory).join(&self.name)
    }

    pub fn kind(&self) -> String {
        if self.folder {
            return "文件夹".into();
        }
        Path::new(&self.name)
            .extension()
            .map_or_else(|| "文件".into(), |ext| ext.to_string_lossy().to_uppercase())
    }
}

pub struct Page {
    pub rows: Vec<Entry>,
    pub total: usize,
}

fn bookmarks_path() -> Result<PathBuf> {
    Ok(
        PathBuf::from(env::var_os("LOCALAPPDATA").context("LOCALAPPDATA 未设置")?)
            .join("everything-ui/bookmarks.json"),
    )
}

pub fn load_bookmarks() -> Result<Vec<Query>> {
    match std::fs::read(bookmarks_path()?) {
        Ok(data) => {
            let bookmarks: Vec<Query> = serde_json::from_slice(&data).context("书签文件损坏")?;
            ensure!(
                bookmarks
                    .iter()
                    .all(|q| q.sort_column < 5 && !q.text.contains('\0')),
                "书签包含无效搜索参数"
            );
            Ok(bookmarks)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(error) => Err(error.into()),
    }
}

pub fn save_bookmarks(bookmarks: &[Query]) -> Result<()> {
    use std::io::Write;
    let path = bookmarks_path()?;
    let parent = path.parent().context("书签路径缺少父目录")?;
    std::fs::create_dir_all(parent)?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    temporary.write_all(&serde_json::to_vec_pretty(bookmarks)?)?;
    temporary.as_file().sync_all()?;
    temporary
        .persist(path)
        .context("无法保存书签；原文件未改动")?;
    Ok(())
}

pub fn human_size(size: Option<u64>) -> String {
    let Some(size) = size else {
        return String::new();
    };
    if size < 1024 {
        return format!("{size} B");
    }
    let mut value = size as f64;
    let mut unit = "B";
    for next in ["KB", "MB", "GB", "TB", "PB", "EB"] {
        value /= 1024.;
        unit = next;
        if value < 1024. {
            break;
        }
    }
    format!("{value:.1} {unit}")
}

/// Move paths to the recycle bin. `Ok(false)` means the user cancelled.
pub fn recycle(entries: &[Entry]) -> Result<bool> {
    if entries.is_empty() {
        return Ok(false);
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        use windows::{
            Win32::{
                System::Com::{
                    CLSCTX_ALL, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
                    CoUninitialize,
                },
                UI::Shell::{
                    FOF_ALLOWUNDO, FOF_WANTNUKEWARNING, FileOperation, IFileOperation, IShellItem,
                    SHCreateItemFromParsingName,
                },
            },
            core::{HRESULT, PCWSTR},
        };
        unsafe {
            let hr = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
            let uninit = hr == HRESULT(0);
            let result = (|| {
                let operation: IFileOperation = CoCreateInstance(&FileOperation, None, CLSCTX_ALL)
                    .context("无法创建文件操作")?;
                operation
                    .SetOperationFlags(FOF_ALLOWUNDO | FOF_WANTNUKEWARNING)
                    .context("无法设置删除选项")?;
                for entry in entries {
                    let path = entry.path();
                    let wide: Vec<u16> = path.as_os_str().encode_wide().chain([0]).collect();
                    let item: IShellItem = SHCreateItemFromParsingName(PCWSTR(wide.as_ptr()), None)
                        .with_context(|| format!("无法访问 {}", path.display()))?;
                    operation
                        .DeleteItem(&item, None)
                        .with_context(|| format!("无法删除 {}", path.display()))?;
                }
                match operation.PerformOperations() {
                    Ok(()) => Ok(!operation.GetAnyOperationsAborted()?.as_bool()),
                    Err(error) if error.code() == HRESULT(0x8007_04C7u32 as i32) => Ok(false),
                    Err(error) => Err(error).context("删除失败"),
                }
            })();
            if uninit {
                CoUninitialize();
            }
            result
        }
    }
    #[cfg(not(windows))]
    {
        let _ = entries;
        anyhow::bail!("仅 Windows 支持删除到回收站")
    }
}
