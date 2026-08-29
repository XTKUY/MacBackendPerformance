use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use crate::model::{SampleMsg, SharedCtx};
use crate::sampler::{chip_model, spawn_process_sampler, spawn_soc_sampler};
use crate::storage::Storage;

/// 运行时可调参数（TUI 设置界面实时修改采样间隔 / Top N）。
pub struct RuntimeConfig {
    pub interval_ms: AtomicU64,
    pub process_interval_ms: AtomicU64,
    pub top_n: AtomicUsize,
}

impl RuntimeConfig {
    pub fn new(interval_ms: u64, process_interval_ms: u64, top_n: usize) -> Arc<RuntimeConfig> {
        Arc::new(RuntimeConfig {
            interval_ms: AtomicU64::new(interval_ms),
            process_interval_ms: AtomicU64::new(process_interval_ms),
            top_n: AtomicUsize::new(top_n),
        })
    }
}

pub struct RecordConfig {
    pub db_path: PathBuf,
    pub interval_ms: u64,
    pub process_interval_ms: u64,
    pub top_n: usize,
    pub gpu: bool,
    pub note: Option<String>,
}

impl RecordConfig {
    pub fn from_app_config(cfg: &crate::config::AppConfig) -> RecordConfig {
        RecordConfig {
            db_path: cfg.db_path.clone(),
            interval_ms: cfg.interval_ms,
            process_interval_ms: cfg.process_interval_ms,
            top_n: cfg.top_n,
            gpu: cfg.gpu,
            note: None,
        }
    }
}

/// 采集编排器：启动各采集线程，统一写入 SQLite。
pub struct Recorder {
    pub stop: Arc<AtomicBool>,
    pub ui_rx: Receiver<SampleMsg>,
    pub session_id: i64,
    pub db_path: PathBuf,
    write_tx: Sender<SampleMsg>,
    handles: Vec<JoinHandle<()>>,
}

impl Recorder {
    pub fn start(cfg: RecordConfig, runtime: Arc<RuntimeConfig>) -> Result<Recorder, String> {
        let stop = Arc::new(AtomicBool::new(false));
        let ctx: Arc<Mutex<SharedCtx>> = Arc::new(Mutex::new(SharedCtx {
            power_state: "awake".to_string(),
            ..Default::default()
        }));

        let ram_total = sysinfo::System::new_all().total_memory();
        let model = chip_model();
        let mut storage = Storage::open(&cfg.db_path, model.as_deref(), ram_total)
            .map_err(|e| format!("打开数据库失败 {}: {e}", cfg.db_path.display()))?;
        let session_id = storage.session_id();

        let (write_tx, write_rx) = channel::<SampleMsg>();
        let (ui_tx, ui_rx) = channel::<SampleMsg>();

        let mut handles = Vec::new();

        // 写入线程：独占 Storage。
        handles.push(std::thread::spawn(move || {
            loop {
                match write_rx.recv() {
                    Ok(SampleMsg::System(s)) => {
                        if let Err(e) = storage.insert_system(&s) {
                            eprintln!("[neko-perf] 写入 system_samples 失败: {e}");
                        }
                    }
                    Ok(SampleMsg::Processes(rows)) => {
                        if let Err(e) = storage.insert_processes(&rows) {
                            eprintln!("[neko-perf] 写入 process_samples 失败: {e}");
                        }
                    }
                    Ok(SampleMsg::Power(ev)) => {
                        if let Err(e) = storage.insert_power_event(&ev) {
                            eprintln!("[neko-perf] 写入 power_events 失败: {e}");
                        }
                        // 同步共享电源状态，供系统行标记。
                    }
                    Ok(SampleMsg::SocUnavailable(m)) => {
                        eprintln!("[neko-perf] {m}");
                    }
                    Ok(SampleMsg::Stop) | Err(_) => break,
                }
            }
            let _ = storage.finish(None);
        }));

        // 进程采样线程。
        let p_stop = stop.clone();
        let p_wt = write_tx.clone();
        let p_ut = ui_tx.clone();
        let p_ctx = ctx.clone();
        handles.push(spawn_process_sampler(
            runtime.clone(),
            cfg.gpu,
            p_ctx,
            p_wt,
            p_ut,
            p_stop,
        ));

        // SoC（GPU/功耗/温度）采样线程。
        if cfg.gpu {
            let s_stop = stop.clone();
            let s_wt = write_tx.clone();
            let s_ut = ui_tx.clone();
            let s_ctx = ctx.clone();
            handles.push(spawn_soc_sampler(
                runtime.clone(),
                s_ctx,
                s_wt,
                s_ut,
                s_stop,
            ));
        }

        // 电源事件线程（不 join，退出时随进程结束；run loop 0.2s 轮询自行退出）。
        let pw_stop = stop.clone();
        let _pw_handle =
            crate::power::spawn_power_monitor(write_tx.clone(), ui_tx.clone(), pw_stop);

        // assertions 快照线程（60s 一次，不 join）。
        let as_stop = stop.clone();
        let _as_handle = crate::power::spawn_assertion_monitor(write_tx.clone(), ui_tx, as_stop);

        Ok(Recorder {
            stop,
            ui_rx,
            session_id,
            db_path: cfg.db_path,
            write_tx: write_tx.clone(),
            handles,
        })
    }

    pub fn stop(self) {
        self.stop.store(true, Ordering::SeqCst);
        let _ = self.write_tx.send(SampleMsg::Stop);
        for h in self.handles {
            let _ = h.join();
        }
    }
}

pub fn session_started_note(cfg: &RecordConfig) -> String {
    let gpu = if cfg.gpu {
        "GPU 采集开启"
    } else {
        "GPU 采集关闭"
    };
    format!(
        "interval={}ms process_interval={}ms top_n={} {}",
        cfg.interval_ms, cfg.process_interval_ms, cfg.top_n, gpu
    )
}

/// 无 TUI 的后台采集模式（适合 tmux / nohup）。
pub fn run_daemon(args: &crate::cli::DaemonArgs) -> Result<(), String> {
    let app_cfg = args.merged_config(&crate::config::load());
    let cfg = RecordConfig::from_app_config(&app_cfg);
    let runtime = RuntimeConfig::new(cfg.interval_ms, cfg.process_interval_ms, cfg.top_n);
    let recorder = Recorder::start(cfg, runtime)?;
    let (tx, rx) = std::sync::mpsc::channel::<()>();
    ctrlc::set_handler(move || {
        let _ = tx.send(());
    })
    .map_err(|e| format!("注册 Ctrl+C 处理失败: {e}"))?;

    let db = recorder.db_path.display().to_string();
    println!(
        "neko-perf 后台采集已启动：会话 {}，DB {db}",
        recorder.session_id
    );
    println!("按 Ctrl+C 停止并保存。");
    let _ = rx.recv();
    println!("\n收到 Ctrl+C，正在收尾…");
    recorder.stop();
    println!("已停止。");
    println!("  neko-perf inspect --db {db}");
    println!("  neko-perf report --db {db} --format md");
    Ok(())
}
