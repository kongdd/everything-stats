# Everything Rust UI

基于 [egui](https://github.com/emilk/egui) 的桌面搜索界面。通过 ES 查询已运行的 Everything 索引，不扫描磁盘。

## 运行

需要 Windows 10/11、Rust，以及已运行的 Everything。安装 [ES](https://www.voidtools.com/downloads/)，将 `es.exe` 放到 Everything 安装目录、应用旁或 `PATH`。

```powershell
cargo run --manifest-path ui/Cargo.toml
```

```powershell
$env:EVERYTHING_ES = 'D:\Tools\Everything\es.exe'
$env:EVERYTHING_INSTANCE = '1.5a'
cargo run --manifest-path ui/Cargo.toml
```

## 功能

菜单栏、类型筛选、搜索框、明细列表、状态栏。输入 120 ms 防抖。名称／路径／类型／大小／修改时间可排序。Ctrl 点选、Shift 连选、Shift+方向键扩展，当前行深于其余多选。双击或回车打开，Delete 移到回收站。书签在 `%LOCALAPPDATA%\everything-ui\bookmarks.json`。

| 快捷键 | 功能 |
|---|---|
| Ctrl+F / Ctrl+L | 定位搜索框 |
| ↑ / ↓ | 移动当前行 |
| Shift+↑ / Shift+↓ | 扩展选区 |
| Enter / 双击 | 打开 |
| Alt+Enter | 打开所在目录 |
| Ctrl+C | 复制选中路径 |
| Ctrl+A | 全选当前页 |
| Delete | 移到回收站 |
| F5 | 刷新 |
| PageUp / PageDown | 翻页 |
| Ctrl+E | 导出全部结果 |
| Ctrl+D | 收藏当前搜索 |
| Ctrl+Q | 退出 |
