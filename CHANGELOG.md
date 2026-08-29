# Changelog

## [0.1.0] - 2026-08-29

首个可用版本（MVP）。

### Added

- 统一 TUI 主菜单：开始新记录 / 历史会话回放 / 设置，全部键盘操作
- 实时录制仪表盘：进程 Top N 表、CPU / GPU / 内存 / 功耗曲线、电源事件日志、catppuccin 主题
- 历史回放（`inspect`）：会话列表 → 时间窗口平移 / 缩放 → 单进程随时间曲线
- 设置界面与持久化（`~/Library/Application Support/neko-perf/config.toml`），录制中 `o` 键修改，间隔 / Top N 立即生效
- 电源事件：IOKit 睡眠 / 唤醒精确时间戳、`pmset -g assertions` 变化快照
- GPU / 功耗 / 温度采集：IOReport 免 sudo（基于 macmon），失败优雅降级
- 内存统计：与活动监视器同口径（App + Wired + Compressed），含压缩 / Inactive / Free / Swap 明细
- CLI 子命令：`record` / `daemon` / `inspect` / `report` / `sessions` / `events`
- 报表导出：table / csv / json / md

### Fixed

- 进程 CPU 采样全为 0（sysinfo 需 `refresh_processes` 才会刷新进程 CPU）
- 工具自身占用 100% CPU（电源事件线程 `CFRunLoopRunInMode` 忙轮询，改为阻塞式 run loop + 低频定时器）
- 内存读数与活动监视器不一致（改用 `host_statistics64` + `vm.swapusage` 同口径计算）
- `CFRunLoopRunInMode` 使用 `kCFRunLoopCommonModes` 报错（伪模式不能直接 run）

### Notes

- 仅支持 Apple Silicon（Intel 无 IOReport GPU 通道）
- 逐进程 GPU 归因无公共 API，进程表为 CPU / 内存维度

