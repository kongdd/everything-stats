//! Search the existing Everything index, never crawl the disk.
use std::{
    env,
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{Context, Result, ensure};

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

    fn expression(self) -> &'static str {
        match self {
            Self::All => "",
            Self::Files => "file:",
            Self::Folders => "folder:",
            Self::Documents => "file: ext:pdf;doc;docx;xls;xlsx;ppt;pptx;txt;md;csv;odt",
            Self::Pictures => "file: ext:png;jpg;jpeg;gif;webp;bmp;svg;tif;tiff;heic",
            Self::Audio => "file: ext:mp3;wav;flac;aac;ogg;m4a;opus",
            Self::Video => "file: ext:mp4;mkv;avi;mov;wmv;webm;m4v",
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

impl Query {
    pub fn arguments(&self) -> Vec<String> {
        let property = ["name", "path", "extension", "size", "date-modified"]
            .get(self.sort_column)
            .unwrap_or(&"name");
        let direction = if self.descending {
            "descending"
        } else {
            "ascending"
        };
        let mut args = vec![
            "-timeout".into(),
            "1500".into(),
            "-sort".into(),
            format!("{property}-{direction}"),
        ];
        for (enabled, flag) in [
            (self.match_case, "case"),
            (self.match_path, "p"),
            (self.whole_word, "whole-word"),
        ] {
            args.push(format!("-{}{flag}", if enabled { "" } else { "no-" }));
        }
        args
    }

    fn expression(&self) -> Result<String> {
        let text = if self.regex && !self.text.is_empty() {
            // ES uses three quotes for a literal quote, not backslash escaping.
            format!("regex:\"{}\"", self.text.replace('"', "\"\"\""))
        } else {
            self.text.clone()
        };
        let text = if text.trim().is_empty() {
            text
        } else {
            format!("<{text}>")
        };
        protect_switches(&format!("{text} {}", self.filter.expression()))
    }
}

// ES parses GetCommandLineW itself and retains quotes. Standard argv escaping changes queries.
fn protect_switches(text: &str) -> Result<String> {
    let mut output = String::new();
    let mut quoted = false;
    let mut start = true;
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        if start && matches!(ch, '-' | '/') {
            output.push_str("\"\""); // Empty quoted prefix: literal search, never an ES command.
        }
        output.push(ch);
        if ch == '"' {
            if chars.peek() == Some(&'"') && chars.clone().nth(1) == Some('"') {
                output.push(chars.next().unwrap());
                output.push(chars.next().unwrap());
            } else {
                quoted = !quoted;
            }
        }
        start = !quoted && matches!(ch, ' ' | '\t' | '\r' | '\n');
    }
    ensure!(!quoted, "搜索文本中的引号未闭合");
    Ok(output)
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

fn executable() -> Result<PathBuf> {
    if let Some(path) = env::var_os("EVERYTHING_ES") {
        let path = PathBuf::from(path);
        ensure!(path.is_file(), "EVERYTHING_ES 指定的 es.exe 不存在");
        return Ok(path);
    }
    let mut candidates = vec![env::current_exe()?.with_file_name("es.exe")];
    for key in ["ProgramFiles", "ProgramFiles(x86)"] {
        if let Some(root) = env::var_os(key) {
            candidates.push(PathBuf::from(root).join("Everything/es.exe"));
        }
    }
    if let Some(path) = env::var_os("PATH") {
        candidates.extend(env::split_paths(&path).map(|root| root.join("es.exe")));
    }
    candidates
        .into_iter()
        .find(|path| path.is_file())
        .context("未找到 es.exe；请安装 Everything 命令行工具，或设置 EVERYTHING_ES")
}

fn command(query: &Query) -> Result<Command> {
    ensure!(!query.text.contains('\0'), "搜索文本不能包含 NUL");
    let mut command = Command::new(executable()?);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000); // CREATE_NO_WINDOW
    }
    if let Some(instance) = env::var_os("EVERYTHING_INSTANCE") {
        command.arg("-instance").arg(instance);
    }
    command.args(query.arguments());
    let expression = query.expression()?;
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.raw_arg(expression);
    }
    #[cfg(not(windows))]
    command.arg(expression);
    Ok(command)
}

fn output(command: &mut Command) -> Result<Vec<u8>> {
    let output = command.output().context("无法启动 es.exe")?;
    ensure!(
        output.status.success(),
        "Everything 查询失败（{}）；请确认 Everything 已运行且索引就绪。{}",
        output.status,
        String::from_utf8_lossy(&output.stderr).trim()
    );
    Ok(output.stdout)
}

pub fn search(query: &Query) -> Result<Page> {
    // ponytail: one ES process per request; use direct SDK IPC if launch latency matters.
    let data = output(command(query)?.args([
        "-csv",
        "-name",
        "-path-column",
        "-size",
        "-date-modified",
        "-attributes",
        "-no-digit-grouping",
        "-size-format",
        "1",
        "-date-format",
        "1",
        "-offset",
        &query.offset.to_string(),
        "-n",
        &PAGE_SIZE.to_string(),
    ]))?;
    let rows = parse(&data)?;
    let count = output(command(query)?.args(["-offset", "0", "-n", "0", "-get-result-count"]))?;
    let total = std::str::from_utf8(&count)?
        .trim()
        .parse()
        .context("Everything 返回的结果数量无效")?;
    Ok(Page { rows, total })
}

fn parse(data: &[u8]) -> Result<Vec<Entry>> {
    let mut reader = csv::Reader::from_reader(data);
    let headers = reader.headers()?.clone();
    let fields = ["Name", "Path", "Size", "Date Modified", "Attributes"].map(|name| {
        headers
            .iter()
            .position(|field| field == name)
            .with_context(|| format!("Everything CSV 缺少 {name} 列"))
    });
    let [name, path, size, modified, attributes] = fields;
    let (name, path, size, modified, attributes) = (name?, path?, size?, modified?, attributes?);
    reader
        .records()
        .map(|record| {
            let record = record?;
            ensure!(!record[name].is_empty(), "Everything 返回空文件名");
            Ok(Entry {
                name: record[name].into(),
                directory: record[path].into(),
                size: if record[size].is_empty() {
                    None
                } else {
                    Some(record[size].parse()?)
                },
                modified: record[modified].replace('T', " "),
                folder: record[attributes].contains('D'),
            })
        })
        .collect()
}

pub fn export(query: &Query, path: &Path) -> Result<()> {
    let parent = path.parent().context("导出路径缺少父目录")?;
    // Close the handle before ES opens the file (Windows sharing rules).
    let temporary = tempfile::NamedTempFile::new_in(parent)?.into_temp_path();
    output(
        command(query)?
            .args([
                "-name",
                "-path-column",
                "-size",
                "-date-modified",
                "-attributes",
                "-no-digit-grouping",
                "-size-format",
                "1",
                "-date-format",
                "1",
                "-offset",
                "0",
                "-n",
                "4294967295",
                "-utf8-bom",
                "-export-csv",
            ])
            .arg(&temporary),
    )?;
    ensure!(
        std::fs::metadata(&temporary)?.len() > 0,
        "Everything 未生成 CSV 文件"
    );
    temporary
        .persist(path)
        .context("无法保存 CSV；原文件未改动")?;
    Ok(())
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn csv_and_search_arguments() {
        let rows = parse(
            concat!(
                "Attributes,Name,Path,Date Modified,Size\r\n",
                "A,\"研究,\"\"报告\"\".txt\",C:\\资料,2026-01-02T03:04:05,1024\r\n",
                "D,目录,C:\\资料,,\r\n"
            )
            .as_bytes(),
        )
        .unwrap();
        assert_eq!(rows[0].name, "研究,\"报告\".txt");
        assert_eq!(rows[0].modified, "2026-01-02 03:04:05");
        assert_eq!(rows[0].kind(), "TXT");
        assert!(rows[1].folder);
        assert_eq!(human_size(rows[0].size), "1.0 KB");
        assert_eq!(human_size(None), "");
        assert!(parse(b"Name,Path\r\n").is_err());
        assert!(parse(b"Name,Path,Size,Date Modified,Attributes\nfile,C:,bad,,A").is_err());
        assert!(
            parse(b"Name,Path,Size,Date Modified,Attributes\r\n")
                .unwrap()
                .is_empty()
        );
        let query = Query {
            text: "-exit | ext:rs".into(),
            filter: Filter::Files,
            sort_column: 3,
            descending: true,
            ..Default::default()
        };
        let args = query.arguments();
        assert_eq!(query.expression().unwrap(), "<-exit | ext:rs> file:");
        assert!(args.contains(&"size-descending".into()));
        assert_eq!(
            protect_switches("name -exit /exit").unwrap(),
            "name \"\"-exit \"\"/exit"
        );
        assert_eq!(
            protect_switches("\"a -exit\" -exit").unwrap(),
            "\"a -exit\" \"\"-exit"
        );
        assert!(protect_switches("\"unclosed").is_err());
        assert!(protect_switches("\"a\"\"\"b\"").is_ok());
        let regex = Query {
            text: "^main\\.rs$".into(),
            regex: true,
            ..query
        };
        assert_eq!(regex.expression().unwrap(), "<regex:\"^main\\.rs$\"> file:");
    }

    #[test]
    #[ignore = "requires a running Everything instance and es.exe"]
    fn live_search_and_export() {
        let query = Query {
            text: "ext:rs".into(),
            ..Default::default()
        };
        let first = search(&query).unwrap();
        assert!(first.total >= first.rows.len());
        assert!(first.rows.len() <= PAGE_SIZE);
        let second = search(&Query {
            offset: PAGE_SIZE,
            ..query.clone()
        })
        .unwrap();
        if let (Some(a), Some(b)) = (first.rows.first(), second.rows.first()) {
            assert_ne!(a.path(), b.path());
        }
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("results.csv");
        export(&query, &path).unwrap();
        let exported = parse(&std::fs::read(&path).unwrap()).unwrap();
        assert!(exported.len() >= first.rows.len());
        std::fs::write(&path, b"keep original").unwrap();
        assert!(
            export(
                &Query {
                    text: "\0".into(),
                    ..query.clone()
                },
                &path
            )
            .is_err()
        );
        assert_eq!(std::fs::read(&path).unwrap(), b"keep original");
        let spaced = search(&Query {
            text: "ext:rs main".into(),
            ..Default::default()
        })
        .unwrap();
        assert!(spaced.total > 0);
        assert!(
            spaced
                .rows
                .iter()
                .all(|row| row.name.to_lowercase().contains("main"))
        );
        let union = search(&Query {
            text: "ext:rs | ext:jl".into(),
            ..Default::default()
        })
        .unwrap();
        assert!(union.total >= first.total);
        let protected = search(&Query {
            text: "ext:rs -get-everything-version".into(),
            ..Default::default()
        })
        .unwrap();
        assert_eq!(protected.total, 0);
        let regex = search(&Query {
            text: "^main\\.rs$".into(),
            regex: true,
            filter: Filter::Files,
            ..Default::default()
        })
        .unwrap();
        assert!(regex.rows.iter().all(|row| row.name == "main.rs"));
    }
}
