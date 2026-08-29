use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};

use crate::config::AppConfig;

#[derive(Parser)]
#[command(
    name = "neko-perf",
    version,
    about = "Apple Silicon 性能记录器：记录 CPU/GPU/内存与电源事件，统一 TUI 回放与设置",
    long_about = "不带参数进入统一 TUI：主菜单（开始新记录 / 历史会话回放 / 设置）。\n\
                  也可直接用子命令：record（录制）、daemon（后台采集）、inspect（历史检查）、\n\
                  report（报表）、sessions（会话列表）、events（电源事件）。"
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Subcommand)]
pub enum Command {
    /// 统一 TUI 主菜单（默认命令）
    Hub(HubArgs),
    /// 直接进入 TUI 交互录制
    Record(RecordArgs),
    /// 无 TUI 后台采集（适合 tmux/nohup）
    Daemon(DaemonArgs),
    /// 停止记录后，用 TUI 检查历史数据
    Inspect(InspectArgs),
    /// 时间段聚合报告（table/csv/json/md）
    Report(ReportArgs),
    /// 列出电源事件
    Events(EventsArgs),
    /// 列出会话
    Sessions(SessionsArgs),
}

#[derive(Args, Clone, Default)]
pub struct HubArgs {
    /// 数据库路径（默认读取设置）
    #[arg(long)]
    pub db: Option<PathBuf>,
}

#[derive(Args, Clone)]
pub struct RecordArgs {
    /// 采样间隔（毫秒），TUI 启动时仍可修改
    #[arg(long)]
    pub interval: Option<u64>,
    /// 进程表采样间隔（毫秒）
    #[arg(long)]
    pub process_interval: Option<u64>,
    /// 进程表行数
    #[arg(long)]
    pub top_n: Option<usize>,
    /// 数据库路径
    #[arg(long)]
    pub db: Option<PathBuf>,
    /// 关闭 GPU/功耗/温度采集
    #[arg(long)]
    pub no_gpu: bool,
    /// 初始 catppuccin 主题：latte/frappe/macchiato/mocha
    #[arg(long)]
    pub theme: Option<String>,
}

impl RecordArgs {
    /// 合并 CLI 参数与已保存设置，CLI 显式参数优先。
    pub fn merged_config(&self, saved: &AppConfig) -> AppConfig {
        AppConfig {
            interval_ms: self.interval.unwrap_or(saved.interval_ms),
            process_interval_ms: self.process_interval.unwrap_or(saved.process_interval_ms),
            top_n: self.top_n.unwrap_or(saved.top_n),
            gpu: if self.no_gpu { false } else { saved.gpu },
            theme: self.theme.clone().unwrap_or_else(|| saved.theme.clone()),
            db_path: self.db.clone().unwrap_or_else(|| saved.db_path.clone()),
        }
    }
}

#[derive(Args, Clone)]
pub struct DaemonArgs {
    /// 采样间隔（毫秒）
    #[arg(long)]
    pub interval: Option<u64>,
    /// 进程表采样间隔（毫秒）
    #[arg(long)]
    pub process_interval: Option<u64>,
    /// 进程表行数
    #[arg(long)]
    pub top_n: Option<usize>,
    /// 数据库路径
    #[arg(long)]
    pub db: Option<PathBuf>,
    /// 关闭 GPU/功耗/温度采集
    #[arg(long)]
    pub no_gpu: bool,
}

impl DaemonArgs {
    pub fn merged_config(&self, saved: &AppConfig) -> AppConfig {
        AppConfig {
            interval_ms: self.interval.unwrap_or(saved.interval_ms),
            process_interval_ms: self.process_interval.unwrap_or(saved.process_interval_ms),
            top_n: self.top_n.unwrap_or(saved.top_n),
            gpu: if self.no_gpu { false } else { saved.gpu },
            theme: saved.theme.clone(),
            db_path: self.db.clone().unwrap_or_else(|| saved.db_path.clone()),
        }
    }
}

#[derive(Args, Clone)]
pub struct InspectArgs {
    /// 数据库路径
    #[arg(long, default_value = "neko-perf.db")]
    pub db: PathBuf,
    /// 会话 ID（默认最新会话）
    #[arg(long)]
    pub session: Option<i64>,
    /// 起始时间（ISO8601，默认会话开始）
    #[arg(long)]
    pub from: Option<String>,
    /// 结束时间（ISO8601，默认会话结束）
    #[arg(long)]
    pub to: Option<String>,
}

#[derive(Args, Clone)]
pub struct ReportArgs {
    /// 数据库路径
    #[arg(long, default_value = "neko-perf.db")]
    pub db: PathBuf,
    /// 会话 ID（默认最新会话）
    #[arg(long)]
    pub session: Option<i64>,
    /// 起始时间（ISO8601）
    #[arg(long)]
    pub from: Option<String>,
    /// 结束时间（ISO8601）
    #[arg(long)]
    pub to: Option<String>,
    /// Top N 进程
    #[arg(long, default_value_t = 20)]
    pub top: usize,
    /// 输出格式：table/csv/json/md
    #[arg(long, default_value = "table")]
    pub format: String,
}

#[derive(Args, Clone)]
pub struct EventsArgs {
    /// 数据库路径
    #[arg(long, default_value = "neko-perf.db")]
    pub db: PathBuf,
    /// 会话 ID（默认最新会话）
    #[arg(long)]
    pub session: Option<i64>,
    /// 起始时间（ISO8601）
    #[arg(long)]
    pub from: Option<String>,
    /// 结束时间（ISO8601）
    #[arg(long)]
    pub to: Option<String>,
}

#[derive(Args, Clone)]
pub struct SessionsArgs {
    /// 数据库路径
    #[arg(long, default_value = "neko-perf.db")]
    pub db: PathBuf,
}
