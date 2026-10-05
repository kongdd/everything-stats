# Everything Rust UI

基于 [GPUI](https://gpui.rs/) 的 Windows 桌面搜索界面，布局为菜单栏、文件类型筛选、搜索框、明细列表和状态栏。

## 运行

需要 Windows 10/11、Rust，以及已运行的 Everything 1.4/1.5。安装 [ES 命令行工具](https://www.voidtools.com/downloads/)，将 `es.exe` 放到 Everything 安装目录、应用旁或 `PATH`。

在仓库根目录运行：

```powershell
cargo run --manifest-path ui/Cargo.toml
```

非标准安装位置或命名实例：

```powershell
$env:EVERYTHING_ES = 'D:\Tools\Everything\es.exe'
$env:EVERYTHING_INSTANCE = '1.5a' # 默认实例不必设置
cargo run --manifest-path ui/Cargo.toml
```

GNU 工具链需要 MinGW 的 `dlltool` 和 `windres`。发布构建需要 Windows SDK 中的 `fxc.exe`；无法自动找到时设置 `GPUI_FXC_PATH`：

```powershell
$env:GPUI_FXC_PATH = 'C:\Program Files (x86)\Windows Kits\10\bin\10.0.26100.0\x64\fxc.exe'
cargo build --release --manifest-path ui/Cargo.toml
```

## 功能

- 输入即搜索，120 ms 防抖；旧请求不会覆盖新结果。
- Everything 原生搜索语法、通配符、正则表达式、大小写、全字和完整路径匹配。
- 全部、文件、文件夹、文档、图片、音频、视频分类。
- 菜单栏悬停即展开，相邻菜单直接切换；Esc 或点击外部关闭。
- 名称、路径、类型、大小、修改时间；全索引排序、可调列宽、虚拟列表。
- Windows 文件关联图标；程序、快捷方式显示自身图标，后台加载并缓存。
- 紧凑明细列表：26 px 行高、12 px 字号、4 px 单元格水平内边距，大小右对齐。
- 双击／回车打开，右键打开所在目录、复制路径、限定目录搜索。
- Ctrl 点选、Shift 连选、Ctrl+A 全选当前页；Delete 将选中项移到回收站，由系统确认。
- 收藏搜索条件，保存至 `%LOCALAPPDATA%\everything-ui\bookmarks.json`。
- 导出当前搜索的**全部结果**为 UTF-8 CSV；导出和书签通过临时文件替换，失败不截断旧文件。
- 深色／浅色切换，分页和结果数量。

| 快捷键 | 功能 |
|---|---|
| Ctrl+F / Ctrl+L | 定位搜索框 |
| 搜索框 Enter | 进入结果列表 |
| ↑ / ↓ | 选择结果 |
| Enter / 双击 | 打开选中结果 |
| Alt+Enter | 打开所在目录 |
| Ctrl+C（列表）/ Ctrl+Shift+C | 复制选中路径 |
| Ctrl+A（列表） | 全选当前页 |
| Shift+↑ / Shift+↓（列表） | 向上／向下扩展选区 |
| Delete（列表） | 移到回收站 |
| F5 | 刷新 |
| PageUp / PageDown（列表） | 上一页／下一页 |
| Ctrl+E | 导出全部结果 |
| Ctrl+D | 收藏当前搜索 |
| Ctrl+Q | 退出 |

## 验证

```powershell
cargo fmt --manifest-path ui/Cargo.toml -- --check
cargo test --manifest-path ui/Cargo.toml
cargo clippy --manifest-path ui/Cargo.toml -- -D warnings
# Everything 正在运行时，验证真实查询、分页、正则与全量导出：
cargo test --manifest-path ui/Cargo.toml live_search_and_export -- --ignored
# Debug 构建后，验证仅凭鼠标移动展开／切换菜单，以及 Esc 关闭：
cargo build --manifest-path ui/Cargo.toml
powershell -NoProfile -ExecutionPolicy Bypass -File ui/tests/menu-hover.ps1
```

## 范围

这是 Rust 搜索前端，**不是独立索引引擎**；通过 ES 复用 Everything 索引，不扫描磁盘，不改动原有统计程序。每页加载 500 条，避免一次读入百万记录；索引变化后用 F5 刷新。未实现多选、删除／重命名、系统托盘和索引管理。
