# neko-perf

> Apple Silicon 性能记录器：记录 CPU / 内存 / GPU / 功耗 / 温度与电源事件，统一 TUI 回放与设置。

![macOS](https://img.shields.io/badge/macOS-Apple%20Silicon-black?logo=apple&logoColor=white)
![Rust](https://img.shields.io/badge/Rust-1.88%2B-orange?logo=rust&logoColor=white)
![License](https://img.shields.io/badge/License-MIT-blue)

`neko-perf` 解决一个很具体的问题：**关掉显示器后 Mac mini 发烫，但活动监视器看不到"休眠期间"是谁在吃性能**。

它会在关屏 / 休眠前启动，持续采样并把数据落盘；之后用一个 TUI 回放任意时间段的系统与进程曲线，定位热源。**不需要 sudo**，单二进制，零运行时依赖。

## 特性

- **免 sudo 全量遥测**：GPU 活动率 / 频率 / 功耗、CPU / ANE / 整机功耗、温度均通过 IOReport 读取（与 powermetrics 同源）
- **内存与活动监视器同口径**：内存已用 = App + Wired + Compressed，另含压缩内存、Inactive、Free、Swap 总量 / 已用明细
- **电源事件**：系统进入睡眠的精确时间戳、唤醒时间、被阻止睡眠（`pmset -g assertions` 变化快照）
- **统一 TUI**：主菜单 → 开始新记录 / 历史会话回放 / 设置，全部键盘操作
- **历史回放**：会话列表 → 时间窗口平移 / 缩放 → 单进程随时间曲线
- **设置持久化**：采样间隔、Top N、GPU 开关、主题、数据库路径保存到配置文件；录制中按 `o` 修改，间隔与 Top N **立即生效**
- **catppuccin 主题**：latte / frappe / macchiato / mocha 四套配色，`t` 键切换
- **CLI 与 TUI 等价**：`record` / `daemon` / `inspect` / `report` / `sessions` / `events` 全部保留

## 环境要求

- macOS（Apple Silicon，M 系列）
- 构建需要 [Rust](https://rustup.rs) 1.88+（运行不需要任何运行时）

## 安装与构建

```bash
cargo build --release
# 产物：target/release/neko-perf（约 3 MB，Mach-O arm64，可拷到任意 M 芯片 Mac 直接运行）

# 可选：放进 PATH
mkdir -p ~/bin && cp target/release/neko-perf ~/bin/
```

## 快速开始

```bash
# 1. 直接进入统一 TUI（主菜单）
./target/release/neko-perf

# 2. 典型排查流程
./target/release/neko-perf record --db ~/neko-perf.db   # 关屏前开始记录
# …… 关屏 / 休眠，回来之后按 q 结束 ……
./target/release/neko-perf inspect --db ~/neko-perf.db  # TUI 回放：找热源

# 3. 其他常用命令
./target/release/neko-perf daemon --db ~/neko-perf.db --interval 5000   # 无 TUI 后台采集
./target/release/neko-perf report --db ~/neko-perf.db --format md       # 时间段聚合报表
./target/release/neko-perf sessions --db ~/neko-perf.db                 # 会话列表
./target/release/neko-perf events --db ~/neko-perf.db                   # 电源事件
```

### 界面预览（ASCII）

```text
┌─ neko-perf ── 会话 3 ── 已运行 02:31:04 ── 采样 9,064 ──────────────┐
│ 电源: awake   CPU: 12.4%   GPU: 3.1%   温度: 48°C   DB: ~/perf.db │
├────────────────────────────────┬──────────────────────────────────┤
│  PID   NAME          CPU%   MEM │  CPU ──────────────             │
│ 1234  WindowServer   18.2   1.2G│  GPU ──                        │
│ 2310  coreaudiod      8.1  240M │  MEM ───────────                │
├────────────────────────────────┴──────────────────────────────────┤
│ 20:00:01 sleep   (assertion: WindowServer PreventUserIdleSystemSleep)│
│ 20:05:33 wake                                                     │
└────────────────────────────────────────────────────────────────────┘
```

## 命令参考

| 命令 | 作用 |
| --- | --- |
| `neko-perf`（无参数） | 进入统一 TUI 主菜单 |
| `neko-perf hub --db <路径>` | 同上，指定数据库 |
| `neko-perf record` | 直接进入录制（TUI，启动时可改采样间隔） |
| `neko-perf daemon` | 无 TUI 后台采集，Ctrl+C 停止 |
| `neko-perf inspect` | 历史数据 TUI 回放 |
| `neko-perf report --format table\|csv\|json\|md` | 时间段聚合报表 |
| `neko-perf sessions` | 列出会话 |
| `neko-perf events` | 列出电源事件 |

## TUI 快捷键

| 界面 | 键 | 作用 |
| --- | --- | --- |
| 主菜单 | `↑` `↓` / `j` `k` | 选择菜单项 |
| 主菜单 | `Enter` | 确认（开始记录 / 回放 / 设置） |
| 会话列表 | `Enter` | 回放选中的会话 |
| 会话列表 | `q` / `Esc` | 返回主菜单 |
| 记录 | `Enter` | 确认设置并开始记录 |
| 记录 | `o` | 打开设置（间隔 / Top N 立即生效） |
| 记录 | `Space` | 暂停 / 继续采样显示 |
| 记录 | `s` / `t` / `/` / `?` | 排序 / 主题 / 过滤 / 帮助 |
| 记录 / 检查 | `q` | 退出（记录时自动保存并结束会话） |
| 检查 | `←` `→` | 平移时间窗口 |
| 检查 | `+` `-` | 缩放时间窗口 |
| 检查 | `Enter` | 查看选中进程随时间的曲线 |
| 检查 | `r` `s` `t` | 重置窗口 / 切换排序 / 主题 |

## 配置

设置界面（主菜单或录制中 `o`）修改后保存到：

```text
~/Library/Application Support/neko-perf/config.toml
```

```toml
interval_ms = 2000          # 系统与 GPU 采样间隔
process_interval_ms = 2000  # 进程表采样间隔
top_n = 15                  # 记录 Top N 进程
gpu = true                  # GPU / 功耗 / 温度采集
theme = "mocha"             # latte / frappe / macchiato / mocha
db_path = "neko-perf.db"    # SQLite 路径
```

CLI 显式参数优先于配置文件；配置文件不存在时使用内置默认值。

## 数据存储

SQLite（WAL 模式），四张表：

| 表 | 内容 |
| --- | --- |
| `sessions` | 会话（起止时间、机型、总内存） |
| `system_samples` | 系统级采样：CPU / GPU / 功耗 / 温度 / 内存明细 / Swap |
| `process_samples` | 进程级采样：PID、进程名、CPU%、内存 |
| `power_events` | 电源事件：sleep / wake / will_not_sleep / assertion_changed |

估算：1s 系统采样 + 2s 进程采样（Top 15），24 小时约 50–80 MB。可用 SQLite 自带 `sqlite3` 直接查询。

## 工作原理

```text
进程采样线程 (sysinfo) ──┐
SoC 采样线程 (macmon/IOReport) ─┼─► SQLite（WAL）
电源事件线程 (IOKit + pmset) ──┘
              │
TUI 实时仪表盘 ◄── channel ──┘
              │
inspect / report ◄── 查询 SQLite
```

- **GPU / 功耗 / 温度**：IOReport 私有框架（免 sudo），同一数据源与 `powermetrics` 一致；采样失败时优雅降级为 `N/A`，不影响 CPU 采集
- **电源事件**：IOKit `IORegisterForSystemPower` 接收睡眠 / 唤醒通知（免 sudo），`pmset -g assertions` 每 60s 快照一次，内容变化才落库
- **内存口径**：`host_statistics64` + `vm.swapusage` 直接计算，与活动监视器一致
- **关键认知**：关屏 ≠ 睡眠。Mac 可能在 dark wake 或阻止睡眠状态；工具记录的是"关屏前 → 唤醒后"全时间线，真正睡眠时进程挂起、采样自然中断（属预期，用电源事件分段分析）

## 已知限制

- **逐进程 GPU 归因**：Apple 公共 API 不提供（活动监视器使用私有接口），进程表为 CPU / 内存维度，GPU 为系统级
- **IOReport 是私有框架**：未来 macOS 版本可能变更，失败时自动降级
- **仅支持 Apple Silicon**：Intel Mac 无 IOReport GPU 通道

## 开发

```bash
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo test
cargo build --release
```

```text
nekoBackendPerformance/
├── src/
│   ├── main.rs        # 入口与命令分发
│   ├── cli.rs         # clap 参数定义
│   ├── config.rs      # 配置加载 / 持久化
│   ├── hub.rs         # 统一 TUI 主菜单
│   ├── tui.rs         # 录制仪表盘、设置表单、历史回放
│   ├── sampler.rs     # 进程与 SoC 采样线程
│   ├── macos_mem.rs   # macOS 内存统计（活动监视器口径）
│   ├── power.rs       # IOKit 电源事件 + assertions
│   ├── record.rs      # 采集编排（线程、SQLite 写入）
│   ├── storage.rs     # SQLite schema 与查询
│   ├── report.rs      # 报表聚合
│   ├── theme.rs       # catppuccin 调色板
│   └── model.rs       # 数据模型
├── docs/开发文档.md   # 详细设计与开发计划
└── .github/workflows/ci.yml
```

## 技术栈

Rust · [ratatui](https://ratatui.rs)（TUI）· [sysinfo](https://github.com/GuillaumeGomez/sysinfo)（进程）· [macmon](https://github.com/vladkens/macmon)（IOReport）· [rusqlite](https://github.com/rusqlite/rusqlite)（SQLite bundled）· IOKit · [clap](https://github.com/clap-rs/clap)

## 许可证

[MIT](LICENSE)
