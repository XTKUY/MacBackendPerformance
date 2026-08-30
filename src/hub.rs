use std::io;
use std::time::Duration;

use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Cell, Clear, Paragraph, Row, Table};
use ratatui::Frame;

use crate::cli::{HubArgs, InspectArgs};
use crate::config::{load as load_config, save as save_config, AppConfig};
use crate::storage::{delete_session, list_sessions, SessionInfo};
use crate::theme::{Flavor, Palette};
use crate::tui::{run_inspect_terminal, run_record_terminal, FormResult, SetupForm};

#[derive(PartialEq)]
enum Screen {
    Menu,
    Sessions,
    Settings,
    Record,
    Quit,
}

fn next_key() -> io::Result<Option<KeyCode>> {
    if !event::poll(Duration::from_millis(150))? {
        return Ok(None);
    }
    if let Event::Key(k) = event::read()? {
        if k.kind == KeyEventKind::Press {
            return Ok(Some(k.code));
        }
    }
    Ok(None)
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

fn draw_menu(frame: &mut Frame, cfg: &AppConfig, sel: usize) {
    let pal = Palette::of(Flavor::from_name(&cfg.theme).unwrap_or(Flavor::Mocha));
    frame.render_widget(
        Block::default().style(Style::new().bg(pal.bg)),
        frame.area(),
    );

    let items = ["开始新记录", "历史会话（回放）", "设置", "退出"];
    let mut lines = vec![
        Line::from(" "),
        Line::from(vec![
            Span::styled(
                " neko-perf ",
                Style::new().fg(pal.mantle).bg(pal.mauve).bold(),
            ),
            Span::styled("  统一监控台", pal.title_style()),
        ]),
        Line::from(" "),
    ];
    for (i, item) in items.iter().enumerate() {
        let arrow = if i == sel { "▸ " } else { "  " };
        let style = if i == sel {
            Style::new()
                .fg(pal.mantle)
                .bg(pal.mauve)
                .add_modifier(Modifier::BOLD)
        } else {
            pal.text_style()
        };
        lines.push(Line::from(vec![Span::styled(
            format!("{arrow}{item}"),
            style,
        )]));
        lines.push(Line::from(" "));
    }
    lines.push(Line::from(" "));
    lines.push(Line::from(vec![Span::styled(
        format!(
            "数据库 {} · 采样间隔 {}ms · 进程间隔 {}ms · Top {} · GPU {} · 主题 {}",
            cfg.db_path.display(),
            cfg.interval_ms,
            cfg.process_interval_ms,
            cfg.top_n,
            if cfg.gpu { "开" } else { "关" },
            cfg.theme
        ),
        pal.sub_style(),
    )]));
    lines.push(Line::from(vec![Span::styled(
        "↑/↓ 选择 · Enter 确认 · q/Esc 退出",
        pal.sub_style(),
    )]));
    frame.render_widget(
        Paragraph::new(ratatui::text::Text::from(lines)).block(
            Block::bordered()
                .title("主菜单")
                .title_style(pal.title_style())
                .border_style(pal.border_style())
                .style(pal.bg_style()),
        ),
        frame.area(),
    );
}

fn draw_sessions(
    frame: &mut Frame,
    sessions: &[SessionInfo],
    sel: usize,
    db: &str,
    confirm: Option<&SessionInfo>,
) {
    let pal = Palette::of(Flavor::Mocha);
    frame.render_widget(
        Block::default().style(Style::new().bg(pal.bg)),
        frame.area(),
    );
    let header = Row::new(vec![
        "ID",
        "开始时间",
        "结束时间",
        "机型",
        "系统采样",
        "进程采样",
    ])
    .style(pal.title_style());
    let rows: Vec<Row> = sessions
        .iter()
        .enumerate()
        .map(|(i, s)| {
            let row_style = if i == sel {
                Style::new().fg(pal.text).bg(pal.surface)
            } else {
                pal.text_style()
            };
            let ended = s.ended_at.as_deref().unwrap_or("进行中");
            Row::new(vec![
                Cell::from(s.id.to_string()).style(row_style),
                Cell::from(s.started_at.clone()).style(row_style),
                Cell::from(ended.to_string()).style(row_style),
                Cell::from(s.model.clone().unwrap_or_else(|| "-".into())).style(row_style),
                Cell::from(s.system_samples.to_string()).style(pal.sub_style()),
                Cell::from(s.process_samples.to_string()).style(pal.sub_style()),
            ])
        })
        .collect();
    let table = Table::new(
        rows,
        [
            Constraint::Length(5),
            Constraint::Length(26),
            Constraint::Length(26),
            Constraint::Length(14),
            Constraint::Length(10),
            Constraint::Length(10),
        ],
    )
    .header(header)
    .column_spacing(1)
    .block(
        Block::bordered()
            .title(format!("历史会话（回放） · {db}"))
            .title_style(pal.title_style())
            .border_style(pal.border_style())
            .style(pal.bg_style()),
    );
    frame.render_widget(table, frame.area());

    let footer = if sessions.is_empty() {
        "没有会话记录。Enter 后选择“开始新记录”。"
    } else {
        "↑/↓ 选择 · Enter 回放 · d 删除 · q/Esc 返回主菜单"
    };
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(footer, pal.sub_style()))),
        Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).split(frame.area())[1],
    );

    if let Some(s) = confirm {
        let pop = centered_area(frame.area(), 62, 34);
        frame.render_widget(Clear, pop);
        let block = Block::bordered()
            .title("确认删除")
            .title_style(pal.title_style())
            .border_style(pal.border_style())
            .style(pal.bg_style());
        frame.render_widget(block.clone(), pop);
        let inner = block.inner(pop);
        let lines = vec![
            Line::from(" "),
            Line::from(Span::styled(
                format!("确定删除会话 {}（开始于 {}）？", s.id, s.started_at),
                pal.text_style(),
            )),
            Line::from(Span::styled(
                "将删除该会话全部采样数据及其独立数据库文件，此操作不可恢复。",
                pal.yellow,
            )),
            Line::from(" "),
            Line::from(Span::styled("Enter 确认删除 · Esc 取消", pal.sub_style())),
        ];
        frame.render_widget(Paragraph::new(ratatui::text::Text::from(lines)), inner);
    }
}

pub fn run_hub(args: &HubArgs) -> io::Result<()> {
    let mut cfg = load_config();
    if let Some(db) = &args.db {
        cfg.db_path = db.clone();
    }
    let mut terminal = ratatui::init();

    let mut screen = Screen::Menu;
    let mut menu_idx = 0usize;
    let mut session_idx = 0usize;
    let mut confirm_delete: Option<usize> = None;
    let mut sessions: Vec<SessionInfo> = Vec::new();
    let mut conn: Option<rusqlite::Connection> = None;

    let result = loop {
        match screen {
            Screen::Menu => {
                terminal.draw(|f| draw_menu(f, &cfg, menu_idx))?;
                match next_key()? {
                    Some(KeyCode::Up | KeyCode::Char('k')) => {
                        menu_idx = menu_idx.saturating_sub(1);
                    }
                    Some(KeyCode::Down | KeyCode::Char('j')) => menu_idx = (menu_idx + 1) % 4,
                    Some(KeyCode::Enter) => {
                        screen = match menu_idx {
                            0 => Screen::Record,
                            1 => {
                                conn = rusqlite::Connection::open(&cfg.db_path).ok();
                                sessions = match conn.as_ref() {
                                    Some(c) => list_sessions(c).unwrap_or_default(),
                                    None => Vec::new(),
                                };
                                session_idx = 0;
                                Screen::Sessions
                            }
                            2 => Screen::Settings,
                            _ => Screen::Quit,
                        };
                    }
                    Some(KeyCode::Char('q') | KeyCode::Esc) => screen = Screen::Quit,
                    _ => {}
                }
            }
            Screen::Sessions => {
                terminal.draw(|f| {
                    draw_sessions(
                        f,
                        &sessions,
                        session_idx,
                        &cfg.db_path.display().to_string(),
                        confirm_delete.and_then(|i| sessions.get(i)),
                    )
                })?;
                if let Some(k) = next_key()? {
                    if confirm_delete.is_some() {
                        // 删除确认窗口：Enter 确认，Esc/q 取消。
                        match k {
                            KeyCode::Enter => {
                                if let (Some(c), Some(idx)) = (conn.as_ref(), confirm_delete) {
                                    if let Some(s) = sessions.get(idx) {
                                        match delete_session(c, s.id) {
                                            Ok(_) => {
                                                sessions = list_sessions(c).unwrap_or_default();
                                            }
                                            Err(e) => eprintln!("[neko-perf] 删除会话失败: {e}"),
                                        }
                                    }
                                }
                                confirm_delete = None;
                                session_idx = session_idx.min(sessions.len().saturating_sub(1));
                            }
                            KeyCode::Esc | KeyCode::Char('q') => confirm_delete = None,
                            _ => {}
                        }
                    } else {
                        match k {
                            KeyCode::Up | KeyCode::Char('k') => {
                                session_idx = session_idx.saturating_sub(1);
                            }
                            KeyCode::Down | KeyCode::Char('j') => {
                                session_idx =
                                    (session_idx + 1).min(sessions.len().saturating_sub(1));
                            }
                            KeyCode::Enter => {
                                if let (Some(c), Some(s)) =
                                    (conn.as_ref(), sessions.get(session_idx))
                                {
                                    let iargs = InspectArgs {
                                        db: cfg.db_path.clone(),
                                        session: Some(s.id),
                                        from: None,
                                        to: None,
                                    };
                                    run_inspect_terminal(&mut terminal, c, &iargs)?;
                                }
                            }
                            KeyCode::Char('d') | KeyCode::Char('x') => {
                                if !sessions.is_empty() {
                                    confirm_delete = Some(session_idx);
                                }
                            }
                            KeyCode::Char('q') | KeyCode::Esc => {
                                conn = None;
                                confirm_delete = None;
                                screen = Screen::Menu;
                            }
                            _ => {}
                        }
                    }
                }
            }
            Screen::Settings => {
                let mut form = SetupForm::new(&cfg);
                let theme = Flavor::from_name(&cfg.theme).unwrap_or(Flavor::Mocha);
                'settings: loop {
                    terminal.draw(|f| {
                        form.draw(
                            f,
                            Palette::of(theme),
                            "设置",
                            "Enter 保存 · Tab/方向键 切换 · Esc 返回",
                        )
                    })?;
                    if let Some(k) = next_key()? {
                        match form.handle(k) {
                            FormResult::Submit(c) => {
                                cfg = c;
                                if let Err(e) = save_config(&cfg) {
                                    eprintln!("[neko-perf] 保存设置失败: {e}");
                                }
                                screen = Screen::Menu;
                                break 'settings;
                            }
                            FormResult::Cancel => {
                                screen = Screen::Menu;
                                break 'settings;
                            }
                            FormResult::None => {}
                        }
                    }
                }
            }
            Screen::Record => {
                run_record_terminal(&mut terminal, &cfg)?;
                screen = Screen::Menu;
            }
            Screen::Quit => break Ok(()),
        }
    };
    ratatui::restore();
    result
}
