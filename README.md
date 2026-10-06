# es-stats

按目录统计 Everything 索引中的文件数，不遍历磁盘。
1.5 使用官方 SDK3，1.4 读取离线数据库。

## 使用

```powershell
cargo install --path . --offline
es-stats                          # 全库排名，默认前 10
es-stats 'C:\Users' -n 20          # 指定子树
es-stats d: exts:pdf,docx          # 按扩展名筛选，忽略大小写
es-stats d: exts:pdf,docx --recursive false  # 仅计直属文件
es-stats --db 'Everything.snapshot.db'      # 1.4 离线快照
es-stats update                   # 保存索引并刷新缓存
es-stats update -f                # 强制重建缓存
es-stats clean                    # 清空当前用户各盘回收站
```

- 默认包含子目录；父子计数重叠。`Total` 为整个子树总数，不是排名之和。
- 盘符大小写均可；目录名使用索引中的大小写。
- `clean` 保留系统确认，删除不可撤销。
- `update` 需要 Everything 运行，且安装目录中有 `es.exe`；显式 `--db` 不调用 ES／SDK。
- 1.5 扩展名筛选需要默认实例运行，结果仅存内存，不使用全文件缓存。
- `--db` 默认 `%LOCALAPPDATA%\Everything\Everything.db`。
- `--cache` 默认 `%LOCALAPPDATA%\es-stats\stats.db`；源数据库只读。

## 验证

```powershell
cargo test
cargo clippy -- -D warnings
cargo build --release
```

统计代码改编自 nasfind，保留 MIT 许可。

```toml
[profile.release]
opt-level = "z"
lto = "fat"
codegen-units = 1
panic = "abort"
strip = true
```
