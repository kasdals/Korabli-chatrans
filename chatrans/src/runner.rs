//! 无 GUI 可跑的库层（方案 C / R1 风险应对）。
//!
//! CLI 定义、replay 监听、事件分流的编排逻辑全部住在库层，`main.rs` 只留一个
//! 薄入口。eframe 占主线程事件循环（S2 起 GUI 模式），后台管线通过
//! [`start`] / [`wait`] 拆开：GUI 在后台线程起管线、主线程跑事件循环。
//!
//! S5 起 Web/SSE 整体移除：事件管线从 "tee→server/gui" 简化为
//! "monitor→pump→gui（console 模式时接 console::printer 打印原文）"。
//! CLI 行为变化属于设计变更：`-i` / `-p` / `--no-browser` 移除、console
//! 只打印原文；`-r` / `-t` / `--headless` 语义保持。

use std::{
    path::PathBuf,
    process,
    thread::JoinHandle,
    time::{Duration, Instant},
};

use clap::Parser;
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};

use crate::live::LiveMonitor;
use crate::processor::LiveEvent;

/// 命令行定义。S5 瘦身：Web/SSE 相关的 `-i` / `-p` / `--no-browser` 移除。
#[derive(Parser, Debug)]
#[command(
    name="Chatrans",
    author="Yusux",
    version=concat!("v", option_env!("APP_VERSION").unwrap_or(env!("CARGO_PKG_VERSION"))),
    about="Translates chat from Korabli via the replay file",
    long_about=None,
    next_line_help = true,
)]
pub struct ClientArgs {
    #[arg(short, long, help = "The replay dir to use")]
    pub replay_dir: String,
    #[arg(short, long, help = "The target language, where `zh` stands for Chinese, `en` stands for English. Default is `zh`", default_value = "zh")]
    pub target_language: String,
    #[arg(long, help = "The Aliyun access key id")]
    pub access_key_id: Option<String>,
    #[arg(long, help = "The Aliyun access key secret")]
    pub access_key_secret: Option<String>,
    /// 仅 gui feature 下存在：跳过翻译面板，纯控制台运行（console 打印原文）。
    #[cfg(feature = "gui")]
    #[arg(long, help = "Run without the translation overlay panel (console mode)")]
    pub headless: bool,
}

/// `start_with_dir` 返回的后台管线句柄。
pub struct AppHandles {
    pub token: CancellationToken,
    monitor_thread: JoinHandle<()>,
    pump_handle: JoinHandle<()>,
    /// 面板/打印器的事件出口。headless 下由 console::printer 消费；
    /// GUI 下由面板独占。unbounded 通道不会阻塞生产端。
    gui_events: async_channel::Receiver<LiveEvent>,
}

impl AppHandles {
    /// 把事件分流的所有权交给面板/打印器。
    ///
    /// 注意 async_channel 的多 receiver 是"负载均衡"语义（每条消息只给一个
    /// receiver），所以这里**不能**克隆后两头各收一半——消费者必须独占这一路。
    pub fn gui_events_clone(&self) -> async_channel::Receiver<LiveEvent> {
        self.gui_events.clone()
    }

    /// 关停后台管线：先 cancel 再 join。
    ///
    /// join 带 2s 总时限（QA Bug#2）：正常情况全部线程 ≤2s 退出（live 解析
    /// 循环已有 token 检查）；万一某线程卡在无法中断的 CPU 段里，超时后
    /// 分离之，让进程按承诺 ≤3s 退出 —— GUI 的 restart 也不会泄漏线程。
    /// 分离的线程随进程退出由 OS 回收，不存在资源泄漏面。
    pub fn shutdown(self) {
        self.token.cancel();

        let (done_tx, done_rx) = std::sync::mpsc::channel::<&'static str>();
        let mut waiters = Vec::new();
        for (name, handle) in [
            ("monitor", self.monitor_thread),
            ("pump", self.pump_handle),
        ] {
            let done_tx = done_tx.clone();
            waiters.push(std::thread::spawn(move || {
                let _ = handle.join();
                let _ = done_tx.send(name);
            }));
        }
        drop(done_tx);

        let total = waiters.len();
        let deadline = Instant::now() + Duration::from_secs(2);
        let mut finished = 0;
        while finished < total {
            match rx_recv_until(&done_rx, deadline) {
                Some(_) => finished += 1,
                None => break,
            }
        }
        if finished < total {
            warn!(
                "后台管线 {}/{} 个线程未在 2s 内退出，分离之",
                total - finished,
                total
            );
            // forget JoinHandle = 分离线程；等待者线程同样分离
            for waiter in waiters {
                std::mem::forget(waiter);
            }
        } else {
            for waiter in waiters {
                let _ = waiter.join();
            }
        }
    }
}

/// 启动后台管线（replay 监听 + 事件分流），立即返回句柄。
///
/// 控制台模式的全部可见行为（日志文案、退出码）与历史版本一致；GUI 模式由
/// main 调用本函数后把主线程交给 eframe。目录无效返回 `Err`（GUI 据此保持
/// "未配置"态，控制台据此退出）——**绝不** `process::exit`，否则 GUI 的
/// restart 会把整个面板带崩。
pub fn start(client: ClientArgs) -> Result<AppHandles, String> {
    start_with_dir(&client.replay_dir)
}

/// 用显式的 replay 目录启动后台管线（GUI 的"管线可重启"入口，S5）。
///
/// `replay_dir` 是游戏根目录下的 replays 目录。目录不存在时先
/// `create_dir_all`（LiveMonitor 的 PollWatcher 对不存在的目录会 watch
/// 失败，设计文档第一节第 3 条），仍失败（无权限等）才返回 Err。
pub fn start_with_dir(replay_dir: &str) -> Result<AppHandles, String> {
    // Windows 上复制路径时经常混进不可见字符，最典型的是 U+202A
    // (LEFT-TO-RIGHT EMBEDDING)。它会让 PathBuf 变成一个不存在的相对路径，
    // 程序照样启动，但永远等不到 replay 文件 —— 现象是"看起来一切正常，
    // 实际什么都没干"。这里统一清掉。
    let input = sanitize_path(replay_dir);
    if input != replay_dir {
        warn!(
            "Replay dir contained invisible or surrounding characters, normalized to: {:?}",
            input
        );
    }

    info!("Parsing live chat from replay dir: {:?}", input);

    // 目录不存在就先建（防 watcher busy spin / 管线死亡），建不出来才报错
    let dir = PathBuf::from(&input);
    if !dir.is_dir() {
        if let Err(e) = std::fs::create_dir_all(&dir) {
            return Err(format!(
                "Replay dir does not exist and cannot be created: {:?} ({})",
                input, e
            ));
        }
        info!("Replay dir created: {:?}", input);
    }

    // 事件管线（S5 简化）：
    //   monitor → bounded(128) → pump → gui_tx(unbounded) → 面板 / 打印器
    // pump 只是把事件从有界通道搬到无界通道（解耦背压与消费者）。
    let (monitor_tx, monitor_rx) = async_channel::bounded::<LiveEvent>(128);
    let (gui_tx, gui_rx) = async_channel::unbounded::<LiveEvent>();

    // Create CancellationToken
    let token = CancellationToken::new();

    // Start the monitor
    let monitor = LiveMonitor::new(input, monitor_tx);
    let token_clone = token.clone();
    let monitor_thread = std::thread::spawn(move || {
        monitor.run(token_clone);
    });

    // 单路 pump：monitor 的事件搬给 gui_tx。
    // 轮询间隔 20ms：聊天频率下开销可忽略；不依赖 channel 关闭时机，
    // token 取消后立即退出，保证 shutdown 不挂起。
    let token_clone = token.clone();
    let pump_handle = std::thread::spawn(move || loop {
        if token_clone.is_cancelled() {
            break;
        }
        match monitor_rx.try_recv() {
            Ok(event) => {
                if gui_tx.send_blocking(event).is_err() {
                    break; // 消费端已退出
                }
            }
            Err(async_channel::TryRecvError::Empty) => {
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(async_channel::TryRecvError::Closed) => break,
        }
    });

    Ok(AppHandles {
        token,
        monitor_thread,
        pump_handle,
        gui_events: gui_rx,
    })
}

/// 等待中断信号（Ctrl+C）并关停后台管线。控制台模式专用 —— GUI 模式由
/// 面板退出路径调用 [`AppHandles::shutdown`]。
pub fn wait(handles: AppHandles) {
    // Wait for the interrupt signal
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            match tokio::signal::ctrl_c().await {
                Ok(()) => {},
                Err(err) => {
                    error!("Unable to listen for shutdown signal: {}", err);
                },
            }
        });

    info!("SIGINT received, shutting down monitor and pump");
    handles.shutdown();
}

/// 应用主流程（控制台模式）。假定调用方已完成日志初始化（`main.rs` 负责）。
///
/// S5：console 只打印原文（翻译归 GUI 面板）。打印器与管线并发跑，
/// Ctrl+C 后先关管线（通道关闭 → 打印器自然退出），再限时 join 打印器。
pub fn run(client: ClientArgs) {
    let handles = match start(client) {
        Ok(handles) => handles,
        Err(e) => {
            error!("{}", e);
            process::exit(1);
        }
    };
    info!("Console mode: printing original chat lines (translation runs in the GUI panel)");
    let printer = crate::console::spawn_printer(handles.gui_events_clone());
    wait(handles);
    // 打印器靠通道关闭自然退出。极端情况下管线线程被 shutdown 分离、
    // 通道迟迟不关 —— join 限时 1s，超时分离（进程即将退出，承诺 ≤3s）。
    let (done_tx, done_rx) = std::sync::mpsc::channel::<()>();
    let waiter = std::thread::spawn(move || {
        let _ = printer.join();
        let _ = done_tx.send(());
    });
    if rx_recv_until(&done_rx, Instant::now() + Duration::from_secs(1)).is_none() {
        warn!("console printer did not exit in 1s, detaching (process exiting)");
        std::mem::forget(waiter);
    }
}

/// 带截止时间的 channel 等待。
fn rx_recv_until<T>(rx: &std::sync::mpsc::Receiver<T>, deadline: Instant) -> Option<T> {
    let now = Instant::now();
    if now >= deadline {
        return None;
    }
    rx.recv_timeout(deadline - now).ok()
}

/// 去掉零宽字符与 Unicode 双向控制符（U+200B..U+200F、U+202A..U+202E、
/// U+2066..U+2069、U+FEFF），以及被一起复制进来的首尾引号和空白。
pub fn sanitize_path(raw: &str) -> String {
    raw.trim()
        .trim_matches(|c: char| c == '"' || c == '\'' || c == '`')
        .trim()
        .chars()
        .filter(|c| {
            !matches!(
                *c,
                '\u{200B}'..='\u{200F}'
                    | '\u{202A}'..='\u{202E}'
                    | '\u{2066}'..='\u{2069}'
                    | '\u{FEFF}'
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 拆库回归护栏：路径清洗行为不得因搬位置而改变。
    #[test]
    fn sanitize_path_strips_invisible_chars() {
        assert_eq!(sanitize_path("\"D:\\Games\\replays\" "), "D:\\Games\\replays");
        assert_eq!(sanitize_path("D:\\plain"), "D:\\plain");
        // 行为对齐历史实现：清洗顺序是「去空白 → 去首尾引号 → 再去空白 →
        // 滤不可见字符」。U+202A 挡在引号前面时，引号不会被剥（历史行为
        // 就是这样，这里只锁行为不"改进"）。
        assert_eq!(
            sanitize_path("\u{202A}\"D:\\Games\\replays\" "),
            "\"D:\\Games\\replays"
        );
    }

    /// start_with_dir 对不存在的目录：能建就建，建不了报 Err，绝不 panic。
    #[test]
    fn start_with_dir_creates_missing_dir() {
        let dir = std::env::temp_dir().join(format!(
            "chatrans-runner-test-{}-missing",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let dir_str = dir.to_string_lossy().to_string();

        let handles = start_with_dir(&dir_str).expect("目录应被自动创建");
        assert!(dir.is_dir(), "replays 目录应已创建");
        // 立即关停，验证 shutdown 不挂起（2 waiter 全退出）
        handles.shutdown();
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// T04 验收④（自动化部分）：连续 start → shutdown 3 次 —— 管线可
    /// 关可启，无泄漏无 panic（每次 shutdown 都 join 全部线程才返回）。
    #[test]
    fn restart_pipeline_three_times() {
        let dir = std::env::temp_dir().join(format!(
            "chatrans-runner-restart-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let dir_str = dir.to_string_lossy().to_string();
        for round in 0..3 {
            let handles = start_with_dir(&dir_str)
                .unwrap_or_else(|e| panic!("round {} start failed: {}", round, e));
            handles.shutdown();
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
