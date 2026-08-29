use chrono::{DateTime, Local};

/// 系统级采样（含 SoC 指标），对应 system_samples 表。
#[derive(Debug, Clone)]
pub struct SystemSample {
    pub ts: DateTime<Local>,
    pub power_state: String,
    pub cpu_total_pct: f32,
    pub cpu_active_ratio: Option<f32>,
    pub cpu_power_w: Option<f32>,
    pub gpu_active_ratio: Option<f32>,
    pub gpu_freq_mhz: Option<f32>,
    pub gpu_power_w: Option<f32>,
    pub ane_power_w: Option<f32>,
    pub package_power_w: Option<f32>,
    pub cpu_temp_c: Option<f32>,
    pub gpu_temp_c: Option<f32>,
    pub ram_total_bytes: u64,
    pub ram_used_bytes: u64, // 活动监视器口径：App + Wired + Compressed
    pub mem_app_bytes: u64,
    pub mem_wired_bytes: u64,
    pub mem_compressed_bytes: u64,
    pub mem_inactive_bytes: u64,
    pub mem_free_bytes: u64,
    pub swap_total_bytes: u64,
    pub swap_used_bytes: u64,
}

/// 进程级采样，对应 process_samples 表。
#[derive(Debug, Clone)]
pub struct ProcessSample {
    pub ts: DateTime<Local>,
    pub pid: i32,
    pub name: String,
    pub cpu_pct: f32,
    pub mem_bytes: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PowerEventKind {
    Sleep,
    Wake,
    WillNotSleep,
    AssertionChanged,
}

impl PowerEventKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            PowerEventKind::Sleep => "sleep",
            PowerEventKind::Wake => "wake",
            PowerEventKind::WillNotSleep => "will_not_sleep",
            PowerEventKind::AssertionChanged => "assertion_changed",
        }
    }
}

#[derive(Debug, Clone)]
pub struct PowerEvent {
    pub ts: DateTime<Local>,
    pub kind: PowerEventKind,
    pub detail: String,
}

/// 采集线程之间的消息。
#[derive(Debug)]
pub enum SampleMsg {
    System(SystemSample),
    Processes(Vec<ProcessSample>),
    Power(PowerEvent),
    SocUnavailable(String),
    Stop,
}

/// 写入/采集共享的轻量上下文（进程线程更新，系统线程读取）。
#[derive(Debug, Clone, Default)]
pub struct SharedCtx {
    pub cpu_total_pct: f32,
    pub ram_total_bytes: u64,
    pub ram_used_bytes: u64,
    pub mem_app_bytes: u64,
    pub mem_wired_bytes: u64,
    pub mem_compressed_bytes: u64,
    pub mem_inactive_bytes: u64,
    pub mem_free_bytes: u64,
    pub swap_total_bytes: u64,
    pub swap_used_bytes: u64,
    pub power_state: String,
}

pub fn now() -> DateTime<Local> {
    Local::now()
}

pub fn now_iso() -> String {
    now().to_rfc3339_opts(chrono::SecondsFormat::Millis, false)
}

pub fn parse_iso(s: &str) -> Option<DateTime<Local>> {
    chrono::DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|dt| dt.with_timezone(&Local))
}
