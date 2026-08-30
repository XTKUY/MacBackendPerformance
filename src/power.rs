use std::ffi::{c_char, c_void};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crate::model::{now, PowerEvent, PowerEventKind, SampleMsg};

// 来自 IOKit/IOMessage.h（用户态消息，高位 0xE0000000 | sub_iokit_common | message）。
const KIO_MESSAGE_CAN_SYSTEM_SLEEP: u32 = 0xe000_0040;
const KIO_MESSAGE_SYSTEM_WILL_SLEEP: u32 = 0xe000_0280;
const KIO_MESSAGE_SYSTEM_WILL_NOT_SLEEP: u32 = 0xe000_0290;
const KIO_MESSAGE_SYSTEM_HAS_POWERED_ON: u32 = 0xe000_0300;
const KIO_MESSAGE_SYSTEM_WILL_POWER_ON: u32 = 0xe000_0320;

type IOServiceInterestCallback = unsafe extern "C" fn(
    refcon: *mut c_void,
    service: u32,
    message_type: u32,
    message_arg: *mut c_void,
);

#[link(name = "IOKit", kind = "framework")]
extern "C" {
    fn IORegisterForSystemPower(
        refcon: *mut c_void,
        the_port_ref: *mut *mut c_void,
        callback: Option<IOServiceInterestCallback>,
        notifier: *mut *mut c_void,
    ) -> u32;
    fn IODeregisterForSystemPower(notifier: *mut *mut c_void) -> i32;
    fn IOServiceClose(connection: u32) -> i32;
    fn IONotificationPortCreate(master_port: u32) -> *mut c_void;
    fn IONotificationPortGetRunLoopSource(notify_port: *mut c_void) -> *mut c_void;
    fn IONotificationPortDestroy(notify_port: *mut c_void);
    fn IOAllowPowerChange(kernel_port: u32, notification_id: isize) -> i32;
}

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    fn CFRunLoopGetCurrent() -> *mut c_void;
    fn CFRunLoopAddSource(rl: *mut c_void, source: *mut c_void, mode: *mut c_void);
    fn CFRunLoopRemoveSource(rl: *mut c_void, source: *mut c_void, mode: *mut c_void);
    fn CFRunLoopRun();
    fn CFRunLoopStop(rl: *mut c_void);
    fn CFRunLoopAddTimer(rl: *mut c_void, timer: *mut c_void, mode: *mut c_void);
    fn CFRunLoopTimerInvalidate(timer: *mut c_void);
    fn CFRunLoopTimerCreate(
        allocator: *mut c_void,
        fire_date: f64,
        interval: f64,
        flags: u32,
        order: i32,
        callout: Option<unsafe extern "C" fn(*mut c_void, *mut c_void)>,
        context: *const CFRunLoopTimerContext,
    ) -> *mut c_void;
    fn CFAbsoluteTimeGetCurrent() -> f64;
    fn CFStringCreateWithCString(
        alloc: *mut c_void,
        c_str: *const c_char,
        encoding: u32,
    ) -> *mut c_void;
    fn CFRelease(cf: *mut c_void);
}

#[repr(C)]
struct CFRunLoopTimerContext {
    version: isize,
    info: *mut c_void,
    retain: Option<unsafe extern "C" fn(*const c_void) -> *const c_void>,
    release: Option<unsafe extern "C" fn(*const c_void)>,
    copy_description: Option<unsafe extern "C" fn(*const c_void) -> *const c_void>,
}

struct PowerCtx {
    write_tx: Sender<SampleMsg>,
    ui_tx: Sender<SampleMsg>,
    conn: u32,
}

impl PowerCtx {
    fn emit(&self, kind: PowerEventKind, detail: &str) {
        let ev = PowerEvent {
            ts: now(),
            kind,
            detail: detail.to_string(),
        };
        let _ = self.write_tx.send(SampleMsg::Power(ev.clone()));
        let _ = self.ui_tx.send(SampleMsg::Power(ev));
    }
}

unsafe extern "C" fn power_callback(
    refcon: *mut c_void,
    _service: u32,
    message_type: u32,
    message_arg: *mut c_void,
) {
    let ctx = &*(refcon as *const PowerCtx);
    match message_type {
        KIO_MESSAGE_SYSTEM_WILL_SLEEP => {
            // 必须应答 IOAllowPowerChange，否则系统会等待并可能推迟/放弃本次睡眠。
            IOAllowPowerChange(ctx.conn, message_arg as isize);
            ctx.emit(PowerEventKind::Sleep, "system will sleep");
        }
        KIO_MESSAGE_SYSTEM_HAS_POWERED_ON => {
            ctx.emit(PowerEventKind::Wake, "system has powered on")
        }
        KIO_MESSAGE_SYSTEM_WILL_POWER_ON => {
            ctx.emit(PowerEventKind::Wake, "early wake (will power on)")
        }
        KIO_MESSAGE_SYSTEM_WILL_NOT_SLEEP => {
            ctx.emit(PowerEventKind::WillNotSleep, "idle sleep vetoed")
        }
        KIO_MESSAGE_CAN_SYSTEM_SLEEP => {
            // 允许系统进入空闲睡眠（不阻塞、不推迟）。
            IOAllowPowerChange(ctx.conn, message_arg as isize);
        }
        _ => {}
    }
}

/// 低频（0.5s）唤醒 run loop 检查退出标志；置位后停止 run loop。
unsafe extern "C" fn stop_timer_callout(_timer: *mut c_void, info: *mut c_void) {
    let stop = &*(info as *const AtomicBool);
    if stop.load(Ordering::SeqCst) {
        CFRunLoopStop(CFRunLoopGetCurrent());
    }
}

/// 启动 IOKit 电源事件监听线程（CFRunLoop 阻塞运行，低频定时器检查退出标志）。
pub fn spawn_power_monitor(
    write_tx: Sender<SampleMsg>,
    ui_tx: Sender<SampleMsg>,
    stop: Arc<AtomicBool>,
) -> JoinHandle<()> {
    thread::spawn(move || {
        unsafe {
            let port = IONotificationPortCreate(0);
            if port.is_null() {
                eprintln!("[neko-perf] IONotificationPortCreate 失败，电源事件不可用");
                return;
            }
            let ctx = Box::into_raw(Box::new(PowerCtx {
                write_tx,
                ui_tx,
                conn: 0,
            }));
            let mut notifier: *mut c_void = std::ptr::null_mut();
            let mut port_ref: *mut c_void = port;
            let conn = IORegisterForSystemPower(
                ctx as *mut c_void,
                &mut port_ref,
                Some(power_callback),
                &mut notifier,
            );
            if conn == 0 {
                eprintln!("[neko-perf] IORegisterForSystemPower 注册失败，电源事件不可用");
                let _ = Box::from_raw(ctx);
                IONotificationPortDestroy(port);
                return;
            }
            (*ctx).conn = conn;

            let source = IONotificationPortGetRunLoopSource(port);
            // kCFRunLoopCommonModes 是伪模式，只能用于注册 source，不能直接 run。
            let common_mode = CFStringCreateWithCString(
                std::ptr::null_mut(),
                c"kCFRunLoopCommonModes".as_ptr(),
                0x0800_0100, // kCFStringEncodingUTF8
            );
            let default_mode = CFStringCreateWithCString(
                std::ptr::null_mut(),
                c"kCFRunLoopDefaultMode".as_ptr(),
                0x0800_0100, // kCFStringEncodingUTF8
            );
            let rl = CFRunLoopGetCurrent();
            // 同时挂到 common modes 和 default mode：CFRunLoopRun() 跑在 default mode，
            // 显式加入 default mode 保证 IOKit 通知一定被当前运行的模式调度到。
            CFRunLoopAddSource(rl, source, common_mode);
            CFRunLoopAddSource(rl, source, default_mode);

            // 阻塞式运行；用定时器定期检查 stop，避免忙轮询占满一个核。
            let stop_ptr = Arc::into_raw(stop.clone());
            let timer_ctx = CFRunLoopTimerContext {
                version: 0,
                info: stop_ptr as *mut c_void,
                retain: None,
                release: None,
                copy_description: None,
            };
            let timer = CFRunLoopTimerCreate(
                std::ptr::null_mut(),
                CFAbsoluteTimeGetCurrent() + 0.5,
                0.5,
                0,
                0,
                Some(stop_timer_callout),
                &timer_ctx,
            );
            if !timer.is_null() {
                CFRunLoopAddTimer(rl, timer, default_mode);
            }

            CFRunLoopRun();

            if !timer.is_null() {
                CFRunLoopTimerInvalidate(timer);
                CFRelease(timer);
            }
            let _ = Arc::from_raw(stop_ptr);
            CFRunLoopRemoveSource(rl, source, common_mode);
            CFRelease(common_mode);
            CFRelease(default_mode);
            IODeregisterForSystemPower(&mut notifier);
            IOServiceClose(conn);
            IONotificationPortDestroy(port);
            let _ = Box::from_raw(ctx);
        }
    })
}

/// 定期抓取 `pmset -g assertions`，仅在内容变化时记录事件。
pub fn spawn_assertion_monitor(
    write_tx: Sender<SampleMsg>,
    ui_tx: Sender<SampleMsg>,
    stop: Arc<AtomicBool>,
) -> JoinHandle<()> {
    thread::spawn(move || {
        let mut last: Option<String> = None;
        while !stop.load(Ordering::Relaxed) {
            thread::sleep(Duration::from_secs(60));
            if stop.load(Ordering::Relaxed) {
                break;
            }
            let Some(text) = pmset_assertions(Duration::from_secs(10)) else {
                continue;
            };
            let relevant: Vec<String> = text
                .lines()
                .filter(|l| l.contains("Prevent") || l.contains("named:"))
                .map(|l| l.trim().to_string())
                .collect();
            if relevant.is_empty() {
                continue;
            }
            // 用去掉时长字段的摘要做变化检测：pmset 里的倒计时 / 累计时长
            // （如 00:33:10、240:20:03）每分钟都在变，会误判为断言变化。
            let digest = relevant
                .iter()
                .map(|l| strip_durations(l))
                .collect::<Vec<_>>()
                .join("\n");
            if Some(&digest) != last.as_ref() {
                last = Some(digest.clone());
                let raw = relevant.join("\n");
                let detail = if raw.len() > 600 {
                    format!("{} …（截断）", &raw[..600])
                } else {
                    raw
                };
                let ev = PowerEvent {
                    ts: now(),
                    kind: PowerEventKind::AssertionChanged,
                    detail,
                };
                let _ = write_tx.send(SampleMsg::Power(ev.clone()));
                let _ = ui_tx.send(SampleMsg::Power(ev));
            }
        }
    })
}

/// 抓取 `pmset -g assertions`，带超时保护；超时返回 None（跳过本轮）。
fn pmset_assertions(timeout: Duration) -> Option<String> {
    let mut child = Command::new("pmset")
        .args(["-g", "assertions"])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .ok()?;
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait().ok()? {
            Some(_) => break,
            None => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    return None;
                }
                thread::sleep(Duration::from_millis(50));
            }
        }
    }
    let out = child.wait_with_output().ok()?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// 把行内的时间字段（如 `00:00:34`、`240:20:03`）替换为 `<t>`，
/// 便于比较断言集合是否真的发生变化。
fn strip_durations(line: &str) -> String {
    line.split(' ')
        .map(|tok| if is_duration(tok) { "<t>" } else { tok })
        .collect::<Vec<_>>()
        .join(" ")
}

/// 判断 token 是否为 `H:MM:SS`（小时 1-3 位）形式的时长。
fn is_duration(tok: &str) -> bool {
    let parts: Vec<&str> = tok.split(':').collect();
    if parts.len() != 3 {
        return false;
    }
    let (h, m, s) = (parts[0], parts[1], parts[2]);
    !h.is_empty()
        && h.len() <= 3
        && h.bytes().all(|b| b.is_ascii_digit())
        && m.len() == 2
        && m.bytes().all(|b| b.is_ascii_digit())
        && s.len() == 2
        && s.bytes().all(|b| b.is_ascii_digit())
}

#[cfg(test)]
mod tests {
    use super::{is_duration, strip_durations};

    #[test]
    fn duration_tokens_are_recognized() {
        assert!(is_duration("00:00:34"));
        assert!(is_duration("240:20:03"));
        assert!(is_duration("1:02:03"));
        assert!(!is_duration("1234:20:03")); // 小时超过 3 位
        assert!(!is_duration("00:0:03")); // 分不是 2 位
        assert!(!is_duration("0x000b449400019b45"));
        assert!(!is_duration("DASActivity:501:com.apple"));
        assert!(!is_duration("PreventSystemSleep"));
    }

    #[test]
    fn durations_are_stripped_but_content_kept() {
        assert_eq!(
            strip_durations("PreventUserIdleDisplaySleep    0"),
            "PreventUserIdleDisplaySleep    0"
        );
        assert_eq!(
            strip_durations(
                "pid 616(dasd): [0x000b3c20000b98e0] 00:33:10 BackgroundTask named: \"DASActivity:501:com.apple.Safari.SafeBrowsing.BrowsingDatabases.Update\""
            ),
            "pid 616(dasd): [0x000b3c20000b98e0] <t> BackgroundTask named: \"DASActivity:501:com.apple.Safari.SafeBrowsing.BrowsingDatabases.Update\""
        );
        assert_eq!(
            strip_durations(
                "pid 578(powerd): [0x000000690008809b] 251:48:00 ExternalMedia named: \"com.apple.powermanagement.externalmediamounted\""
            ),
            "pid 578(powerd): [0x000000690008809b] <t> ExternalMedia named: \"com.apple.powermanagement.externalmediamounted\""
        );
    }
}
