use std::collections::VecDeque;
use std::io;
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};

use chrono::{DateTime, Local};
use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style, Stylize};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Cell, Clear, Paragraph, Row, Sparkline, Table, Wrap};
use ratatui::Frame;

use crate::cli::{InspectArgs, RecordArgs};
use crate::config::AppConfig;
use crate::model::{parse_iso, PowerEvent, PowerEventKind, ProcessSample, SampleMsg, SystemSample};
use crate::record::{RecordConfig, Recorder, RuntimeConfig};
use crate::report::{resolve_bounds, resolve_session};
use crate::storage::{self, PowerRow, ProcAgg, SessionInfo, SystemRow};
use crate::theme::{Flavor, Palette};

// ---------------------------------------------------------------- utils

fn fmt_bytes(b: u64) -> String {
    if b >= 1 << 30 {
        format!("{:.1}G", b as f64 / (1 << 30) as f64)
    } else if b >= 1 << 20 {
        format!("{:.0}M", b as f64 / (1 << 20) as f64)
    } else {
        format!("{b}B")
    }
}

fn fmt_elapsed(d: Duration) -> String {
    let s = d.as_secs();
    format!("{:02}:{:02}:{:02}", s / 3600, (s % 3600) / 60, s % 60)
}

fn fmt_dur(d: chrono::Duration) -> String {
    let s = d.num_seconds().max(0);
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, (s % 3600) / 60, s % 60)
    } else {
        format!("{:02}:{:02}", s / 60, s % 60)
    }
}

fn parse_hms(s: &str) -> i64 {
    s.split(':').rev().enumerate().fold(0i64, |acc, (i, p)| {
        acc + p.parse::<i64>().unwrap_or(0) * 60i64.pow(i as u32)
    })
}

fn fmt_secs(s: i64) -> String {
    let s = s.max(0);
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, (s % 3600) / 60, s % 60)
    } else {
        format!("{:02}:{:02}", s / 60, s % 60)
    }
}

fn spark<'a>(title: String, data: &[u64], max: u64, style: Style, pal: Palette) -> Sparkline<'a> {
    Sparkline::default()
        .data(data)
        .max(max.max(1))
        .style(style)
        .block(pal.block(title))
}

fn draw_background(frame: &mut Frame, pal: Palette) {
    frame.render_widget(
        Block::default().style(Style::new().bg(pal.bg)),
        frame.area(),
    );
}

fn centered_area(area: Rect, percent_x: u16, percent_y: u16) -> Rect {
    let pop = Layout::vertical([
        Constraint::Percentage((100 - percent_y) / 2),
        Constraint::Percentage(percent_y),
        Constraint::Percentage((100 - percent_y) / 2),
    ])
    .split(area)[1];
    Layout::horizontal([
        Constraint::Percentage((100 - percent_x) / 2),
        Constraint::Percentage(percent_x),
        Constraint::Percentage((100 - percent_x) / 2),
    ])
    .split(pop)[1]
}

// ---------------------------------------------------------------- record

#[derive(Clone, Copy, PartialEq)]
enum SortKey {
    Cpu,
    Mem,
    Pid,
}

struct FormField {
    label: &'static str,
    value: String,
    hint: &'static str,
}

pub struct SetupForm {
    fields: Vec<FormField>,
    active: usize,
    error: Option<String>,
}

pub enum FormResult {
    None,
    Submit(AppConfig),
    Cancel,
}

impl SetupForm {
    pub fn new(cfg: &AppConfig) -> SetupForm {
        SetupForm {
            fields: vec![
                FormField {
                    label: "采样间隔（毫秒）",
                    value: cfg.interval_ms.to_string(),
                    hint: "如 1000 / 5000 / 10000，影响系统与 GPU 曲线密度",
                },
                FormField {
                    label: "进程采样间隔（毫秒）",
                    value: cfg.process_interval_ms.to_string(),
                    hint: "进程表更新频率，建议 ≥ 采样间隔",
                },
                FormField {
                    label: "进程表行数（Top N）",
                    value: cfg.top_n.to_string(),
                    hint: "只记录最占 CPU 的 N 个进程",
                },
                FormField {
                    label: "GPU / 功耗 / 温度采集（y/n）",
                    value: if cfg.gpu { "y" } else { "n" }.to_string(),
                    hint: "免 sudo，通过 IOReport 读取",
                },
                FormField {
                    label: "主题（latte/frappe/macchiato/mocha）",
                    value: cfg.theme.clone(),
                    hint: "catppuccin 四套配色",
                },
                FormField {
                    label: "数据库路径",
                    value: cfg.db_path.display().to_string(),
                    hint: "SQLite 文件位置",
                },
            ],
            active: 0,
            error: None,
        }
    }

    pub fn handle(&mut self, key: KeyCode) -> FormResult {
        match key {
            KeyCode::Up | KeyCode::BackTab => {
                self.active = if self.active == 0 {
                    self.fields.len() - 1
                } else {
                    self.active - 1
                };
                self.error = None;
            }
            KeyCode::Down | KeyCode::Tab => {
                self.active = (self.active + 1) % self.fields.len();
                self.error = None;
            }
            KeyCode::Char(c) => {
                let f = &mut self.fields[self.active];
                let ok = match f.label {
                    "GPU / 功耗 / 温度采集（y/n）" => {
                        c == 'y' || c == 'n' || c == 'Y' || c == 'N'
                    }
                    "主题（latte/frappe/macchiato/mocha）" => c.is_ascii_alphabetic(),
                    "数据库路径" => c.is_ascii_graphic() || c == ' ',
                    _ => c.is_ascii_digit(),
                };
                if ok && f.value.len() < 200 {
                    f.value.push(c);
                }
            }
            KeyCode::Backspace => {
                self.fields[self.active].value.pop();
            }
            KeyCode::Enter => match self.submit() {
                Ok(cfg) => return FormResult::Submit(cfg),
                Err(e) => self.error = Some(e),
            },
            KeyCode::Esc => return FormResult::Cancel,
            _ => {}
        }
        FormResult::None
    }

    fn submit(&self) -> Result<AppConfig, String> {
        let parse = |i: usize| -> Result<u64, String> {
            self.fields[i]
                .value
                .parse::<u64>()
                .map_err(|_| format!("{} 需要是数字", self.fields[i].label))
        };
        let interval = parse(0)?;
        let process_interval = parse(1)?;
        let top_n = parse(2)? as usize;
        let gpu = matches!(
            self.fields[3].value.to_ascii_lowercase().as_str(),
            "y" | "yes"
        );
        let theme = self.fields[4].value.trim().to_string();
        if Flavor::from_name(&theme).is_none() {
            return Err("主题无效：latte / frappe / macchiato / mocha".to_string());
        }
        let db_path = PathBuf::from(self.fields[5].value.trim());
        if db_path.as_os_str().is_empty() {
            return Err("数据库路径不能为空".to_string());
        }
        if !(100..=3_600_000).contains(&interval) {
            return Err("采样间隔需在 100–3600000 毫秒之间".to_string());
        }
        if !(200..=3_600_000).contains(&process_interval) {
            return Err("进程采样间隔需在 200–3600000 毫秒之间".to_string());
        }
        if !(1..=100).contains(&top_n) {
            return Err("Top N 需在 1–100 之间".to_string());
        }
        Ok(AppConfig {
            interval_ms: interval,
            process_interval_ms: process_interval,
            top_n,
            gpu,
            theme,
            db_path,
        })
    }

    pub fn draw(&self, frame: &mut Frame, pal: Palette, title: &str, footer: &str) {
        draw_background(frame, pal);
        let area = centered_area(frame.area(), 62, 52);
        frame.render_widget(Clear, area);
        let inner = Block::bordered()
            .title(title.to_string().bold())
            .title_style(pal.title_style())
            .border_style(pal.border_style())
            .style(pal.bg_style());
        let inner_area = inner.inner(area);
        frame.render_widget(inner, area);

        let mut lines = vec![Line::from(" ")];
        for (i, f) in self.fields.iter().enumerate() {
            let arrow = if i == self.active { "▸ " } else { "  " };
            let label = Span::styled(format!("{arrow}{}", f.label), pal.text_style());
            let value = Span::styled(
                f.value.clone(),
                Style::new()
                    .fg(pal.blue)
                    .bg(pal.surface)
                    .add_modifier(Modifier::BOLD),
            );
            let hint = Span::styled(format!("  {}", f.hint), pal.sub_style());
            lines.push(Line::from(vec![label, value, hint]));
            lines.push(Line::from(" "));
        }
        if let Some(e) = &self.error {
            lines.push(Line::from(Span::styled(e, Style::new().fg(pal.red).bold())));
            lines.push(Line::from(" "));
        }
        lines.push(Line::from(vec![Span::styled(footer, pal.sub_style())]));
        frame.render_widget(
            Paragraph::new(Text::from(lines)).wrap(Wrap { trim: false }),
            inner_area,
        );
    }
}

struct LiveState {
    theme: Flavor,
    cfg: AppConfig,
    runtime: Arc<RuntimeConfig>,
    session_id: i64,
    started: Instant,
    samples: u64,
    paused: bool,
    sort: SortKey,
    filter: String,
    filter_input: bool,
    help: bool,
    gpu_ok: bool,
    soc_msg: Option<String>,
    latest: Option<SystemSample>,
    processes: Vec<ProcessSample>,
    events: VecDeque<PowerEvent>,
    cpu_hist: VecDeque<f32>,
    gpu_hist: VecDeque<f32>,
    mem_hist: VecDeque<f32>,
    pwr_hist: VecDeque<f32>,
    db: String,
    settings: Option<SetupForm>,
}

impl LiveState {
    fn new(session_id: i64, db: String, cfg: AppConfig, runtime: Arc<RuntimeConfig>) -> LiveState {
        let theme = Flavor::from_name(&cfg.theme).unwrap_or(Flavor::Mocha);
        let gpu_ok = cfg.gpu;
        LiveState {
            theme,
            cfg,
            runtime,
            session_id,
            started: Instant::now(),
            samples: 0,
            paused: false,
            sort: SortKey::Cpu,
            filter: String::new(),
            filter_input: false,
            help: false,
            gpu_ok,
            soc_msg: None,
            latest: None,
            processes: Vec::new(),
            events: VecDeque::new(),
            cpu_hist: VecDeque::new(),
            gpu_hist: VecDeque::new(),
            mem_hist: VecDeque::new(),
            pwr_hist: VecDeque::new(),
            db,
            settings: None,
        }
    }

    fn open_settings(&mut self) {
        self.settings = Some(SetupForm::new(&self.cfg));
    }

    fn apply_settings(&mut self, cfg: AppConfig) {
        self.runtime
            .interval_ms
            .store(cfg.interval_ms, Ordering::Relaxed);
        self.runtime
            .process_interval_ms
            .store(cfg.process_interval_ms, Ordering::Relaxed);
        self.runtime.top_n.store(cfg.top_n, Ordering::Relaxed);
        self.theme = Flavor::from_name(&cfg.theme).unwrap_or(self.theme);
        self.cfg = cfg.clone();
        self.settings = None;
        if let Err(e) = crate::config::save(&cfg) {
            eprintln!("[neko-perf] 保存设置失败: {e}");
        }
    }

    fn handle_msg(&mut self, msg: SampleMsg) {
        if self.paused {
            return;
        }
        match msg {
            SampleMsg::System(s) => {
                self.latest = Some(s.clone());
                self.cpu_hist.push_back(s.cpu_total_pct);
                if let Some(g) = s.gpu_active_ratio {
                    self.gpu_hist.push_back(g * 100.0);
                }
                self.mem_hist
                    .push_back(s.ram_used_bytes as f32 / (1 << 30) as f32);
                if let Some(p) = s.package_power_w {
                    self.pwr_hist.push_back(p);
                }
                for h in [
                    &mut self.cpu_hist,
                    &mut self.gpu_hist,
                    &mut self.mem_hist,
                    &mut self.pwr_hist,
                ] {
                    while h.len() > 2400 {
                        h.pop_front();
                    }
                }
            }
            SampleMsg::Processes(rows) => {
                self.processes = rows;
                self.samples += 1;
            }
            SampleMsg::Power(ev) => {
                self.events.push_back(ev);
                while self.events.len() > 200 {
                    self.events.pop_front();
                }
            }
            SampleMsg::SocUnavailable(m) => {
                self.gpu_ok = false;
                self.soc_msg = Some(m);
            }
            SampleMsg::Stop => {}
        }
    }

    fn sorted(&self) -> Vec<ProcessSample> {
        let mut v: Vec<ProcessSample> = self
            .processes
            .iter()
            .filter(|p| {
                self.filter.is_empty()
                    || p.name.to_lowercase().contains(&self.filter.to_lowercase())
            })
            .cloned()
            .collect();
        match self.sort {
            SortKey::Cpu => v.sort_by(|a, b| b.cpu_pct.total_cmp(&a.cpu_pct)),
            SortKey::Mem => v.sort_by_key(|a| std::cmp::Reverse(a.mem_bytes)),
            SortKey::Pid => v.sort_by_key(|p| p.pid),
        }
        v
    }

    fn draw(&mut self, frame: &mut Frame) {
        if let Some(form) = &self.settings {
            form.draw(
                frame,
                Palette::of(self.theme),
                "设置（运行中）",
                "Enter 保存 · Tab/方向键 切换 · Esc 关闭（GPU/DB 下次录制生效）",
            );
            return;
        }
        let pal = Palette::of(self.theme);
        draw_background(frame, pal);
        let [hdr, mid, foot] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Min(0),
            Constraint::Length(7),
        ])
        .areas(frame.area());

        // 顶部状态栏
        let (cpu, gpu, temp, state, state_color) = match &self.latest {
            Some(s) => (
                s.cpu_total_pct,
                s.gpu_active_ratio.map(|g| g * 100.0),
                s.cpu_temp_c,
                s.power_state.clone(),
                match s.power_state.as_str() {
                    "asleep" => pal.red,
                    _ => pal.green,
                },
            ),
            None => (0.0, None, None, "awake".to_string(), pal.green),
        };
        let mut spans = vec![
            Span::styled(
                " neko-perf ",
                Style::new().fg(pal.mantle).bg(pal.mauve).bold(),
            ),
            Span::styled(format!(" 会话 {}", self.session_id), pal.title_style()),
            Span::styled(
                format!(" 已运行 {}", fmt_elapsed(self.started.elapsed())),
                pal.text_style(),
            ),
            Span::styled(format!(" 采样 {}", self.samples), pal.text_style()),
            Span::styled(
                format!(" 电源 {}", state),
                Style::new().fg(state_color).bg(pal.bg),
            ),
            Span::styled(format!(" CPU {cpu:.1}%"), pal.blue),
        ];
        match gpu {
            Some(g) => spans.push(Span::styled(format!(" GPU {g:.1}%"), pal.teal)),
            None => spans.push(Span::styled(" GPU N/A", pal.overlay)),
        }
        if let Some(t) = temp {
            spans.push(Span::styled(format!(" {t:.0}°C"), pal.peach));
        }
        if let Some(s) = &self.latest {
            if s.swap_total_bytes > 0 {
                spans.push(Span::styled(
                    format!(
                        " Swap {}/{}",
                        fmt_bytes(s.swap_used_bytes),
                        fmt_bytes(s.swap_total_bytes)
                    ),
                    pal.yellow,
                ));
            }
        }
        spans.push(Span::styled(
            format!(" 主题 {}", self.theme.name()),
            pal.pink,
        ));
        spans.push(Span::styled(format!("  {}", self.db), pal.subtext));
        frame.render_widget(Paragraph::new(Line::from(spans)), hdr);

        // 中部：进程表 + 曲线
        let [left, right] =
            Layout::horizontal([Constraint::Percentage(58), Constraint::Percentage(42)]).areas(mid);

        let title = if self.filter.is_empty() {
            format!(
                "进程 Top {}（按 {} 排序）",
                self.processes.len(),
                sort_name(self.sort)
            )
        } else {
            format!("进程过滤: {}", self.filter)
        };
        let header = Row::new(vec!["PID", "进程", "CPU%", "内存"]).style(pal.title_style());
        let rows: Vec<Row> = self
            .sorted()
            .iter()
            .map(|p| {
                let cpu_style = if p.cpu_pct > 80.0 {
                    pal.red
                } else if p.cpu_pct > 40.0 {
                    pal.peach
                } else if p.cpu_pct > 10.0 {
                    pal.yellow
                } else {
                    pal.text
                };
                Row::new(vec![
                    Cell::from(p.pid.to_string()).style(pal.sub_style()),
                    Cell::from(p.name.clone()).style(pal.text_style()),
                    Cell::from(format!("{:.1}", p.cpu_pct)).style(Style::new().fg(cpu_style)),
                    Cell::from(fmt_bytes(p.mem_bytes)).style(pal.sub_style()),
                ])
            })
            .collect();
        let table = Table::new(
            rows,
            [
                Constraint::Length(7),
                Constraint::Fill(1),
                Constraint::Length(7),
                Constraint::Length(9),
            ],
        )
        .header(header)
        .column_spacing(1)
        .block(pal.block(title));
        frame.render_widget(table, left);

        // 右侧曲线
        let cpu_v: Vec<u64> = self.cpu_hist.iter().map(|v| (*v * 100.0) as u64).collect();
        let gpu_v: Vec<u64> = self.gpu_hist.iter().map(|v| (*v * 100.0) as u64).collect();
        let mem_v: Vec<u64> = self.mem_hist.iter().map(|v| *v as u64).collect();
        let pwr_max = self
            .pwr_hist
            .iter()
            .fold(0.0_f32, |a, b| a.max(*b))
            .max(1.0) as u64;
        let pwr_v: Vec<u64> = self.pwr_hist.iter().map(|v| *v as u64).collect();

        let charts = Layout::vertical([
            Constraint::Length(4),
            Constraint::Length(4),
            Constraint::Length(4),
            Constraint::Length(4),
            Constraint::Min(0),
        ])
        .split(right);

        let cpu_cur = self.cpu_hist.back().copied().unwrap_or(0.0);
        let gpu_cur = self.gpu_hist.back().copied().unwrap_or(0.0);
        let mem_cur = self.mem_hist.back().copied().unwrap_or(0.0);
        let (mem_total, mem_comp) = self
            .latest
            .as_ref()
            .map(|s| (s.ram_total_bytes, s.mem_compressed_bytes))
            .unwrap_or((0, 0));
        let pwr_cur = self.pwr_hist.back().copied().unwrap_or(0.0);

        frame.render_widget(
            spark(
                format!("CPU  {cpu_cur:.1}%"),
                &cpu_v,
                100,
                Style::new().fg(pal.blue),
                pal,
            ),
            charts[0],
        );
        if self.gpu_ok {
            frame.render_widget(
                spark(
                    format!("GPU  {gpu_cur:.1}%"),
                    &gpu_v,
                    100,
                    Style::new().fg(pal.teal),
                    pal,
                ),
                charts[1],
            );
        } else {
            frame.render_widget(
                spark(
                    "GPU  N/A".to_string(),
                    &[],
                    1,
                    Style::new().fg(pal.overlay),
                    pal,
                ),
                charts[1],
            );
        }
        frame.render_widget(
            spark(
                format!(
                    "内存  {} / {} · 压缩 {}",
                    fmt_bytes((mem_cur * (1 << 30) as f32) as u64),
                    fmt_bytes(mem_total),
                    fmt_bytes(mem_comp)
                ),
                &mem_v,
                mem_v.iter().copied().max().unwrap_or(1).max(1),
                Style::new().fg(pal.mauve),
                pal,
            ),
            charts[2],
        );
        frame.render_widget(
            spark(
                format!("功耗  {pwr_cur:.1}W"),
                &pwr_v,
                pwr_max,
                Style::new().fg(pal.peach),
                pal,
            ),
            charts[3],
        );
        if let Some(m) = &self.soc_msg {
            frame.render_widget(
                Paragraph::new(m.as_str())
                    .style(pal.sub_style())
                    .wrap(Wrap { trim: true }),
                charts[4],
            );
        }

        // 底部事件日志
        let mut lines: Vec<Line> = self
            .events
            .iter()
            .rev()
            .take(5)
            .map(|e| event_line(e, pal))
            .collect();
        lines.push(Line::from(" "));
        lines.push(Line::from(vec![Span::styled(
            "q 退出 · Space 暂停 · s 排序 · t 主题 · / 过滤 · ? 帮助",
            pal.sub_style(),
        )]));
        frame.render_widget(
            Paragraph::new(Text::from(lines)).block(pal.block("电源事件")),
            foot,
        );

        // 覆盖层
        if self.filter_input {
            let pop = centered_area(frame.area(), 46, 20);
            frame.render_widget(Clear, pop);
            frame.render_widget(
                Paragraph::new(Line::from(vec![
                    Span::styled("过滤进程名: ", pal.text_style()),
                    Span::styled(self.filter.clone(), Style::new().fg(pal.blue).bold()),
                ]))
                .block(
                    Block::bordered()
                        .border_style(pal.border_style())
                        .style(pal.bg_style()),
                ),
                pop,
            );
        } else if self.help {
            let pop = centered_area(frame.area(), 60, 55);
            frame.render_widget(Clear, pop);
            let help = vec![
                Line::from(Span::styled("快捷键", pal.title_style())),
                Line::from(" "),
                Line::from(Span::styled(
                    "q          退出（自动保存并结束会话）",
                    pal.text_style(),
                )),
                Line::from(Span::styled(
                    "Space      暂停 / 继续采样显示",
                    pal.text_style(),
                )),
                Line::from(Span::styled(
                    "s          切换排序：CPU / 内存 / PID",
                    pal.text_style(),
                )),
                Line::from(Span::styled(
                    "t          切换 catppuccin 主题",
                    pal.text_style(),
                )),
                Line::from(Span::styled("/          过滤进程名", pal.text_style())),
                Line::from(Span::styled(
                    "o          打开设置（间隔 / TopN 立即生效）",
                    pal.text_style(),
                )),
                Line::from(Span::styled("?          关闭本帮助", pal.text_style())),
                Line::from(" "),
                Line::from(Span::styled(
                    "数据保存在 SQLite，之后可运行：",
                    pal.sub_style(),
                )),
                Line::from(Span::styled("  neko-perf inspect --db <路径>", pal.blue)),
                Line::from(Span::styled(
                    "  neko-perf report --db <路径> --format md",
                    pal.blue,
                )),
            ];
            frame.render_widget(
                Paragraph::new(Text::from(help))
                    .block(
                        Block::bordered()
                            .border_style(pal.border_style())
                            .style(pal.bg_style()),
                    )
                    .wrap(Wrap { trim: false }),
                pop,
            );
        }
    }
}

fn sort_name(s: SortKey) -> &'static str {
    match s {
        SortKey::Cpu => "CPU%",
        SortKey::Mem => "内存",
        SortKey::Pid => "PID",
    }
}

fn event_line(e: &PowerEvent, pal: Palette) -> Line<'static> {
    let color = match e.kind {
        PowerEventKind::Sleep => pal.red,
        PowerEventKind::Wake => pal.green,
        PowerEventKind::WillNotSleep => pal.yellow,
        PowerEventKind::AssertionChanged => pal.blue,
    };
    let ts = e.ts.format("%H:%M:%S").to_string();
    let mut detail = e.detail.clone();
    if detail.len() > 110 {
        detail.truncate(110);
        detail.push('…');
    }
    Line::from(vec![
        Span::styled(format!("{ts} "), pal.overlay),
        Span::styled(format!("{:<18}", e.kind.as_str()), Style::new().fg(color)),
        Span::styled(detail, pal.subtext),
    ])
}

pub fn run_record(args: &RecordArgs) -> io::Result<()> {
    let saved = crate::config::load();
    let initial = args.merged_config(&saved);
    let mut terminal = ratatui::init();
    let result = run_record_terminal(&mut terminal, &initial);
    ratatui::restore();
    result
}

/// 在给定 terminal 上运行完整录制流程（表单 → 录制 → 退出），供 CLI 与主菜单复用。
pub fn run_record_terminal(
    terminal: &mut ratatui::DefaultTerminal,
    initial: &AppConfig,
) -> io::Result<()> {
    let form_theme = Flavor::from_name(&initial.theme).unwrap_or(Flavor::Mocha);
    let mut form = SetupForm::new(initial);
    let cfg = loop {
        terminal.draw(|f| {
            form.draw(
                f,
                Palette::of(form_theme),
                "开始记录前设置",
                "Enter 确认 · Tab/方向键 切换字段 · Esc 取消",
            )
        })?;
        if !event::poll(Duration::from_millis(200))? {
            continue;
        }
        if let Event::Key(k) = event::read()? {
            if k.kind != KeyEventKind::Press {
                continue;
            }
            match form.handle(k.code) {
                FormResult::Submit(c) => break c,
                FormResult::Cancel => return Ok(()),
                FormResult::None => {}
            }
        }
    };
    if let Err(e) = crate::config::save(&cfg) {
        eprintln!("[neko-perf] 保存设置失败: {e}");
    }
    let runtime = RuntimeConfig::new(cfg.interval_ms, cfg.process_interval_ms, cfg.top_n);
    let mut rc = RecordConfig::from_app_config(&cfg);
    rc.note = Some(crate::record::session_started_note(&rc));
    let db = cfg.db_path.display().to_string();
    let recorder = match Recorder::start(rc, runtime.clone()) {
        Ok(r) => r,
        Err(e) => return Err(io::Error::other(e)),
    };
    let session_id = recorder.session_id;
    let mut state = LiveState::new(session_id, db.clone(), cfg, runtime.clone());
    let mut quit = false;

    while !quit {
        // 收采样消息（丢弃积压，保留最新）
        while let Ok(msg) = recorder.ui_rx.try_recv() {
            state.handle_msg(msg);
        }
        terminal.draw(|f| state.draw(f))?;

        if event::poll(Duration::from_millis(100))? {
            if let Event::Key(k) = event::read()? {
                if k.kind != KeyEventKind::Press {
                    continue;
                }
                if let Some(form) = state.settings.as_mut() {
                    match form.handle(k.code) {
                        FormResult::Submit(c) => state.apply_settings(c),
                        FormResult::Cancel => state.settings = None,
                        FormResult::None => {}
                    }
                    continue;
                }
                if state.filter_input {
                    match k.code {
                        KeyCode::Esc | KeyCode::Enter => state.filter_input = false,
                        KeyCode::Backspace => {
                            state.filter.pop();
                        }
                        KeyCode::Char(c) => state.filter.push(c),
                        _ => {}
                    }
                    continue;
                }
                if state.help {
                    match k.code {
                        KeyCode::Esc | KeyCode::Char('?') | KeyCode::Char('q') => {
                            state.help = false
                        }
                        _ => {}
                    }
                    continue;
                }
                match k.code {
                    KeyCode::Char('q') => quit = true,
                    KeyCode::Esc => quit = true,
                    KeyCode::Char(' ') => state.paused = !state.paused,
                    KeyCode::Char('s') => {
                        state.sort = match state.sort {
                            SortKey::Cpu => SortKey::Mem,
                            SortKey::Mem => SortKey::Pid,
                            SortKey::Pid => SortKey::Cpu,
                        }
                    }
                    KeyCode::Char('t') => state.theme = state.theme.next(),
                    KeyCode::Char('o') => state.open_settings(),
                    KeyCode::Char('/') => state.filter_input = true,
                    KeyCode::Char('?') => state.help = true,
                    _ => {}
                }
            }
        }
    }

    recorder.stop();
    println!(
        "已结束会话 {}（{} 次采样），数据保存在 {db}",
        session_id, state.samples
    );
    Ok(())
}

// ---------------------------------------------------------------- inspect

struct InspectState<'a> {
    conn: &'a rusqlite::Connection,
    info: SessionInfo,
    full_from: DateTime<Local>,
    full_to: DateTime<Local>,
    win_from: DateTime<Local>,
    win_to: DateTime<Local>,
    events: Vec<PowerRow>,
    sys: Vec<SystemRow>,
    agg: Vec<ProcAgg>,
    sel_idx: usize,
    scroll: usize,
    vis_rows: usize,
    sel_pid: Option<i64>,
    sel_name: String,
    sel_series: Vec<ProcSeriesView>,
    sort_peak: bool,
    sleep_episodes: Vec<SleepEpisode>,
    show_sleep: bool,
    sleep_scroll: usize,
    sleep_vis: usize,
    theme: Flavor,
    cpu_s: Vec<u64>,
    gpu_s: Vec<u64>,
    mem_s: Vec<u64>,
    pwr_s: Vec<u64>,
    pwr_max: u64,
}

struct ProcSeriesView {
    cpu_pct: f64,
}

/// 一次睡眠事件：入睡时间 → 唤醒时间 → 持续时长。
struct SleepEpisode {
    start: String,
    end: String,
    dur: String,
    source: &'static str,
}

impl<'a> InspectState<'a> {
    fn new(
        conn: &'a rusqlite::Connection,
        info: SessionInfo,
        from: String,
        to: String,
        events: Vec<PowerRow>,
        sys: Vec<SystemRow>,
    ) -> InspectState<'a> {
        let full_from = parse_iso(&from).unwrap_or_else(Local::now);
        let full_to = parse_iso(&to).unwrap_or_else(Local::now);
        let mut st = InspectState {
            conn,
            info,
            full_from,
            full_to,
            win_from: full_from,
            win_to: full_to,
            events,
            sys,
            agg: Vec::new(),
            sel_idx: 0,
            scroll: 0,
            vis_rows: 20,
            sel_pid: None,
            sel_name: String::new(),
            sel_series: Vec::new(),
            sort_peak: false,
            sleep_episodes: Vec::new(),
            show_sleep: false,
            sleep_scroll: 0,
            sleep_vis: 20,
            theme: Flavor::Mocha,
            cpu_s: Vec::new(),
            gpu_s: Vec::new(),
            mem_s: Vec::new(),
            pwr_s: Vec::new(),
            pwr_max: 1,
        };
        st.recalc();
        st.sleep_episodes = st.build_sleep_episodes();
        st
    }

    fn iso(&self, dt: DateTime<Local>) -> String {
        dt.to_rfc3339_opts(chrono::SecondsFormat::Millis, false)
    }

    fn recalc(&mut self) {
        let from = self.iso(self.win_from);
        let to = self.iso(self.win_to);
        let mut agg =
            storage::process_aggregate(self.conn, self.info.id, &from, &to, 0).unwrap_or_default();
        if self.sort_peak {
            agg.sort_by(|a, b| b.peak_cpu.total_cmp(&a.peak_cpu));
        }
        self.agg = agg;
        self.sel_idx = self.sel_idx.min(self.agg.len().saturating_sub(1));
        self.scroll = self.scroll.min(self.agg.len().saturating_sub(1));

        self.cpu_s.clear();
        self.gpu_s.clear();
        self.mem_s.clear();
        self.pwr_s.clear();
        let total = self.info.ram_total_bytes.unwrap_or(0).max(1) as f64;
        for r in &self.sys {
            if let Some(v) = r.cpu_total_pct {
                self.cpu_s.push((v * 100.0).clamp(0.0, 10000.0) as u64);
            }
            if let Some(v) = r.gpu_active_ratio {
                self.gpu_s.push((v * 10000.0).clamp(0.0, 10000.0) as u64);
            }
            if let Some(v) = r.ram_used_bytes {
                self.mem_s
                    .push(((v as f64 / total) * 10000.0).clamp(0.0, 10000.0) as u64);
            }
            if let Some(v) = r.package_power_w {
                self.pwr_s.push((v * 10.0) as u64);
            }
        }
        self.pwr_max = self.pwr_s.iter().copied().max().unwrap_or(10).max(10);

        if let Some(pid) = self.sel_pid {
            let rows = storage::process_series(self.conn, self.info.id, pid, &from, &to)
                .unwrap_or_default();
            self.sel_series = rows
                .into_iter()
                .map(|r| ProcSeriesView { cpu_pct: r.cpu_pct })
                .collect();
        }
    }

    fn window_len(&self) -> i64 {
        (self.win_to - self.win_from).num_seconds().max(1)
    }

    fn shift(&mut self, frac: f64) {
        let step = (self.window_len() as f64 * frac) as i64;
        let span = (self.full_to - self.full_from).num_seconds();
        let from = (self.win_from - chrono::Duration::seconds(step))
            .timestamp()
            .clamp(self.full_from.timestamp(), self.full_to.timestamp() - 1);
        let to = (from + span).min(self.full_to.timestamp());
        self.win_from = DateTime::from_timestamp(from, 0)
            .map(|d| d.with_timezone(&Local))
            .unwrap_or(self.full_from);
        self.win_to = DateTime::from_timestamp(to, 0)
            .map(|d| d.with_timezone(&Local))
            .unwrap_or(self.full_to);
        self.recalc();
    }

    fn zoom(&mut self, factor: f64) {
        let span = (self.full_to - self.full_from).num_seconds().max(1);
        let center = self.win_from.timestamp() + self.window_len() / 2;
        let mut len = (self.window_len() as f64 * factor) as i64;
        len = len.clamp(2, span);
        let mut from = center - len / 2;
        from = from.clamp(self.full_from.timestamp(), self.full_to.timestamp() - len);
        let to = (from + len).min(self.full_to.timestamp());
        self.win_from = DateTime::from_timestamp(from, 0)
            .map(|d| d.with_timezone(&Local))
            .unwrap_or(self.full_from);
        self.win_to = DateTime::from_timestamp(to, 0)
            .map(|d| d.with_timezone(&Local))
            .unwrap_or(self.full_to);
        self.recalc();
    }

    fn reset_window(&mut self) {
        self.win_from = self.full_from;
        self.win_to = self.full_to;
        self.recalc();
    }

    fn toggle_select(&mut self) {
        if let Some(p) = self.agg.get(self.sel_idx) {
            if self.sel_pid == Some(p.pid) {
                self.sel_pid = None;
                self.sel_series.clear();
            } else {
                self.sel_pid = Some(p.pid);
                self.sel_name = p.name.clone();
                self.recalc();
            }
        }
    }

    /// 整理本会话的全部睡眠事件（整个会话范围，不受时间窗口影响）。
    /// 优先用 IOKit 记录的 sleep/wake 事件配对；旧会话没有这些事件时，
    /// 退回到按 system_samples 的空窗（>10 秒）推断睡眠时段。
    fn build_sleep_episodes(&self) -> Vec<SleepEpisode> {
        let mut eps: Vec<SleepEpisode> = Vec::new();

        let mut open: Option<(DateTime<Local>, DateTime<Local>)> = None;
        for e in &self.events {
            match e.event_type.as_str() {
                "sleep" => {
                    if let Some(dt) = parse_iso(&e.ts) {
                        open = Some((dt, dt));
                    }
                }
                "wake" => {
                    if let Some((st, _)) = open.take() {
                        if let Some(en) = parse_iso(&e.ts) {
                            eps.push(SleepEpisode {
                                start: st.format("%H:%M:%S").to_string(),
                                end: en.format("%H:%M:%S").to_string(),
                                dur: fmt_dur(en - st),
                                source: "IOKit",
                            });
                        }
                    }
                }
                _ => {}
            }
        }
        if !eps.is_empty() {
            return eps;
        }

        // 旧会话：按采样空窗推断。真正睡眠时进程挂起、采样中断，
        // 唤醒后恢复，因此空窗起止时间近似等于睡眠时段。
        let mut prev: Option<DateTime<Local>> = None;
        for r in &self.sys {
            if let Some(dt) = parse_iso(&r.ts) {
                if let Some(p) = prev {
                    let gap = dt - p;
                    if gap.num_seconds() >= 10 {
                        eps.push(SleepEpisode {
                            start: p.format("%H:%M:%S").to_string(),
                            end: dt.format("%H:%M:%S").to_string(),
                            dur: fmt_dur(gap),
                            source: "采样空窗推断",
                        });
                    }
                }
                prev = Some(dt);
            }
        }
        eps
    }

    fn draw(&mut self, frame: &mut Frame) {
        let pal = Palette::of(self.theme);
        draw_background(frame, pal);
        let [hdr, mid, foot] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Min(0),
            Constraint::Length(6),
        ])
        .areas(frame.area());

        let header = Line::from(vec![
            Span::styled(
                " neko-perf inspect ",
                Style::new().fg(pal.mantle).bg(pal.teal).bold(),
            ),
            Span::styled(
                format!(
                    " 会话 {} · {}  ",
                    self.info.id,
                    self.info.model.as_deref().unwrap_or("Apple Silicon")
                ),
                pal.title_style(),
            ),
            Span::styled(
                format!(
                    "窗口 {} → {}（{}s）",
                    self.win_from.format("%m-%d %H:%M:%S"),
                    self.win_to.format("%m-%d %H:%M:%S"),
                    self.window_len()
                ),
                pal.text_style(),
            ),
            Span::styled(format!(" 主题 {}", self.theme.name()), pal.pink),
        ]);
        frame.render_widget(Paragraph::new(header), hdr);

        let [left, right] =
            Layout::horizontal([Constraint::Percentage(55), Constraint::Percentage(45)]).areas(mid);

        // 左：聚合进程表（可滚动，保证窗口内所有采样进程都能看到）
        let vis = (left.height as usize).saturating_sub(3).max(1);
        self.vis_rows = vis;
        let total = self.agg.len();
        self.scroll = self.scroll.min(total.saturating_sub(vis));
        if self.sel_idx < self.scroll {
            self.scroll = self.sel_idx;
        } else if self.sel_idx >= self.scroll + vis {
            self.scroll = self.sel_idx + 1 - vis;
        }
        let scroll = self.scroll;
        let header =
            Row::new(vec!["PID", "进程", "平均%", "峰值%", "样本"]).style(pal.title_style());
        let rows: Vec<Row> = self
            .agg
            .iter()
            .skip(scroll)
            .take(vis)
            .enumerate()
            .map(|(vi, p)| {
                let i = scroll + vi;
                let sel = self.sel_pid == Some(p.pid);
                let row_style = if i == self.sel_idx {
                    Style::new().fg(pal.text).bg(pal.surface)
                } else {
                    pal.text_style()
                };
                let mark = if sel { "● " } else { "  " };
                Row::new(vec![
                    Cell::from(format!("{mark}{}", p.pid)).style(row_style),
                    Cell::from(p.name.clone()).style(row_style),
                    Cell::from(format!("{:.1}", p.avg_cpu)).style(row_style),
                    Cell::from(format!("{:.1}", p.peak_cpu)).style(Style::new().fg(pal.peach)),
                    Cell::from(p.samples.to_string()).style(pal.sub_style()),
                ])
            })
            .collect();
        let sort_txt = if self.sort_peak { "峰值" } else { "平均" };
        let title = if total > vis {
            format!("进程聚合（按 {sort_txt} CPU 排序） {}/{total}", scroll + 1)
        } else {
            format!("进程聚合（按 {sort_txt} CPU 排序）")
        };
        let table = Table::new(
            rows,
            [
                Constraint::Length(8),
                Constraint::Fill(1),
                Constraint::Length(7),
                Constraint::Length(8),
                Constraint::Length(7),
            ],
        )
        .header(header)
        .column_spacing(1)
        .block(pal.block(title));
        frame.render_widget(table, left);

        // 右：系统曲线 + 选中进程曲线
        let chart_h = if self.sel_pid.is_some() { 3 } else { 4 };
        let mut cons = vec![Constraint::Length(4); chart_h];
        cons.push(Constraint::Min(0));
        let charts = Layout::vertical(cons).split(right);
        let mut i = 0;
        frame.render_widget(
            spark(
                "CPU %".to_string(),
                &self.cpu_s,
                10000,
                Style::new().fg(pal.blue),
                pal,
            ),
            charts[i],
        );
        i += 1;
        frame.render_widget(
            spark(
                "GPU %".to_string(),
                &self.gpu_s,
                10000,
                Style::new().fg(pal.teal),
                pal,
            ),
            charts[i],
        );
        i += 1;
        frame.render_widget(
            spark(
                "内存 %".to_string(),
                &self.mem_s,
                10000,
                Style::new().fg(pal.mauve),
                pal,
            ),
            charts[i],
        );
        i += 1;
        if self.sel_pid.is_some() {
            let ps: Vec<u64> = self
                .sel_series
                .iter()
                .map(|r| (r.cpu_pct * 100.0).clamp(0.0, 10000.0) as u64)
                .collect();
            frame.render_widget(
                spark(
                    format!("进程 {} CPU%", self.sel_name),
                    &ps,
                    10000,
                    Style::new().fg(pal.pink),
                    pal,
                ),
                charts[i],
            );
        } else {
            frame.render_widget(
                spark(
                    "功耗 W".to_string(),
                    &self.pwr_s,
                    self.pwr_max,
                    Style::new().fg(pal.peach),
                    pal,
                ),
                charts[i],
            );
        }

        // 底部事件
        let mut lines: Vec<Line> = self
            .events
            .iter()
            .rev()
            .take(4)
            .map(|e| {
                let color = match e.event_type.as_str() {
                    "sleep" => pal.red,
                    "wake" => pal.green,
                    "will_not_sleep" => pal.yellow,
                    _ => pal.blue,
                };
                Line::from(vec![
                    Span::styled(format!("{} ", e.ts), pal.overlay),
                    Span::styled(format!("{:<18}", e.event_type), Style::new().fg(color)),
                    Span::styled(e.detail.clone(), pal.subtext),
                ])
            })
            .collect();
        lines.push(Line::from(" "));
        lines.push(Line::from(vec![Span::styled(
            "←/→ 平移 · +/- 缩放 · r 全部 · Enter 选择进程 · ↑/↓ 选择 · PgUp/PgDn 翻页 · Home/End 首尾 · s 排序 · t 主题 · v 睡眠事件 · q 退出",
            pal.sub_style(),
        )]));
        frame.render_widget(
            Paragraph::new(Text::from(lines)).block(pal.block("电源事件")),
            foot,
        );

        if self.show_sleep {
            self.draw_sleep_popup(frame, pal);
        }
    }

    fn draw_sleep_popup(&mut self, frame: &mut Frame, pal: Palette) {
        let total_dur = self
            .sleep_episodes
            .iter()
            .fold(0i64, |acc, e| acc + parse_hms(&e.dur));
        let block = pal.block(format!(
            "睡眠事件 · 会话 {} · {} 次 · 合计 {}",
            self.info.id,
            self.sleep_episodes.len(),
            fmt_secs(total_dur)
        ));
        let pop = centered_area(frame.area(), 72, 70);
        frame.render_widget(Clear, pop);
        frame.render_widget(block.clone(), pop);
        let inner = block.inner(pop);

        let mut lines: Vec<Line> = Vec::new();
        if self.sleep_episodes.is_empty() {
            lines.push(Line::from(Span::styled(
                "未发现睡眠事件（该会话可能整晚未入睡）",
                pal.text_style(),
            )));
        } else {
            lines.push(Line::from(Span::styled(
                "入睡            唤醒            持续    来源",
                pal.title_style(),
            )));
            for e in &self.sleep_episodes {
                lines.push(Line::from(vec![
                    Span::styled(
                        format!("{}  →  {}   {}", e.start, e.end, e.dur),
                        pal.text_style(),
                    ),
                    Span::styled(format!("  [{}]", e.source), pal.overlay),
                ]));
            }
        }
        lines.push(Line::from(" "));
        lines.push(Line::from(Span::styled(
            "↑/↓ 滚动 · PgUp/PgDn 翻页 · Home/End 首尾 · q/Esc/v 关闭",
            pal.sub_style(),
        )));

        // 内容区 = 表头 + 数据 + 空行 + 提示；数据行按可视高度滚动。
        let vis = (inner.height as usize).saturating_sub(3).max(1);
        self.sleep_vis = vis;
        let data_start = 1;
        let data_end = lines.len().saturating_sub(2); // 去掉空行和提示行
        let data_len = data_end.saturating_sub(data_start);
        let max_scroll = data_len.saturating_sub(vis);
        self.sleep_scroll = self.sleep_scroll.min(max_scroll);
        let from = data_start + self.sleep_scroll;
        let to = (from + vis).min(data_end);

        let mut shown: Vec<Line> = lines[..data_start.min(lines.len())].to_vec();
        shown.extend(lines[from..to.max(from)].iter().cloned());
        shown.extend(lines[lines.len().saturating_sub(1)..].to_vec());

        frame.render_widget(
            Paragraph::new(Text::from(shown)).style(Style::new().bg(pal.bg)),
            inner,
        );
    }
}

pub fn run_inspect(args: &InspectArgs) -> io::Result<()> {
    let conn = rusqlite::Connection::open(&args.db)
        .map_err(|e| io::Error::other(format!("打开数据库失败: {e}")))?;
    let mut terminal = ratatui::init();
    let result = run_inspect_terminal(&mut terminal, &conn, args);
    ratatui::restore();
    result
}

/// 在给定 terminal 上运行历史检查（回放），供 CLI 与主菜单复用。
pub fn run_inspect_terminal(
    terminal: &mut ratatui::DefaultTerminal,
    conn: &rusqlite::Connection,
    args: &InspectArgs,
) -> io::Result<()> {
    let info = resolve_session(conn, args.session).map_err(io::Error::other)?;
    let (from, to) = resolve_bounds(&info, args.from.as_deref(), args.to.as_deref())
        .map_err(io::Error::other)?;
    let events = storage::power_events_range(conn, info.id, &from, &to)
        .map_err(|e| io::Error::other(e.to_string()))?;
    let sys = storage::system_series(conn, info.id, &from, &to)
        .map_err(|e| io::Error::other(e.to_string()))?;

    let mut state = InspectState::new(conn, info, from, to, events, sys);
    let mut quit = false;
    while !quit {
        terminal.draw(|f| state.draw(f))?;
        if !event::poll(Duration::from_millis(100))? {
            continue;
        }
        if let Event::Key(k) = event::read()? {
            if k.kind != KeyEventKind::Press {
                continue;
            }
            if state.show_sleep {
                // 睡眠事件窗口：方向键滚动，q/Esc/v 关闭。
                match k.code {
                    KeyCode::Char('q') | KeyCode::Esc | KeyCode::Char('v') => {
                        state.show_sleep = false
                    }
                    KeyCode::Up => state.sleep_scroll = state.sleep_scroll.saturating_sub(1),
                    KeyCode::Down => {
                        let max = state.sleep_episodes.len().saturating_sub(state.sleep_vis);
                        state.sleep_scroll = (state.sleep_scroll + 1).min(max);
                    }
                    KeyCode::PageUp => {
                        let page = state.sleep_vis.max(1);
                        state.sleep_scroll = state.sleep_scroll.saturating_sub(page);
                    }
                    KeyCode::PageDown => {
                        let page = state.sleep_vis.max(1);
                        let max = state.sleep_episodes.len().saturating_sub(state.sleep_vis);
                        state.sleep_scroll = (state.sleep_scroll + page).min(max);
                    }
                    KeyCode::Home => state.sleep_scroll = 0,
                    KeyCode::End => {
                        state.sleep_scroll =
                            state.sleep_episodes.len().saturating_sub(state.sleep_vis);
                    }
                    _ => {}
                }
            } else {
                match k.code {
                    KeyCode::Char('q') | KeyCode::Esc => quit = true,
                    KeyCode::Left => state.shift(-0.1),
                    KeyCode::Right => state.shift(0.1),
                    KeyCode::Char('+') | KeyCode::Char('=') => state.zoom(0.5),
                    KeyCode::Char('-') | KeyCode::Char('_') => state.zoom(2.0),
                    KeyCode::Char('r') => state.reset_window(),
                    KeyCode::Enter => state.toggle_select(),
                    KeyCode::Up => {
                        state.sel_idx = state.sel_idx.saturating_sub(1);
                    }
                    KeyCode::Down => {
                        state.sel_idx = (state.sel_idx + 1).min(state.agg.len().saturating_sub(1));
                    }
                    KeyCode::PageUp => {
                        let page = state.vis_rows.max(1);
                        state.sel_idx = state.sel_idx.saturating_sub(page);
                    }
                    KeyCode::PageDown => {
                        let page = state.vis_rows.max(1);
                        state.sel_idx =
                            (state.sel_idx + page).min(state.agg.len().saturating_sub(1));
                    }
                    KeyCode::Home => state.sel_idx = 0,
                    KeyCode::End => state.sel_idx = state.agg.len().saturating_sub(1),
                    KeyCode::Char('v') => state.show_sleep = true,
                    KeyCode::Char('s') => {
                        state.sort_peak = !state.sort_peak;
                        state.recalc();
                    }
                    KeyCode::Char('t') => state.theme = state.theme.next(),
                    _ => {}
                }
            }
        }
    }
    Ok(())
}
