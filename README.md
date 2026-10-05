# es-stats

从 Everything.db 统计各目录的**文件数**，不遍历磁盘。由 `../nasfind/src/stats.rs`、`stats_cache.rs` 复制改造，原 nasfind 未修改；保留 MIT 许可。

## 使用

在本目录安装并运行（PowerShell）：

```powershell
cargo install --path . --offline
es-stats -n 20
es-stats 'C:\Users' -n 20
es-stats 'Z:\GitHub' --recursive false -n 20
es-stats --db '..\data\Everything.snapshot.db'
es-stats update
es-stats update -f
```

- `update`：检查 Everything.db 的修改时间（及路径、长度）；不一致则重建 stats.db，不输出排名。`-f` 强制重建。
- `--db` / `-d`：源数据库，默认 `%LOCALAPPDATA%\Everything\Everything.db`。
- `--cache`：SQLite 缓存，默认 `%LOCALAPPDATA%\es-stats\stats.db`，不向当前目录写入。
- `--recursive true`：默认；文件数包含所有后代目录中的文件。
- `--recursive false`：仅统计直接包含的文件，不计子目录本身。
- `--top` / `-n`：显示前 N 个目录，默认 10。
- 盘符大小写均可；目录名请使用索引中的大小写。允许末尾反斜杠、`.` 和 `..`，不要求目录在线。

计数对象为 **Everything 中的文件记录**，不是 nasfind 原版的“文件＋目录”条目；不按 FRN／硬链接身份去重。只统计已保存到 DB 的索引，不代表实时磁盘状态；已经被 Everything 排除的文件不在统计范围内。

## 实现

| 文件 | 用途 |
|---|---|
| `src/stats.rs` | 查询、输出与 Windows 查询路径规范化 |
| `src/stats_cache.rs` | 单源 SQLite 缓存与排名 |
| `src/database.rs` | 只读解码，沿原生目录 ID 汇总计数并恢复目录路径 |
| `%LOCALAPPDATA%\es-stats\stats.db` | 默认缓存，保存直接与递归计数 |
| `data/top-recursive.txt` | 全库递归计数示例 |
| `data/top-direct.txt` | 全库直接文件计数示例 |
| `data/top-users.txt` | C:\Users 子树示例 |
| `data/validation.txt` | 全库校验结果 |

保留空目录的零计数，但排名仅显示非零项。全库排名包含盘符根目录；递归模式下父子计数有重叠，不能将排名各行直接相加。输出中的 `Total` 是整库或指定子树的文件总数，不是前 N 行之和；直接模式下该总数仍包含子树后代文件。

源数据库只读，缓存单独存放；通过源文件的路径、长度和修改时间判断缓存是否失效，重建采用 SQLite 事务。缓存有独立 application ID，拒绝覆盖源 DB 或混用 nasfind 的 stats.db。

缓存仅保存一个源数据库的统计，切换 `--db` 时重建；本工具旧 v1 缓存会在源库解析成功后事务升级为 v2。直接复用 Everything 的目录 ID 和父子关系，不再维护路径字典、多索引选择表或第二套目录编号。

目前仅支持**未压缩 ESDb 1.7.20，NTFS／文件夹来源**；其他版本、文件列表来源和 ReFS 明确报错。文件必须引用已索引目录。名称按原始字节保存，含孤立代理项的名称不会被替换；终端显示这类名称可能不正常。

## 验证与构建

```powershell
cargo test
cargo clippy -- -D warnings
cargo build --release
```

6 项测试覆盖 NTFS／文件夹来源、非拓扑父索引、中文和重复名称、两种计数、子树排名、异常输入、原始名称字节、缓存失败回滚、旧缓存升级，以及用户缓存路径和小写盘符。

本机全库验证：**1,046,436 个目录，8,228,251 条文件记录**；10 个盘符总数与原解析器一致，全部目录满足“递归数＝直接数＋子目录递归数”，SQLite 完整性检查通过。
主体代码由 **706 行减至 567 行**（约减少 20%），删除 `serde_json` 依赖。全部目录的路径、直接计数与递归计数经 SHA-256 对照与精简前一致。

精简后建缓存约 **13.32 秒**；C:\Users 子树缓存查询约 **0.37 秒**。这些为本机样本实测值，不是性能保证。

若替换 DB 时保留了相同长度和修改时间，需手动删除本工具的缓存后重建。数据库和统计结果含本机路径，勿公开上传。
