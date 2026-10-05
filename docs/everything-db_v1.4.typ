#set document(title: "Everything.db 数据格式", author: "本地逆向分析")
#set page(paper: "a4", margin: (x: 22mm, y: 20mm), numbering: "1")
#set text(font: ("Microsoft YaHei", "Consolas"), size: 10pt, lang: "zh")
#set par(justify: true, leading: 0.65em)
#set heading(numbering: "1.1")
#set table(inset: 6pt, stroke: 0.4pt + luma(75%))
#show raw: set text(font: ("Consolas", "Microsoft YaHei"), size: 8.5pt)
#show table.cell.where(y: 0): set text(weight: "bold")

#align(center)[
  #text(size: 21pt, weight: "bold")[Everything.db 数据格式]

  ESDb 1.7.20 · 汇编逆向与全样本验证
]

*本机 Everything.db 为自有二进制格式；已恢复名称、层次、大小、修改时间及尾部索引，并精确解析至文件末尾。*
本文针对 Everything 1.4.1.1032 生成的 ESDb 1.7.20；其他版本不得直接套用。原程序与原数据库未修改。

= 样本与验证范围

#table(
  columns: (1fr, 3fr),
  [项目], [信息],
  [主程序], [`Everything.exe`，1.4.1.1032，PE32/x86],
  [数据库版本], [`0x01070014`，即 1.7.20],
  [数据库大小], [388,320,589 字节],
  [目录与文件数], [1,046,436 个目录；8,228,251 个文件],
  [来源], [5 个 NTFS 来源；5 个文件夹索引来源],
  [验证方法], [主程序反汇编、全样本解析、与本机 `db2efu.exe` 的小样本对照],
)

文件位置：
```text
原程序：C:\Program Files (x86)\Everything\Everything.exe
原数据库：C:\Users\hydro\AppData\Local\Everything\Everything.db
研究目录：C:\Users\hydro\Everything-db-research
```

数据库副本 SHA-256：
```text
390642242fdc1934c4c33e9e76244601f2e47a72d06a42e47d1f5ce485a43bc3
```

*验证边界：* NTFS 与文件夹来源已做全样本验证；文件列表和 ReFS 仅有汇编证据。解析器检查边界、层次环及索引引用，但未完整验证排序比较规则和排列唯一性。

= 编码与总体布局

*所有定长整数均为小端，区域连续写入，不要求 4 或 8 字节对齐。*

```text
u8  = 1 字节无符号整数
u32 = 4 字节无符号整数
u64 = 8 字节无符号整数
P(n) = n < 255 时为 u8(n)；否则为 FF + u32(n)
S(s) = P(字节长度) + 字符串字节，不保存结尾 NUL
```

`P` 不是 LEB128。例如 255 编码为 `FF FF 00 00 00`，256 编码为 `FF 00 01 00 00`。

```text
文件头（20 字节）
来源表
排除开关与三张过滤表
目录父索引数组
目录记录区
文件记录区
NTFS / ReFS 目录 FRN 索引表
目录快速排序表
文件快速排序表
EOF
```

字符串主要采用 UTF-8；样本中 145 条名称包含代理区码点，不能全部按严格 UTF-8 解码。名称使用 `errors="surrogatepass"`；长度与名称差分均按*字节*计算。

辅助程序在 `0x409852` 检测 `BZh9`，从文件头重新进行 bzip2 解压，再读取数据库流。因此压缩包装不是 ESDb 内部的逐记录压缩。本机样本未压缩；压缩变体尚未做动态交叉验证。

= 文件头与 flags

#table(
  columns: (auto, auto, 1fr),
  [偏移], [类型], [含义],
  [`0x00`], [4 字节], [ASCII `ESDb`；小端整数 `0x62445345`],
  [`0x04`], [`u32`], [版本 `0x01070014`；高 8 位、中 8 位、低 16 位分别为 1、7、20],
  [`0x08`], [`u32`], [属性与排序表标志 `flags`],
  [`0x0C`], [`u32`], [目录数 `D`],
  [`0x10`], [`u32`], [文件数 `F`],
  [`0x14`], [变长], [来源表起点],
)

== 属性标志

*属性块只保存启用的字段，未启用字段不占空间。* 字段顺序为大小、创建时间、修改时间、访问时间、Windows 属性。

#table(
  columns: (auto, 1fr, auto),
  [标志], [含义], [类型],
  [`0x0001`], [文件大小], [`u64`],
  [`0x0002`], [创建时间，文件与目录], [`u64`],
  [`0x0004`], [修改时间，文件与目录], [`u64`],
  [`0x0008`], [访问时间，文件与目录], [`u64`],
  [`0x0010`], [Windows 属性，文件与目录], [`u32`],
  [`0x0020`], [目录大小], [`u64`],
)

时间采用 Windows FILETIME：自 1601-01-01 00:00:00 UTC 起的 100 ns 计数。显示时需另行转换时区；字段未记录时区。100 ns 是*存储分辨率*，不保证底层文件系统实际具有同等时间精度。

== 快速排序标志

#table(
  columns: (auto, 1fr),
  [标志], [对应排序表],
  [`0x0100`], [大小],
  [`0x0200`], [创建时间],
  [`0x0400`], [修改时间],
  [`0x0800`], [访问时间],
  [`0x1000`], [Windows 属性],
  [`0x2000`], [路径],
  [`0x4000`], [扩展名，仅文件],
)

当前样本 `flags = 0x2525`：启用文件大小、目录大小、修改时间，以及大小、修改时间、路径快速排序。
文件与目录的属性块均为 `u64 size + u64 modified`，共 16 字节。

*不能恢复的属性：* 当前样本未保存创建时间、访问时间、隐藏／只读等 Windows 属性，不能将缺席字段解释为 0。
旧 `db2efu` 日志把 `0x02/0x04` 解释为隐藏／系统排除，属于过时标签；1.7.20 应以实际读写汇编为准。

= 来源与过滤配置

*来源表描述卷或扫描入口；记录通过父索引间接关联来源。*

```text
P(source_count)
重复 source_count 次：
    P(type)
    u8(out_of_date)
    类型相关数据
```

#table(
  columns: (auto, auto, 1fr),
  [type], [来源], [类型相关字段],
  [0], [NTFS], [`S(guid), S(path), S(root), S(include_only), u64(journal_id), u64(next_usn)`],
  [1], [文件列表], [`S(path), u64(时间字段)`；未做实样验证],
  [2], [文件夹索引], [`S(path), u64(next_update)`],
  [3], [ReFS], [4 个 `S` 字段与 2 个 `u64`，类似 NTFS；未做实样验证],
)

本机 NTFS 来源为 C、D、E、F、G；文件夹来源为 I、O、X、Y、Z。
来源表中的卷路径、卷 GUID、USN journal ID 和 next USN 已恢复；文件列表、ReFS 的字段语义仍需独立样本核实。

来源表后紧接以下配置：
```text
u8(exclude_flags)
排除目录：P(count) + 重复 [u8(pattern_type) + S(pattern)]
仅包含文件：同上
排除文件：同上
```

样本 `exclude_flags = 0`，三张过滤表数量为 13、0、1。
过滤模式已经规范化，例如配置 `*.LNK` 存为 `type=5, pattern=".lnk"`。
`pattern_type` 是匹配类型，不是 Windows 文件属性；各类型的完整匹配规则尚未恢复。

= 目录层次与名称差分

== 父索引空间

配置后保存 `u32 parents[D]`，与后续目录记录一一对应。

#table(
  columns: (auto, 1fr),
  [父索引 p], [解释],
  [`p < D`], [父目录索引],
  [`D <= p < D + source_count`], [来源编号 `p - D`；对应根目录],
  [其他值], [无效索引],
)

父目录不保证排在子目录前，应先读取完整父索引数组，再解析来源或重建路径。需要检测越界和层次环。

== 名称差分

*目录区、文件区分别维护前一名称；进入新区时重置为空字节串。*

```text
suffix_len = P()
if suffix_len == 0:
    name = previous
else:
    backtrack = P()
    suffix = read(suffix_len)
    name = previous[:len(previous) - backtrack] + suffix
previous = name
```

`backtrack` 为删去前一名称末尾的*字节数*，不是共同前缀长度。必须检查其不超过前一名称长度。
例如 `abcdef` 变成 `abcdXYZ` 编码为 `03 02 58 59 5A`；重复名称仅编码为 `00`，此时没有 backtrack 字段。

== 目录记录

```text
名称差分
按 flags 写入目录属性块：
    [u64 size]       条件：0x0020
    [u64 created]    条件：0x0002
    [u64 modified]   条件：0x0004
    [u64 accessed]   条件：0x0008
    [u32 attributes] 条件：0x0010
若来源为 NTFS：u64 FRN
若来源为 ReFS：16 字节文件 ID（仅汇编确认）
```

目录的 parent 不重复保存在此记录中，而是来自前面的 parents 数组。
NTFS FRN 是文件引用号；样本已恢复其原始 64 位值。文件夹扫描来源没有这个附加字段。

根目录记录的名称已含盘符，例如 `C:`。恢复路径时沿 parents 拼接名称，*不再添加来源表的 path*；否则会错误地出现 `C:\C:\...`。

= 文件记录与属性实例

*每条文件记录先保存父索引，再保存名称差分和属性块。*

```text
u32 parent
名称差分
按 flags 写入文件属性块：
    [u64 size]       条件：0x0001
    [u64 created]    条件：0x0002
    [u64 modified]   条件：0x0004
    [u64 accessed]   条件：0x0008
    [u32 attributes] 条件：0x0010
```

parent 使用与目录相同的索引空间。已验证的 NTFS／文件夹来源中，普通文件记录*不附带 NTFS FRN*。

已恢复的属性示例，时间按 UTC+08:00 显示：

#table(
  columns: (auto, 1fr, auto, auto),
  [类型], [名称], [大小／字节], [修改时间],
  [文件], [` 2. Cadenas.ipynb`], [5,600], [2019-09-08 19:24:29],
  [目录], [`!v`], [43,435], [2021-09-15 07:22:44],
  [目录], [`#!123`], [104,264], [2022-05-24 10:32:04],
)

文件示例的完整路径与原始时间值：
```text
E:\github\julia\JuliaBoxTutorials\introductory-tutorials\
intro-to-julia-ES\ 2. Cadenas.ipynb
modified = 132124154693945455
```
以上为排版而换行，实际路径中没有换行符；文件名开头的空格是真实内容，不应 trim。

= 尾部索引

== FRN 索引表

文件记录结束后写入两张目录索引表：
```text
u32 NTFS_count
u32 NTFS_folder_indexes[NTFS_count]
u32 ReFS_count
u32 ReFS_folder_indexes[ReFS_count]
```

两张表均引用*目录索引*，不是文件索引或最近修改列表。
样本数量为 868,664、0；第一张数量与五个 NTFS 来源的目录总数一致。FRN 表的内部排序规则尚未完整反推。

== 快速排序表

*排序表不保存额外数量，由 D、F 和 flags 决定长度。* 写入顺序为：

+ 目录：路径 → 大小 → 创建时间 → 修改时间 → 访问时间 → 属性。
+ 文件：路径 → 大小 → 创建时间 → 修改时间 → 访问时间 → 属性 → 扩展名。

仅写启用的表。每张目录表有 D 个 u32，每张文件表有 F 个 u32，分别引用相应记录的索引；目录大小表还要求 `0x0020` 开启。
当前样本保存三张目录表和三张文件表，即路径、大小、修改时间排序。

== 实测区域边界

#table(
  columns: (1fr, auto, auto),
  [区域], [起始偏移], [字节数],
  [文件头、来源与配置], [`0x00000000`], [640],
  [目录父索引], [`0x00000280`], [4,185,744],
  [目录记录], [`0x003FE110`], [30,496,561],
  [文件记录], [`0x02113841`], [238,866,736],
  [两张 FRN 表], [`0x104E0971`], [3,474,664],
  [目录路径排序], [`0x10830E59`], [4,185,744],
  [目录大小排序], [`0x10C2ECE9`], [4,185,744],
  [目录修改时间排序], [`0x1102CB79`], [4,185,744],
  [文件路径排序], [`0x1142AA09`], [32,913,004],
  [文件大小排序], [`0x1338E075`], [32,913,004],
  [文件修改时间排序], [`0x152F16E1`], [32,913,004],
)

所有区域字节数之和为 388,320,589，与文件大小一致，未留下未解析尾部。
本样本未发现额外页结构、对齐填充或独立的 ESDb 尾部校验字段；不能据此推断所有版本均如此。

= 汇编依据与复现

*关键字段由主程序读写路径确认，辅助转换器用于对照。* 以下地址均为 ImageBase `0x00400000` 下的静态 VA；ASLR 后须换算实际地址。

#table(
  columns: (auto, 1fr),
  [地址], [证据],
  [`0x40A6EC / 0x40A713`], [Everything 检查魔数与版本],
  [`0x40E131`], [Everything 写入文件头],
  [`0x403DE0 / 0x403E00 / 0x403E60`], [写 u32、紧凑整数、长度前缀字符串],
  [`0x48E2F0 / 0x48E3F0`], [计算属性布局、从配置生成 flags],
  [`0x40E3F4 / 0x40E5BD`], [写目录名称差分、文件父索引及差分],
  [`0x40E637 / 0x40E6A5`], [写 NTFS、ReFS 目录 FRN 索引],
  [`0x40E707 / 0x40E9EB`], [写目录、文件快速排序表],
  [`db2efu 0x409A60 / 0x404C20`], [分派 1.7.20、执行解码],
)

研究目录内的主要产物：
```text
code/inspect_db.py       只读解析器，Python 标准库
asm/Everything.asm      主程序反汇编
asm/db2efu.asm           辅助程序反汇编
data/inspection.json    全样本结构、属性与路径示例
data/sha256.txt         样本与程序哈希
data/test.db            人工构造的小样本
data/test.efu           db2efu 对照输出
data/db2efu-test.log    小样本转换日志
```

复现命令，工作目录为研究目录：
```powershell
python code/inspect_db.py --test
python code/inspect_db.py data/Everything.snapshot.db > data/inspection.json
```

自检覆盖紧凑整数、名称回退、异常输入、盘符根目录、中文名称、重复名称、属性块及排序表。
人工小样本的四条路径、大小与修改时间均已与 db2efu 输出逐项对照一致。
全样本解析约 30 秒，解析器只读访问数据库并要求精确 EOF；目前仅输出少量路径样例，*尚未全量导出路径清单*。

= 未确认部分

*以下内容不应视为已恢复的通用格式保证。*

- 其他版本：辅助程序支持的 `EZDB 1.6.6/1.6.7` 与 `ESDb 1.7.5/1.7.8/1.7.17` 使用独立分派，不能混用布局。
- 文件列表与 ReFS：缺乏实样；解析器主动拒绝这两种来源。旧转换器的文件列表分支还读取逐记录附加字节，需另行核对。
- 压缩包装：已定位 bzip2 解码分支，尚未完成压缩样本的动态交叉验证。
- 过滤匹配类型、排序比较器、FRN 表排序规则尚未完整恢复。
- 未实现数据库写回；不能将只读解析成功等同于写回兼容性。

数据库与解析结果包含本机路径信息，宜留在本地，不宜公开上传。
