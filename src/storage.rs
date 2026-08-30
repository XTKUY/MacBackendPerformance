use std::path::{Path, PathBuf};

use rusqlite::{params, Connection, OptionalExtension, Result};

use crate::model::{now_iso, PowerEvent, ProcessSample, SystemSample};

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS sessions (
  id              INTEGER PRIMARY KEY,
  started_at      TEXT NOT NULL,
  ended_at        TEXT,
  hostname        TEXT,
  model           TEXT,
  ram_total_bytes INTEGER,
  note            TEXT,
  db_path         TEXT
);

CREATE TABLE IF NOT EXISTS system_samples (
  id               INTEGER PRIMARY KEY,
  session_id       INTEGER NOT NULL REFERENCES sessions(id),
  ts               TEXT NOT NULL,
  power_state      TEXT,
  cpu_total_pct    REAL,
  cpu_active_ratio REAL,
  cpu_power_w      REAL,
  gpu_active_ratio REAL,
  gpu_freq_mhz     REAL,
  gpu_power_w      REAL,
  ane_power_w      REAL,
  package_power_w  REAL,
  cpu_temp_c       REAL,
  gpu_temp_c       REAL,
  thermal_state    TEXT,
  ram_total_bytes  INTEGER,
  ram_used_bytes   INTEGER,
  mem_app_bytes    INTEGER,
  mem_wired_bytes  INTEGER,
  mem_compressed_bytes INTEGER,
  mem_inactive_bytes   INTEGER,
  mem_free_bytes       INTEGER,
  swap_total_bytes INTEGER,
  swap_used_bytes  INTEGER
);

CREATE TABLE IF NOT EXISTS process_samples (
  id         INTEGER PRIMARY KEY,
  session_id INTEGER NOT NULL REFERENCES sessions(id),
  ts         TEXT NOT NULL,
  pid        INTEGER NOT NULL,
  name       TEXT NOT NULL,
  cpu_pct    REAL,
  mem_bytes  INTEGER,
  gpu_ms     INTEGER
);

CREATE TABLE IF NOT EXISTS power_events (
  id         INTEGER PRIMARY KEY,
  session_id INTEGER NOT NULL REFERENCES sessions(id),
  ts         TEXT NOT NULL,
  event_type TEXT NOT NULL,
  detail     TEXT
);

CREATE INDEX IF NOT EXISTS idx_sys_ts  ON system_samples(session_id, ts);
CREATE INDEX IF NOT EXISTS idx_proc_ts ON process_samples(session_id, ts);
CREATE INDEX IF NOT EXISTS idx_pwr_ts  ON power_events(session_id, ts);
"#;

/// 旧库迁移：按需补充 system_samples 新增列。
fn migrate(conn: &Connection) -> Result<()> {
    let new_columns: &[(&str, &str)] = &[
        ("ram_total_bytes", "INTEGER"),
        ("mem_app_bytes", "INTEGER"),
        ("mem_wired_bytes", "INTEGER"),
        ("mem_compressed_bytes", "INTEGER"),
        ("mem_inactive_bytes", "INTEGER"),
        ("mem_free_bytes", "INTEGER"),
        ("swap_total_bytes", "INTEGER"),
    ];
    let mut stmt = conn.prepare("PRAGMA table_info(system_samples)")?;
    let existing: Vec<String> = stmt
        .query_map([], |r| r.get::<_, String>(1))?
        .collect::<Result<Vec<_>>>()?;
    for (col, ty) in new_columns {
        if !existing.iter().any(|c| c == col) {
            conn.execute_batch(&format!(
                "ALTER TABLE system_samples ADD COLUMN {col} {ty};"
            ))?;
        }
    }

    // sessions.db_path：记录该会话的独立数据文件位置。
    let mut stmt = conn.prepare("PRAGMA table_info(sessions)")?;
    let sess_cols: Vec<String> = stmt
        .query_map([], |r| r.get::<_, String>(1))?
        .collect::<Result<Vec<_>>>()?;
    if !sess_cols.iter().any(|c| c == "db_path") {
        conn.execute_batch("ALTER TABLE sessions ADD COLUMN db_path TEXT;")?;
    }
    Ok(())
}

/// 会话独立数据文件目录：与主库同目录下的 sessions/。
fn session_folder(main: &Path) -> PathBuf {
    match main.parent() {
        Some(p) if !p.as_os_str().is_empty() => p.join("sessions"),
        _ => PathBuf::from("sessions"),
    }
}

/// 会话独立数据文件：sessions/<会话ID>.db。
fn session_file(main: &Path, id: i64) -> PathBuf {
    session_folder(main).join(format!("{id}.db"))
}

/// 由写入线程独占持有的存储句柄。
pub struct Storage {
    /// 主库连接：只维护 sessions 索引表。
    conn: Connection,
    /// 本会话独立数据文件连接：system/process/power 三张表。
    data_conn: Connection,
    session_id: i64,
    #[allow(dead_code)]
    data_path: PathBuf,
}

impl Storage {
    pub fn open(path: &Path, model: Option<&str>, ram_total_bytes: u64) -> Result<Storage> {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
            }
        }
        let conn = Connection::open(path)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.execute_batch(SCHEMA)?;
        migrate(&conn)?;

        let hostname = sysinfo::System::host_name().unwrap_or_else(|| "unknown".into());
        let started = now_iso();
        conn.execute(
            "INSERT INTO sessions (started_at, hostname, model, ram_total_bytes)
             VALUES (?1, ?2, ?3, ?4)",
            params![started, hostname, model, ram_total_bytes as i64],
        )?;
        let session_id = conn.last_insert_rowid();

        // 每个会话的数据写入独立文件 sessions/<会话ID>.db。
        let data_path = session_file(path, session_id);
        if let Some(parent) = data_path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
            }
        }
        let data_conn = Connection::open(&data_path)?;
        data_conn.pragma_update(None, "journal_mode", "WAL")?;
        data_conn.pragma_update(None, "synchronous", "NORMAL")?;
        data_conn.execute_batch(SCHEMA)?;
        // 数据文件内也写入同一条会话元数据，保证文件自包含、固定 id 标识。
        data_conn.execute(
            "INSERT INTO sessions (id, started_at, hostname, model, ram_total_bytes)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![session_id, started, hostname, model, ram_total_bytes as i64],
        )?;
        conn.execute(
            "UPDATE sessions SET db_path = ?1 WHERE id = ?2",
            params![data_path.display().to_string(), session_id],
        )?;

        Ok(Storage {
            conn,
            data_conn,
            session_id,
            data_path,
        })
    }

    pub fn session_id(&self) -> i64 {
        self.session_id
    }

    pub fn insert_system(&mut self, s: &SystemSample) -> Result<()> {
        self.data_conn.execute(
            "INSERT INTO system_samples (
                session_id, ts, power_state, cpu_total_pct, cpu_active_ratio, cpu_power_w,
                gpu_active_ratio, gpu_freq_mhz, gpu_power_w, ane_power_w, package_power_w,
                cpu_temp_c, gpu_temp_c, thermal_state, ram_total_bytes, ram_used_bytes,
                mem_app_bytes, mem_wired_bytes, mem_compressed_bytes, mem_inactive_bytes,
                mem_free_bytes, swap_total_bytes, swap_used_bytes
             ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,?21,?22,?23)",
            params![
                self.session_id,
                s.ts.to_rfc3339_opts(chrono::SecondsFormat::Millis, false),
                s.power_state,
                s.cpu_total_pct,
                s.cpu_active_ratio,
                s.cpu_power_w,
                s.gpu_active_ratio,
                s.gpu_freq_mhz,
                s.gpu_power_w,
                s.ane_power_w,
                s.package_power_w,
                s.cpu_temp_c,
                s.gpu_temp_c,
                None::<String>,
                s.ram_total_bytes as i64,
                s.ram_used_bytes as i64,
                s.mem_app_bytes as i64,
                s.mem_wired_bytes as i64,
                s.mem_compressed_bytes as i64,
                s.mem_inactive_bytes as i64,
                s.mem_free_bytes as i64,
                s.swap_total_bytes as i64,
                s.swap_used_bytes as i64,
            ],
        )?;
        Ok(())
    }

    pub fn insert_processes(&mut self, rows: &[ProcessSample]) -> Result<()> {
        let tx = self.data_conn.transaction()?;
        {
            let mut stmt = tx.prepare_cached(
                "INSERT INTO process_samples (session_id, ts, pid, name, cpu_pct, mem_bytes)
                 VALUES (?1,?2,?3,?4,?5,?6)",
            )?;
            for r in rows {
                stmt.execute(params![
                    self.session_id,
                    r.ts.to_rfc3339_opts(chrono::SecondsFormat::Millis, false),
                    r.pid,
                    r.name,
                    r.cpu_pct,
                    r.mem_bytes as i64,
                ])?;
            }
        }
        tx.commit()
    }

    pub fn insert_power_event(&mut self, e: &PowerEvent) -> Result<()> {
        self.data_conn.execute(
            "INSERT INTO power_events (session_id, ts, event_type, detail)
             VALUES (?1, ?2, ?3, ?4)",
            params![
                self.session_id,
                e.ts.to_rfc3339_opts(chrono::SecondsFormat::Millis, false),
                e.kind.as_str(),
                e.detail,
            ],
        )?;
        Ok(())
    }

    pub fn finish(&mut self, note: Option<&str>) -> Result<()> {
        let ended = now_iso();
        self.conn.execute(
            "UPDATE sessions SET ended_at = ?1, note = COALESCE(?2, note) WHERE id = ?3",
            params![ended, note, self.session_id],
        )?;
        self.data_conn.execute(
            "UPDATE sessions SET ended_at = ?1, note = COALESCE(?2, note) WHERE id = ?3",
            params![ended, note, self.session_id],
        )?;
        Ok(())
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct SessionInfo {
    pub id: i64,
    pub started_at: String,
    pub ended_at: Option<String>,
    pub hostname: Option<String>,
    pub model: Option<String>,
    pub ram_total_bytes: Option<i64>,
    pub note: Option<String>,
    pub system_samples: i64,
    pub process_samples: i64,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct SystemRow {
    pub ts: String,
    pub power_state: Option<String>,
    pub cpu_total_pct: Option<f64>,
    pub gpu_active_ratio: Option<f64>,
    pub gpu_power_w: Option<f64>,
    pub package_power_w: Option<f64>,
    pub ram_used_bytes: Option<i64>,
    pub ram_compressed_bytes: Option<i64>,
    pub swap_used_bytes: Option<i64>,
    pub cpu_temp_c: Option<f64>,
    pub gpu_temp_c: Option<f64>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ProcAgg {
    pub pid: i64,
    pub name: String,
    pub samples: i64,
    pub avg_cpu: f64,
    pub peak_cpu: f64,
    pub avg_mem: f64,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct PowerRow {
    pub ts: String,
    pub event_type: String,
    pub detail: String,
}

/// 打开会话的独立数据文件连接；旧数据（尚未拆分）返回 None。
fn session_data_conn(conn: &Connection, session_id: i64) -> Result<Option<Connection>> {
    let stored: Option<Option<String>> = conn
        .query_row(
            "SELECT db_path FROM sessions WHERE id = ?1",
            params![session_id],
            |r| r.get(0),
        )
        .optional()?;
    match stored {
        Some(Some(p)) if Path::new(&p).exists() => Ok(Some(Connection::open(p)?)),
        _ => Ok(None),
    }
}

/// 会话采样计数：优先从独立数据文件统计，旧数据回退到主库。
fn session_counts(conn: &Connection, session_id: i64) -> Result<(i64, i64)> {
    let data = session_data_conn(conn, session_id)?;
    let c = data.as_ref().unwrap_or(conn);
    let sys = c.query_row(
        "SELECT COUNT(*) FROM system_samples WHERE session_id = ?1",
        params![session_id],
        |r| r.get(0),
    )?;
    let proc = c.query_row(
        "SELECT COUNT(*) FROM process_samples WHERE session_id = ?1",
        params![session_id],
        |r| r.get(0),
    )?;
    Ok((sys, proc))
}

pub fn list_sessions(conn: &Connection) -> Result<Vec<SessionInfo>> {
    ensure_session_split(conn)?;
    let mut stmt = conn.prepare(
        "SELECT s.id, s.started_at, s.ended_at, s.hostname, s.model, s.ram_total_bytes, s.note
         FROM sessions s ORDER BY s.id DESC",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, i64>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, Option<String>>(2)?,
            r.get::<_, Option<String>>(3)?,
            r.get::<_, Option<String>>(4)?,
            r.get::<_, Option<i64>>(5)?,
            r.get::<_, Option<String>>(6)?,
        ))
    })?;
    let mut infos: Vec<SessionInfo> = rows
        .map(|r| {
            let (id, started_at, ended_at, hostname, model, ram_total_bytes, note) = r?;
            Ok(SessionInfo {
                id,
                started_at,
                ended_at,
                hostname,
                model,
                ram_total_bytes,
                note,
                system_samples: 0,
                process_samples: 0,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    for info in &mut infos {
        let (sys, proc) = session_counts(conn, info.id)?;
        info.system_samples = sys;
        info.process_samples = proc;
    }
    Ok(infos)
}

pub fn session_info(conn: &Connection, id: i64) -> Result<Option<SessionInfo>> {
    ensure_session_split(conn)?;
    let row = conn
        .query_row(
            "SELECT s.id, s.started_at, s.ended_at, s.hostname, s.model, s.ram_total_bytes, s.note
             FROM sessions s WHERE s.id = ?1",
            params![id],
            |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, Option<String>>(2)?,
                    r.get::<_, Option<String>>(3)?,
                    r.get::<_, Option<String>>(4)?,
                    r.get::<_, Option<i64>>(5)?,
                    r.get::<_, Option<String>>(6)?,
                ))
            },
        )
        .optional()?;
    let Some((id, started_at, ended_at, hostname, model, ram_total_bytes, note)) = row else {
        return Ok(None);
    };
    let (sys, proc) = session_counts(conn, id)?;
    Ok(Some(SessionInfo {
        id,
        started_at,
        ended_at,
        hostname,
        model,
        ram_total_bytes,
        note,
        system_samples: sys,
        process_samples: proc,
    }))
}

pub fn system_series(
    conn: &Connection,
    session_id: i64,
    from: &str,
    to: &str,
) -> Result<Vec<SystemRow>> {
    let data = session_data_conn(conn, session_id)?;
    let c = data.as_ref().unwrap_or(conn);
    system_series_impl(c, session_id, from, to)
}

fn system_series_impl(
    conn: &Connection,
    session_id: i64,
    from: &str,
    to: &str,
) -> Result<Vec<SystemRow>> {
    let mut stmt = conn.prepare(
        "SELECT ts, power_state, cpu_total_pct, gpu_active_ratio, gpu_power_w,
                package_power_w, ram_used_bytes, mem_compressed_bytes, swap_used_bytes,
                cpu_temp_c, gpu_temp_c
         FROM system_samples
         WHERE session_id = ?1 AND ts >= ?2 AND ts <= ?3
         ORDER BY ts",
    )?;
    let rows = stmt.query_map(params![session_id, from, to], |r| {
        Ok(SystemRow {
            ts: r.get(0)?,
            power_state: r.get(1)?,
            cpu_total_pct: r.get(2)?,
            gpu_active_ratio: r.get(3)?,
            gpu_power_w: r.get(4)?,
            package_power_w: r.get(5)?,
            ram_used_bytes: r.get(6)?,
            ram_compressed_bytes: r.get(7)?,
            swap_used_bytes: r.get(8)?,
            cpu_temp_c: r.get(9)?,
            gpu_temp_c: r.get(10)?,
        })
    })?;
    rows.collect()
}

pub fn process_aggregate(
    conn: &Connection,
    session_id: i64,
    from: &str,
    to: &str,
    limit: usize,
) -> Result<Vec<ProcAgg>> {
    let data = session_data_conn(conn, session_id)?;
    let c = data.as_ref().unwrap_or(conn);
    process_aggregate_impl(c, session_id, from, to, limit)
}

fn process_aggregate_impl(
    conn: &Connection,
    session_id: i64,
    from: &str,
    to: &str,
    limit: usize,
) -> Result<Vec<ProcAgg>> {
    // limit == 0 表示不设上限，加载该窗口内采到的全部进程。
    if limit == 0 {
        let mut stmt = conn.prepare(
            "SELECT pid, name,
                    COUNT(*)                       AS samples,
                    ROUND(AVG(cpu_pct), 1)         AS avg_cpu,
                    MAX(cpu_pct)                   AS peak_cpu,
                    ROUND(AVG(mem_bytes) / 1048576.0, 1) AS avg_mem_mb
             FROM process_samples
             WHERE session_id = ?1 AND ts >= ?2 AND ts <= ?3
             GROUP BY pid, name
             ORDER BY avg_cpu DESC",
        )?;
        let rows = stmt.query_map(params![session_id, from, to], |r| {
            Ok(ProcAgg {
                pid: r.get(0)?,
                name: r.get(1)?,
                samples: r.get(2)?,
                avg_cpu: r.get(3)?,
                peak_cpu: r.get(4)?,
                avg_mem: r.get(5)?,
            })
        })?;
        rows.collect()
    } else {
        let mut stmt = conn.prepare(
            "SELECT pid, name,
                    COUNT(*)                       AS samples,
                    ROUND(AVG(cpu_pct), 1)         AS avg_cpu,
                    MAX(cpu_pct)                   AS peak_cpu,
                    ROUND(AVG(mem_bytes) / 1048576.0, 1) AS avg_mem_mb
             FROM process_samples
             WHERE session_id = ?1 AND ts >= ?2 AND ts <= ?3
             GROUP BY pid, name
             ORDER BY avg_cpu DESC
             LIMIT ?4",
        )?;
        let rows = stmt.query_map(params![session_id, from, to, limit as i64], |r| {
            Ok(ProcAgg {
                pid: r.get(0)?,
                name: r.get(1)?,
                samples: r.get(2)?,
                avg_cpu: r.get(3)?,
                peak_cpu: r.get(4)?,
                avg_mem: r.get(5)?,
            })
        })?;
        rows.collect()
    }
}

#[derive(Debug, Clone)]
pub struct ProcSeriesRow {
    pub cpu_pct: f64,
}

pub fn process_series(
    conn: &Connection,
    session_id: i64,
    pid: i64,
    from: &str,
    to: &str,
) -> Result<Vec<ProcSeriesRow>> {
    let data = session_data_conn(conn, session_id)?;
    let c = data.as_ref().unwrap_or(conn);
    process_series_impl(c, session_id, pid, from, to)
}

fn process_series_impl(
    conn: &Connection,
    session_id: i64,
    pid: i64,
    from: &str,
    to: &str,
) -> Result<Vec<ProcSeriesRow>> {
    let mut stmt = conn.prepare(
        "SELECT cpu_pct FROM process_samples
         WHERE session_id = ?1 AND pid = ?2 AND ts >= ?3 AND ts <= ?4
         ORDER BY ts",
    )?;
    let rows = stmt.query_map(params![session_id, pid, from, to], |r| {
        Ok(ProcSeriesRow { cpu_pct: r.get(0)? })
    })?;
    rows.collect()
}

pub fn power_events_range(
    conn: &Connection,
    session_id: i64,
    from: &str,
    to: &str,
) -> Result<Vec<PowerRow>> {
    let data = session_data_conn(conn, session_id)?;
    let c = data.as_ref().unwrap_or(conn);
    power_events_range_impl(c, session_id, from, to)
}

fn power_events_range_impl(
    conn: &Connection,
    session_id: i64,
    from: &str,
    to: &str,
) -> Result<Vec<PowerRow>> {
    let mut stmt = conn.prepare(
        "SELECT ts, event_type, detail FROM power_events
         WHERE session_id = ?1 AND ts >= ?2 AND ts <= ?3
         ORDER BY ts",
    )?;
    let rows = stmt.query_map(params![session_id, from, to], |r| {
        Ok(PowerRow {
            ts: r.get(0)?,
            event_type: r.get(1)?,
            detail: r.get(2)?,
        })
    })?;
    rows.collect()
}

/// 把旧版单一数据库里的历史会话拆分为独立数据文件（幂等，可重复执行）。
/// 主库只保留 sessions 索引，数据迁到 sessions/<会话ID>.db。
pub fn ensure_session_split(conn: &Connection) -> Result<()> {
    migrate(conn)?;
    let Some(main_path) = conn.path() else {
        return Ok(());
    };

    let mut stmt = conn.prepare("SELECT id, db_path FROM sessions ORDER BY id")?;
    let rows: Vec<(i64, Option<String>)> = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<Result<Vec<_>>>()?;

    for (id, stored) in rows {
        let file = session_file(Path::new(main_path), id);
        let needs_split = match &stored {
            Some(p) => !Path::new(p).exists(),
            None => true,
        };
        if !needs_split {
            continue;
        }

        let sys = conn.query_row(
            "SELECT COUNT(*) FROM system_samples WHERE session_id = ?1",
            params![id],
            |r| r.get::<_, i64>(0),
        )?;
        let proc = conn.query_row(
            "SELECT COUNT(*) FROM process_samples WHERE session_id = ?1",
            params![id],
            |r| r.get::<_, i64>(0),
        )?;
        let pwr = conn.query_row(
            "SELECT COUNT(*) FROM power_events WHERE session_id = ?1",
            params![id],
            |r| r.get::<_, i64>(0),
        )?;
        if sys + proc + pwr == 0 {
            // 无数据可拆：只登记路径，避免每次重复检查。
            conn.execute(
                "UPDATE sessions SET db_path = ?1 WHERE id = ?2",
                params![file.display().to_string(), id],
            )?;
            continue;
        }

        if let Some(parent) = file.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
            }
        }
        let mut data = Connection::open(&file)?;
        data.pragma_update(None, "journal_mode", "WAL")?;
        data.pragma_update(None, "synchronous", "NORMAL")?;
        data.execute_batch(SCHEMA)?;
        let esc = main_path.replace('\'', "''");
        data.execute_batch(&format!("ATTACH DATABASE '{esc}' AS maindb;"))?;

        struct SessionMeta {
            started_at: String,
            ended_at: Option<String>,
            hostname: Option<String>,
            model: Option<String>,
            ram_total_bytes: Option<i64>,
            note: Option<String>,
        }
        let meta = conn.query_row(
            "SELECT started_at, ended_at, hostname, model, ram_total_bytes, note
             FROM sessions WHERE id = ?1",
            params![id],
            |r| {
                Ok(SessionMeta {
                    started_at: r.get(0)?,
                    ended_at: r.get(1)?,
                    hostname: r.get(2)?,
                    model: r.get(3)?,
                    ram_total_bytes: r.get(4)?,
                    note: r.get(5)?,
                })
            },
        )?;

        let tx = data.transaction()?;
        // 幂等：重复执行时先清空目标，避免数据翻倍。
        tx.execute("DELETE FROM sessions WHERE id = ?1", params![id])?;
        tx.execute(
            "DELETE FROM system_samples WHERE session_id = ?1",
            params![id],
        )?;
        tx.execute(
            "DELETE FROM process_samples WHERE session_id = ?1",
            params![id],
        )?;
        tx.execute(
            "DELETE FROM power_events WHERE session_id = ?1",
            params![id],
        )?;
        tx.execute(
            "INSERT INTO sessions (id, started_at, ended_at, hostname, model, ram_total_bytes, note)
             VALUES (?1,?2,?3,?4,?5,?6,?7)",
            params![
                id,
                meta.started_at,
                meta.ended_at,
                meta.hostname,
                meta.model,
                meta.ram_total_bytes,
                meta.note
            ],
        )?;
        tx.execute(
            "INSERT INTO system_samples (
                session_id, ts, power_state, cpu_total_pct, cpu_active_ratio, cpu_power_w,
                gpu_active_ratio, gpu_freq_mhz, gpu_power_w, ane_power_w, package_power_w,
                cpu_temp_c, gpu_temp_c, thermal_state, ram_total_bytes, ram_used_bytes,
                mem_app_bytes, mem_wired_bytes, mem_compressed_bytes, mem_inactive_bytes,
                mem_free_bytes, swap_total_bytes, swap_used_bytes
             )
             SELECT session_id, ts, power_state, cpu_total_pct, cpu_active_ratio, cpu_power_w,
                    gpu_active_ratio, gpu_freq_mhz, gpu_power_w, ane_power_w, package_power_w,
                    cpu_temp_c, gpu_temp_c, thermal_state, ram_total_bytes, ram_used_bytes,
                    mem_app_bytes, mem_wired_bytes, mem_compressed_bytes, mem_inactive_bytes,
                    mem_free_bytes, swap_total_bytes, swap_used_bytes
             FROM maindb.system_samples WHERE session_id = ?1",
            params![id],
        )?;
        tx.execute(
            "INSERT INTO process_samples (session_id, ts, pid, name, cpu_pct, mem_bytes, gpu_ms)
             SELECT session_id, ts, pid, name, cpu_pct, mem_bytes, gpu_ms
             FROM maindb.process_samples WHERE session_id = ?1",
            params![id],
        )?;
        tx.execute(
            "INSERT INTO power_events (session_id, ts, event_type, detail)
             SELECT session_id, ts, event_type, detail
             FROM maindb.power_events WHERE session_id = ?1",
            params![id],
        )?;
        tx.commit()?;
        data.execute_batch("DETACH DATABASE maindb;")?;

        conn.execute(
            "UPDATE sessions SET db_path = ?1 WHERE id = ?2",
            params![file.display().to_string(), id],
        )?;
        conn.execute(
            "DELETE FROM system_samples WHERE session_id = ?1",
            params![id],
        )?;
        conn.execute(
            "DELETE FROM process_samples WHERE session_id = ?1",
            params![id],
        )?;
        conn.execute(
            "DELETE FROM power_events WHERE session_id = ?1",
            params![id],
        )?;
    }
    Ok(())
}

/// 删除一个历史会话：移除主库索引行与独立数据文件（不可恢复）。
pub fn delete_session(conn: &Connection, session_id: i64) -> Result<()> {
    let stored: Option<Option<String>> = conn
        .query_row(
            "SELECT db_path FROM sessions WHERE id = ?1",
            params![session_id],
            |r| r.get(0),
        )
        .optional()?;

    // 主库：清理残留数据与索引行。
    conn.execute(
        "DELETE FROM system_samples WHERE session_id = ?1",
        params![session_id],
    )?;
    conn.execute(
        "DELETE FROM process_samples WHERE session_id = ?1",
        params![session_id],
    )?;
    conn.execute(
        "DELETE FROM power_events WHERE session_id = ?1",
        params![session_id],
    )?;
    conn.execute("DELETE FROM sessions WHERE id = ?1", params![session_id])?;

    // 独立数据文件（含 WAL/SHM 日志）。
    if let Some(Some(p)) = stored {
        let path = Path::new(&p);
        let _ = std::fs::remove_file(path);
        let _ = std::fs::remove_file(path.with_extension("db-wal"));
        let _ = std::fs::remove_file(path.with_extension("db-shm"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{now, now_iso, PowerEvent, PowerEventKind, ProcessSample, SystemSample};

    #[test]
    fn round_trip_samples_and_events() {
        let dir = std::env::temp_dir().join(format!("neko-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("t.db");
        let _ = std::fs::remove_file(&path);

        let mut st = Storage::open(&path, Some("Apple M-Test"), 16 << 30).unwrap();
        let sid = st.session_id();

        let sys = SystemSample {
            ts: now(),
            power_state: "awake".into(),
            cpu_total_pct: 12.5,
            cpu_active_ratio: Some(0.1),
            cpu_power_w: Some(1.0),
            gpu_active_ratio: Some(0.05),
            gpu_freq_mhz: Some(500.0),
            gpu_power_w: Some(0.5),
            ane_power_w: None,
            package_power_w: Some(2.0),
            cpu_temp_c: Some(45.0),
            gpu_temp_c: None,
            ram_total_bytes: 16 << 30,
            ram_used_bytes: 1000,
            mem_app_bytes: 500,
            mem_wired_bytes: 300,
            mem_compressed_bytes: 200,
            mem_inactive_bytes: 100,
            mem_free_bytes: 100,
            swap_total_bytes: 4096,
            swap_used_bytes: 0,
        };
        st.insert_system(&sys).unwrap();

        let ps = vec![ProcessSample {
            ts: now(),
            pid: 42,
            name: "test".into(),
            cpu_pct: 88.0,
            mem_bytes: 4096,
        }];
        st.insert_processes(&ps).unwrap();

        let ev = PowerEvent {
            ts: now(),
            kind: PowerEventKind::Sleep,
            detail: "x".into(),
        };
        st.insert_power_event(&ev).unwrap();
        st.finish(Some("note")).unwrap();

        let conn = Connection::open(&path).unwrap();
        let info = session_info(&conn, sid).unwrap().unwrap();
        assert_eq!(info.note.as_deref(), Some("note"));
        assert_eq!(info.ram_total_bytes, Some(16 << 30));

        let from = "1970-01-01T00:00:00+08:00";
        let to = now_iso();
        assert_eq!(
            process_aggregate(&conn, sid, from, &to, 10).unwrap().len(),
            1
        );
        assert_eq!(power_events_range(&conn, sid, from, &to).unwrap().len(), 1);
        assert_eq!(system_series(&conn, sid, from, &to).unwrap().len(), 1);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn process_aggregate_limit_zero_returns_all() {
        let dir = std::env::temp_dir().join(format!("neko-test-agg-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("t.db");
        let _ = std::fs::remove_file(&path);

        let mut st = Storage::open(&path, Some("Apple M-Test"), 16 << 30).unwrap();
        let sid = st.session_id();
        let ts = now();
        let ps: Vec<ProcessSample> = (1..=3)
            .map(|pid| ProcessSample {
                ts,
                pid,
                name: format!("proc{pid}"),
                cpu_pct: pid as f32,
                mem_bytes: 1024,
            })
            .collect();
        st.insert_processes(&ps).unwrap();

        let conn = Connection::open(&path).unwrap();
        let from = "1970-01-01T00:00:00+08:00";
        let to = now_iso();
        assert_eq!(
            process_aggregate(&conn, sid, from, &to, 0).unwrap().len(),
            3
        );
        assert_eq!(
            process_aggregate(&conn, sid, from, &to, 2).unwrap().len(),
            2
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn legacy_db_is_split_and_can_be_deleted() {
        let dir = std::env::temp_dir().join(format!("neko-test-split-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("t.db");
        let _ = std::fs::remove_file(&path);

        // 模拟旧版：数据直接写在主库表里。
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(SCHEMA).unwrap();
            conn.execute(
                "INSERT INTO sessions (started_at) VALUES (?1)",
                params![now_iso()],
            )
            .unwrap();
            let sid = conn.last_insert_rowid();
            conn.execute(
                "INSERT INTO system_samples (session_id, ts, cpu_total_pct)
                 VALUES (?1, ?2, ?3)",
                params![sid, now_iso(), 12.5],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO process_samples (session_id, ts, pid, name, cpu_pct, mem_bytes)
                 VALUES (?1, ?2, 1, 'a', 1.0, 1)",
                params![sid, now_iso()],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO power_events (session_id, ts, event_type, detail)
                 VALUES (?1, ?2, 'sleep', 'x')",
                params![sid, now_iso()],
            )
            .unwrap();
        }

        // 重新打开后 list_sessions 触发拆分。
        let conn = Connection::open(&path).unwrap();
        let sessions = list_sessions(&conn).unwrap();
        assert_eq!(sessions.len(), 1);
        let sid = sessions[0].id;
        assert_eq!(sessions[0].system_samples, 1);
        assert_eq!(sessions[0].process_samples, 1);

        // 主库数据表应已清空，数据都在独立文件里。
        let sys_main: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM system_samples WHERE session_id = ?1",
                params![sid],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(sys_main, 0);
        let file = session_file(&path, sid);
        assert!(file.exists());
        assert_eq!(
            system_series(&conn, sid, "1970-01-01T00:00:00+08:00", &now_iso())
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            power_events_range(&conn, sid, "1970-01-01T00:00:00+08:00", &now_iso())
                .unwrap()
                .len(),
            1
        );

        // 删除会话：索引行与独立文件一起消失。
        delete_session(&conn, sid).unwrap();
        assert!(list_sessions(&conn).unwrap().is_empty());
        assert!(!file.exists());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn sessions_use_separate_files_with_fixed_ids() {
        let dir = std::env::temp_dir().join(format!("neko-test-files-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("t.db");
        let _ = std::fs::remove_file(&path);

        let mut st1 = Storage::open(&path, Some("M"), 8 << 30).unwrap();
        let sid1 = st1.session_id();
        let mut st2 = Storage::open(&path, Some("M"), 8 << 30).unwrap();
        let sid2 = st2.session_id();
        assert_ne!(sid1, sid2);

        st1.insert_processes(&[ProcessSample {
            ts: now(),
            pid: 1,
            name: "s1".into(),
            cpu_pct: 1.0,
            mem_bytes: 1,
        }])
        .unwrap();
        st2.insert_processes(&[ProcessSample {
            ts: now(),
            pid: 2,
            name: "s2".into(),
            cpu_pct: 2.0,
            mem_bytes: 2,
        }])
        .unwrap();

        let f1 = session_file(&path, sid1);
        let f2 = session_file(&path, sid2);
        assert!(f1.exists() && f2.exists());
        assert_ne!(f1, f2);

        let conn = Connection::open(&path).unwrap();
        let sessions = list_sessions(&conn).unwrap();
        assert_eq!(sessions.len(), 2);
        assert_eq!(sessions[0].id, sid2); // 按 id 倒序
        assert_eq!(sessions[0].process_samples, 1);
        assert_eq!(sessions[1].id, sid1);

        // 各自文件里只有自己的数据。
        let c1 = Connection::open(&f1).unwrap();
        let n1: i64 = c1
            .query_row(
                "SELECT COUNT(*) FROM process_samples WHERE session_id = ?1",
                params![sid1],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n1, 1);
        let c2 = Connection::open(&f2).unwrap();
        let n2: i64 = c2
            .query_row(
                "SELECT COUNT(*) FROM process_samples WHERE session_id = ?1",
                params![sid2],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n2, 1);

        // 删除一个会话不影响另一个。
        delete_session(&conn, sid1).unwrap();
        assert!(!f1.exists());
        assert!(f2.exists());
        let sessions = list_sessions(&conn).unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].id, sid2);

        let _ = std::fs::remove_dir_all(&dir);
    }
}
