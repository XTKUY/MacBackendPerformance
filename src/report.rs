use rusqlite::Connection;
use serde::Serialize;

use crate::cli::{EventsArgs, ReportArgs, SessionsArgs};
use crate::model::parse_iso;
use crate::storage::{
    list_sessions, power_events_range, process_aggregate, session_info, system_series, SessionInfo,
    SystemRow,
};

pub fn resolve_session(conn: &Connection, id: Option<i64>) -> Result<SessionInfo, String> {
    let sessions = list_sessions(conn).map_err(|e| format!("读取会话失败: {e}"))?;
    let sid = match id {
        Some(s) => s,
        None => sessions
            .first()
            .map(|s| s.id)
            .ok_or_else(|| "数据库中没有会话".to_string())?,
    };
    session_info(conn, sid)
        .map_err(|e| format!("读取会话 {sid} 失败: {e}"))?
        .ok_or_else(|| format!("会话 {sid} 不存在"))
}

/// 规范化时间段：解析并夹取到会话边界内。
pub fn resolve_bounds(
    info: &SessionInfo,
    from: Option<&str>,
    to: Option<&str>,
) -> Result<(String, String), String> {
    let start = info.started_at.clone();
    let end = info.ended_at.clone().unwrap_or_else(crate::model::now_iso);

    let norm = |s: &str| -> Result<String, String> {
        parse_iso(s)
            .map(|d| d.to_rfc3339_opts(chrono::SecondsFormat::Millis, false))
            .ok_or_else(|| {
                format!("无法解析时间: {s}（请用 ISO8601，如 2026-08-29T20:00:00+08:00）")
            })
    };

    let from = match from {
        Some(f) => {
            let f = norm(f)?;
            if f < start {
                start.clone()
            } else {
                f
            }
        }
        None => start.clone(),
    };
    let to = match to {
        Some(t) => {
            let t = norm(t)?;
            if t > end {
                end.clone()
            } else {
                t
            }
        }
        None => end,
    };
    if from >= to {
        return Err("时间段无效：from 必须早于 to".to_string());
    }
    Ok((from, to))
}

#[derive(Serialize)]
struct SysSummary {
    samples: usize,
    avg_cpu_pct: f64,
    avg_gpu_pct: f64,
    peak_gpu_pct: f64,
    avg_package_w: f64,
    avg_cpu_temp_c: f64,
    avg_gpu_temp_c: f64,
    avg_compressed_gb: f64,
    avg_swap_gb: f64,
}

fn summarize(sys: &[SystemRow]) -> SysSummary {
    let avg = |f: fn(&SystemRow) -> Option<f64>| -> f64 {
        let vals: Vec<f64> = sys.iter().filter_map(f).collect();
        if vals.is_empty() {
            0.0
        } else {
            vals.iter().sum::<f64>() / vals.len() as f64
        }
    };
    SysSummary {
        samples: sys.len(),
        avg_cpu_pct: avg(|r| r.cpu_total_pct),
        avg_gpu_pct: avg(|r| r.gpu_active_ratio.map(|v| v * 100.0)),
        peak_gpu_pct: sys
            .iter()
            .filter_map(|r| r.gpu_active_ratio.map(|v| v * 100.0))
            .fold(0.0_f64, f64::max),
        avg_package_w: avg(|r| r.package_power_w),
        avg_cpu_temp_c: avg(|r| r.cpu_temp_c),
        avg_gpu_temp_c: avg(|r| r.gpu_temp_c),
        avg_compressed_gb: avg(|r| r.ram_compressed_bytes.map(|b| b as f64 / 1073741824.0)),
        avg_swap_gb: avg(|r| r.swap_used_bytes.map(|b| b as f64 / 1073741824.0)),
    }
}

pub fn run_report(args: &ReportArgs) -> Result<(), String> {
    let conn = Connection::open(&args.db).map_err(|e| format!("打开数据库失败: {e}"))?;
    let info = resolve_session(&conn, args.session)?;
    let (from, to) = resolve_bounds(&info, args.from.as_deref(), args.to.as_deref())?;

    let procs = process_aggregate(&conn, info.id, &from, &to, args.top)
        .map_err(|e| format!("聚合失败: {e}"))?;
    let power = power_events_range(&conn, info.id, &from, &to)
        .map_err(|e| format!("读取电源事件失败: {e}"))?;
    let sys =
        system_series(&conn, info.id, &from, &to).map_err(|e| format!("读取系统采样失败: {e}"))?;
    let summary = summarize(&sys);

    match args.format.as_str() {
        "json" => {
            let out = serde_json::json!({
                "session": info,
                "from": from,
                "to": to,
                "system": summary,
                "processes": procs,
                "power_events": power,
            });
            println!(
                "{}",
                serde_json::to_string_pretty(&out).map_err(|e| e.to_string())?
            );
        }
        "csv" => {
            println!("pid,name,avg_cpu_pct,peak_cpu_pct,samples,avg_mem_mb");
            for p in &procs {
                println!(
                    "{},{},{:.1},{:.1},{},{}",
                    p.pid, p.name, p.avg_cpu, p.peak_cpu, p.samples, p.avg_mem
                );
            }
        }
        "md" => {
            println!(
                "## 会话 {} · {} → {}\n\n系统摘要：CPU 平均 {:.1}% · GPU 平均 {:.1}%（峰值 {:.1}%）· 平均功耗 {:.2} W · CPU 温度 {:.0}°C · 压缩内存 {:.1} GB · Swap 已用 {:.1} GB\n",
                info.id, from, to, summary.avg_cpu_pct, summary.avg_gpu_pct, summary.peak_gpu_pct,
                summary.avg_package_w, summary.avg_cpu_temp_c, summary.avg_compressed_gb,
                summary.avg_swap_gb
            );
            println!("| PID | 进程 | 样本数 | 平均 CPU% | 峰值 CPU% | 平均内存 MB |");
            println!("| --- | --- | ---: | ---: | ---: | ---: |");
            for p in &procs {
                println!(
                    "| {} | {} | {} | {:.1} | {:.1} | {} |",
                    p.pid, p.name, p.samples, p.avg_cpu, p.peak_cpu, p.avg_mem
                );
            }
            if !power.is_empty() {
                println!("\n### 电源事件\n");
                println!("| 时间 | 事件 | 详情 |");
                println!("| --- | --- | --- |");
                for e in &power {
                    println!("| {} | {} | {} |", e.ts, e.event_type, e.detail);
                }
            }
        }
        _ => {
            println!("会话 {} · {} → {}", info.id, from, to);
            println!(
                "系统摘要：CPU 平均 {:.1}% · GPU 平均 {:.1}%（峰值 {:.1}%）· 平均功耗 {:.2} W · CPU 温度 {:.0}°C · 压缩内存 {:.1} GB · Swap 已用 {:.1} GB\n",
                summary.avg_cpu_pct, summary.avg_gpu_pct, summary.peak_gpu_pct,
                summary.avg_package_w, summary.avg_cpu_temp_c, summary.avg_compressed_gb,
                summary.avg_swap_gb
            );
            let w = |s: &str, n: usize| format!("{s:<n$}", s = s, n = n);
            println!(
                "{} {} {} {} {} {}",
                w("PID", 8),
                w("进程", 32),
                w("样本数", 8),
                w("平均CPU%", 10),
                w("峰值CPU%", 10),
                w("平均内存MB", 12)
            );
            for p in &procs {
                println!(
                    "{} {} {} {:>9.1} {:>9.1} {:>11.1}",
                    w(&p.pid.to_string(), 8),
                    w(&p.name, 32),
                    w(&p.samples.to_string(), 8),
                    p.avg_cpu,
                    p.peak_cpu,
                    p.avg_mem
                );
            }
            if !power.is_empty() {
                println!("\n电源事件：");
                for e in &power {
                    println!("  {}  {}  {}", e.ts, e.event_type, e.detail);
                }
            }
        }
    }
    Ok(())
}

pub fn run_events(args: &EventsArgs) -> Result<(), String> {
    let conn = Connection::open(&args.db).map_err(|e| format!("打开数据库失败: {e}"))?;
    let info = resolve_session(&conn, args.session)?;
    let (from, to) = resolve_bounds(&info, args.from.as_deref(), args.to.as_deref())?;
    let events = power_events_range(&conn, info.id, &from, &to)
        .map_err(|e| format!("读取电源事件失败: {e}"))?;
    if events.is_empty() {
        println!("该时间段内没有电源事件。");
    }
    for e in &events {
        println!("{}\t{}\t{}", e.ts, e.event_type, e.detail);
    }
    Ok(())
}

pub fn run_sessions(args: &SessionsArgs) -> Result<(), String> {
    let conn = Connection::open(&args.db).map_err(|e| format!("打开数据库失败: {e}"))?;
    let sessions = list_sessions(&conn).map_err(|e| format!("读取会话失败: {e}"))?;
    if sessions.is_empty() {
        println!("数据库中没有会话。");
        return Ok(());
    }
    println!("ID\t开始时间\t结束时间\t机型\t备注");
    for s in &sessions {
        let ended = s.ended_at.as_deref().unwrap_or("进行中");
        let model = s.model.as_deref().unwrap_or("-");
        let note = s.note.as_deref().unwrap_or("-");
        println!("{}\t{}\t{}\t{}\t{}", s.id, s.started_at, ended, model, note);
    }
    Ok(())
}
