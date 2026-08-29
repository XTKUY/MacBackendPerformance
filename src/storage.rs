use std::path::Path;

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
  note            TEXT
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
    Ok(())
}

/// 由写入线程独占持有的存储句柄。
pub struct Storage {
    conn: Connection,
    session_id: i64,
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
        conn.execute(
            "INSERT INTO sessions (started_at, hostname, model, ram_total_bytes)
             VALUES (?1, ?2, ?3, ?4)",
            params![now_iso(), hostname, model, ram_total_bytes as i64],
        )?;
        let session_id = conn.last_insert_rowid();
        Ok(Storage { conn, session_id })
    }

    pub fn session_id(&self) -> i64 {
        self.session_id
    }

    pub fn insert_system(&mut self, s: &SystemSample) -> Result<()> {
        self.conn.execute(
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
        let tx = self.conn.transaction()?;
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
        self.conn.execute(
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
        self.conn.execute(
            "UPDATE sessions SET ended_at = ?1, note = COALESCE(?2, note) WHERE id = ?3",
            params![now_iso(), note, self.session_id],
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

pub fn list_sessions(conn: &Connection) -> Result<Vec<SessionInfo>> {
    let mut stmt = conn.prepare(
        "SELECT s.id, s.started_at, s.ended_at, s.hostname, s.model, s.ram_total_bytes, s.note,
                (SELECT COUNT(*) FROM system_samples x WHERE x.session_id = s.id) AS sys_cnt,
                (SELECT COUNT(*) FROM process_samples p WHERE p.session_id = s.id) AS proc_cnt
         FROM sessions s ORDER BY s.id DESC",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok(SessionInfo {
            id: r.get(0)?,
            started_at: r.get(1)?,
            ended_at: r.get(2)?,
            hostname: r.get(3)?,
            model: r.get(4)?,
            ram_total_bytes: r.get(5)?,
            note: r.get(6)?,
            system_samples: r.get(7)?,
            process_samples: r.get(8)?,
        })
    })?;
    rows.collect()
}

pub fn session_info(conn: &Connection, id: i64) -> Result<Option<SessionInfo>> {
    conn.query_row(
        "SELECT s.id, s.started_at, s.ended_at, s.hostname, s.model, s.ram_total_bytes, s.note,
                (SELECT COUNT(*) FROM system_samples x WHERE x.session_id = s.id) AS sys_cnt,
                (SELECT COUNT(*) FROM process_samples p WHERE p.session_id = s.id) AS proc_cnt
         FROM sessions s WHERE s.id = ?1",
        params![id],
        |r| {
            Ok(SessionInfo {
                id: r.get(0)?,
                started_at: r.get(1)?,
                ended_at: r.get(2)?,
                hostname: r.get(3)?,
                model: r.get(4)?,
                ram_total_bytes: r.get(5)?,
                note: r.get(6)?,
                system_samples: r.get(7)?,
                process_samples: r.get(8)?,
            })
        },
    )
    .optional()
}

pub fn system_series(
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
}
