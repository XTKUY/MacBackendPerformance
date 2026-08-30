# Changelog

## [Unreleased]

### Added

- 历史回放进程列表支持滚动：`↑/↓` 选择、`PgUp/PgDn` 翻页、`Home/End` 跳到首尾，窗口内所有采样到的进程都能显示
- 回放进程聚合不再有 1000 行上限，按窗口加载全部进程
- 历史回放新增睡眠事件窗口（`v` 键）：列出本会话全部睡眠事件的入睡 / 唤醒时间与持续时长；优先用 IOKit 记录的 sleep/wake 事件配对，旧会话自动按采样空窗推断
- 会话列表支持删除历史会话（`d` 键弹出确认窗口），删除后索引与数据文件一并移除
- 数据按会话独立存档：每个会话写入同目录 `sessions/<会话ID>.db`，主库只保留会话索引；旧版本单库数据首次读取时自动拆分迁移

### Fixed

- 电源断言每分钟刷一条：变化检测前剥掉 `pmset` 输出里的时长字段（倒计时 / 累计时长），只有断言集合真正变化才记录
- `pmset -g assertions` 增加 10 秒超时，避免采样线程被挂住
- IOReport（GPU / 功耗 / 温度）采样改为工作线程 + 超时接收，采样卡住时跳过本拍、退出不再等待（曾出现 ~18 分钟空窗）
- IOKit 电源监听修复：`system will sleep` 时正确调用 `IOAllowPowerChange` 应答；通知源同时挂到 default mode 与 common modes；注册失败时输出错误日志（此前 sleep / wake 事件一条都没有记录）

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
