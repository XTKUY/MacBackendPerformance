use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use sysinfo::{ProcessesToUpdate, System};

use crate::macos_mem;
use crate::model::{now, ProcessSample, SampleMsg, SharedCtx, SystemSample};
use crate::record::RuntimeConfig;

/// 进程采样线程：刷新 sysinfo，按 CPU 排序取 Top N。
pub fn spawn_process_sampler(
    runtime: Arc<RuntimeConfig>,
    soc_enabled: bool,
    ctx: Arc<Mutex<SharedCtx>>,
    write_tx: Sender<SampleMsg>,
    ui_tx: Sender<SampleMsg>,
    stop: Arc<AtomicBool>,
) -> JoinHandle<()> {
    thread::spawn(move || {
        let mut sys = System::new_all();
        sys.refresh_cpu_usage();
        sys.refresh_processes(ProcessesToUpdate::All, true);
        sys.refresh_memory();

        while !stop.load(Ordering::Relaxed) {
            let interval_ms = runtime.process_interval_ms.load(Ordering::Relaxed);
            thread::sleep(Duration::from_millis(interval_ms));
            if stop.load(Ordering::Relaxed) {
                break;
            }

            sys.refresh_cpu_usage();
            sys.refresh_processes(ProcessesToUpdate::All, true);
            sys.refresh_memory();

            let cpu_total = sys.global_cpu_usage();

            // 活动监视器口径的内存统计（host_statistics64 + vm.swapusage）。
            let mem = macos_mem::read();
            let mem = mem.unwrap_or_else(|| macos_mem::MemStats {
                total_bytes: sys.total_memory(),
                used_bytes: sys.used_memory(),
                app_bytes: sys.used_memory(),
                wired_bytes: 0,
                compressed_bytes: 0,
                inactive_bytes: 0,
                free_bytes: sys.free_memory(),
                swap_total_bytes: sys.total_swap(),
                swap_used_bytes: sys.used_swap(),
            });

            {
                let mut c = ctx.lock().unwrap();
                c.cpu_total_pct = cpu_total;
                c.ram_total_bytes = mem.total_bytes;
                c.ram_used_bytes = mem.used_bytes;
                c.mem_app_bytes = mem.app_bytes;
                c.mem_wired_bytes = mem.wired_bytes;
                c.mem_compressed_bytes = mem.compressed_bytes;
                c.mem_inactive_bytes = mem.inactive_bytes;
                c.mem_free_bytes = mem.free_bytes;
                c.swap_total_bytes = mem.swap_total_bytes;
                c.swap_used_bytes = mem.swap_used_bytes;
            }

            let ts = now();
            let mut rows: Vec<ProcessSample> = sys
                .processes()
                .iter()
                .map(|(pid, p)| ProcessSample {
                    ts,
                    pid: pid.as_u32() as i32,
                    name: p.name().to_string_lossy().into_owned(),
                    cpu_pct: p.cpu_usage(),
                    mem_bytes: p.memory(),
                })
                .collect();
            rows.sort_by(|a, b| b.cpu_pct.total_cmp(&a.cpu_pct));
            let top_n = runtime.top_n.load(Ordering::Relaxed).max(1);
            rows.truncate(top_n);

            if !rows.is_empty() {
                let _ = write_tx.send(SampleMsg::Processes(rows.clone()));
                let _ = ui_tx.send(SampleMsg::Processes(rows));
            }

            // 未启用 GPU 采集时，由进程线程负责产生系统行（SoC 字段为 None）。
            if !soc_enabled {
                let c = ctx.lock().unwrap().clone();
                let s = SystemSample {
                    ts,
                    power_state: c.power_state.clone(),
                    cpu_total_pct: c.cpu_total_pct,
                    cpu_active_ratio: None,
                    cpu_power_w: None,
                    gpu_active_ratio: None,
                    gpu_freq_mhz: None,
                    gpu_power_w: None,
                    ane_power_w: None,
                    package_power_w: None,
                    cpu_temp_c: None,
                    gpu_temp_c: None,
                    ram_total_bytes: c.ram_total_bytes,
                    ram_used_bytes: c.ram_used_bytes,
                    mem_app_bytes: c.mem_app_bytes,
                    mem_wired_bytes: c.mem_wired_bytes,
                    mem_compressed_bytes: c.mem_compressed_bytes,
                    mem_inactive_bytes: c.mem_inactive_bytes,
                    mem_free_bytes: c.mem_free_bytes,
                    swap_total_bytes: c.swap_total_bytes,
                    swap_used_bytes: c.swap_used_bytes,
                };
                let _ = write_tx.send(SampleMsg::System(s.clone()));
                let _ = ui_tx.send(SampleMsg::System(s));
            }
        }
    })
}

/// SoC 采样线程：macmon（IOReport）读取 GPU/功耗/温度，阻塞式采样。
pub fn spawn_soc_sampler(
    runtime: Arc<RuntimeConfig>,
    ctx: Arc<Mutex<SharedCtx>>,
    write_tx: Sender<SampleMsg>,
    ui_tx: Sender<SampleMsg>,
    stop: Arc<AtomicBool>,
) -> JoinHandle<()> {
    thread::spawn(move || {
        let mut sampler = match macmon::Sampler::new() {
            Ok(s) => s,
            Err(e) => {
                let detail = format!("IOReport 采样器初始化失败（GPU/功耗数据不可用）: {e}");
                let msg = SampleMsg::SocUnavailable(detail);
                let _ = write_tx.send(SampleMsg::SocUnavailable(
                    "IOReport 采样器初始化失败（GPU/功耗数据不可用）".to_string(),
                ));
                let _ = ui_tx.send(msg);
                return;
            }
        };

        while !stop.load(Ordering::Relaxed) {
            let interval_ms = runtime.interval_ms.load(Ordering::Relaxed);
            let m = match sampler.get_metrics(interval_ms as u32) {
                Ok(m) => m,
                Err(_) => continue,
            };
            if stop.load(Ordering::Relaxed) {
                break;
            }

            let c = ctx.lock().unwrap().clone();
            let s = SystemSample {
                ts: now(),
                power_state: c.power_state.clone(),
                cpu_total_pct: c.cpu_total_pct,
                cpu_active_ratio: Some(m.cpu_active_ratio),
                cpu_power_w: Some(m.cpu_power),
                gpu_active_ratio: Some(m.gpu_active_ratio),
                gpu_freq_mhz: Some(m.gpu_freq_mhz as f32),
                gpu_power_w: Some(m.gpu_power),
                ane_power_w: Some(m.ane_power),
                package_power_w: Some(m.all_power),
                cpu_temp_c: Some(m.temp.cpu_temp_avg),
                gpu_temp_c: Some(m.temp.gpu_temp_avg),
                ram_total_bytes: c.ram_total_bytes,
                ram_used_bytes: c.ram_used_bytes,
                mem_app_bytes: c.mem_app_bytes,
                mem_wired_bytes: c.mem_wired_bytes,
                mem_compressed_bytes: c.mem_compressed_bytes,
                mem_inactive_bytes: c.mem_inactive_bytes,
                mem_free_bytes: c.mem_free_bytes,
                swap_total_bytes: c.swap_total_bytes,
                swap_used_bytes: c.swap_used_bytes,
            };
            let _ = write_tx.send(SampleMsg::System(s.clone()));
            let _ = ui_tx.send(SampleMsg::System(s));
        }
    })
}

/// 返回芯片型号（供 sessions.model 使用）。
pub fn chip_model() -> Option<String> {
    let out = std::process::Command::new("sysctl")
        .args(["-n", "machdep.cpu.brand_string"])
        .output()
        .ok()?;
    if out.status.success() {
        let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if !s.is_empty() {
            return Some(s);
        }
    }
    None
}
