use std::{
    fs::{remove_file, File},
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};
use anyhow::{Result, anyhow};
use notify::{
    event::{CreateKind, EventKind},
    Config,
    PollWatcher,
    RecursiveMode,
    Watcher,
};
use async_channel::Sender;
use tokio::time::{Duration, sleep};
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use crate::processor::{BattleInfo, ChatLoggerBuilder, LiveEvent, MyName};

use replay_parser::{
    parse_scripts,
    packet2::Parser,
};

/// 从 `tempArenaInfo.json` 里读对局信息。
///
/// 只用来给网页发一条"新对局"提示，所以任何字段缺失或解析失败都不该影响
/// 主流程：读不到就返回空的 `BattleInfo`，服务器那边会退化成一句通用文案。
fn read_battle_info(path: &Path) -> BattleInfo {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) => {
            debug!("Cannot read {}: {}", path.display(), e);
            return BattleInfo::default();
        }
    };
    let value: serde_json::Value = match serde_json::from_str(&text) {
        Ok(value) => value,
        Err(e) => {
            debug!("Cannot parse {}: {}", path.display(), e);
            return BattleInfo::default();
        }
    };
    let field = |key: &str| {
        value
            .get(key)
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string()
    };
    BattleInfo {
        map: field("mapDisplayName"),
        vehicle: field("playerVehicle"),
        player: field("playerName"),
        started_at: field("dateTime"),
    }
}

pub struct LiveMonitor {
    replay_dir: PathBuf,
    tx: Sender<LiveEvent>,
    /// 本机玩家名，跨局复用同一个句柄，给 ChatLogger 判断友/敌用
    my_name: MyName,
}

impl LiveMonitor {
    pub fn new(replay_dir: String, tx: Sender<LiveEvent>) -> LiveMonitor {
        LiveMonitor {
            replay_dir: PathBuf::from(replay_dir),
            tx,
            my_name: Arc::new(Mutex::new(None)),
        }
    }

    async fn parse_live_chat(&self, token: &CancellationToken) -> Result<()> {
        let temp_replay = self.replay_dir.join("temp.korablireplay");
        let info_json = self.replay_dir.join("tempArenaInfo.json");
        info!("Parsing live chat from temp replay: {:?}", temp_replay);
        info!("Parsing live chat from json: {:?}", info_json);

        // Check if the file exists
        if !(temp_replay.exists() && info_json.exists()) {
            return Err(anyhow!("Temp Replay file not found"));
        }

        // 新的一局真的开始了，通知客户端（网页会据此弹提示）。
        // 放在这个位置能同时覆盖两条进入路径：watcher 检测到文件创建，
        // 以及程序启动时对局已经在进行中。
        let info = read_battle_info(&info_json);
        // 记下本机玩家名：判断"哪一队是我方"要靠它去玩家列表里认出自己
        if !info.player.is_empty() {
            *self.my_name.lock().unwrap() = Some(info.player.clone());
        }
        let _ = self.tx.send(LiveEvent::BattleStart(info)).await;

        // Get datafiles and specs with the version
        let datafiles = replay_parser::version::Datafiles::new(
            PathBuf::from("scripts"),
        )?;
        let specs = parse_scripts(&datafiles)?;

        debug!("Specs loaded: {}", specs.iter().map(|s| s.name.as_str()).collect::<Vec<&str>>().join(", "));

        // Assign processor and parser
        let chatlogger = ChatLoggerBuilder::new();
        let processor = chatlogger.build(self.tx.clone(), Arc::clone(&self.my_name));
        let mut analyzer_set = replay_parser::analyzer::AnalyzerAdapter::new(vec![processor]);
        let mut p = Parser::new(&specs);

        // Monitor the change to the temp.korablireplay file
        // Open the file
        let mut file = File::open(&temp_replay)?;
        // Create a buffer using Vec<u8>
        let mut buffer: Vec<u8> = Vec::new();
        // Current file offset
        let mut offset = 0;
        loop {
            // 取消检查（QA Bug#2）：select! 只能在 await 点响应取消，而
            // parse_buffer 是纯 CPU 段 —— 一批积压字节可能解析很久。
            // 每轮循环开头先看一眼 token，让退出等待 ≤ 一轮（2s）。
            if token.is_cancelled() {
                debug!("Live parse loop cancelled");
                return Ok(());
            }

            // Determine whether to continue
            if !info_json.exists() {
                info!("tempArenaInfo.json not found, waiting for the next game");
                break;
            }

            // Read the file from the current offset to the end 
            // 如果这一局结束后 tempArenaInfo.json 没有被删除，程序会带着上一局
            // 的 offset 直接进入下一局。而游戏重开对局时会把 temp.korablireplay
            // 从很小的大小重新写起，此时 seek(旧 offset) 已经越过 EOF，read 只会
            // 拿到 0 字节 —— 结果是整局都安静地解析不出任何聊天。文件变短就重置。
            let file_len = file.metadata()?.len();
            if file_len < offset {
                debug!(
                    "Temp replay was recreated or truncated ({} -> {}), resetting offset",
                    offset, file_len
                );
                offset = 0;
                buffer.clear();
            }

            file.seek(SeekFrom::Start(offset as u64))?;
            debug!("Offset: {:?}, File size: {:?}", offset, file.metadata()?.len());
            offset += file.read_to_end(&mut buffer)? as u64;

            // Parse the packets
            let parsed_bytes = p.parse_buffer(&buffer, &mut analyzer_set)? as usize;
            debug!("Parsed bytes number: {:?}", parsed_bytes);
            buffer.drain(0..parsed_bytes);

            // Sleep for 2 seconds
            sleep(Duration::from_secs(2)).await;
        }

        info!("Parsing live chat from temp replay done");

        Ok(())
    }

    async fn monitor(&self, token: &CancellationToken) -> Result<()> {
        let info_json = self.replay_dir.join("tempArenaInfo.json");
        info!("Parsing live chat from json: {:?}", info_json);

        loop {
            // 取消检查：watcher 等待段有 await 点，select! 能响应；
            // 这里加一道显式检查让"等文件"循环也能立即退出。
            if token.is_cancelled() {
                debug!("Live monitor loop cancelled");
                return Ok(());
            }

            // Check if the file has been created
            if !info_json.exists() {
                // Create a watcher
                // FIXME: Not work for filesystem in WSL
                let (watcher_tx, watcher_rx) = async_channel::bounded(1);
                let mut watcher = PollWatcher::new(
                    move |res| {
                        match watcher_tx.send_blocking(res) {
                            Ok(_) => {}
                            Err(e) => {
                                warn!("Error sending watcher event: {:?}", e);
                            }
                        }
                    },
                    Config::default()
                        .with_compare_contents(true)
                        .with_poll_interval(Duration::from_secs(1))
                )?;

                // Watch the file
                // 目录不存在（被手动删/网盘抖动）时 watch 会 Err：绝不能让
                // `?` 把整个 monitor 循环打死或空转 —— 睡 2s 等目录恢复
                // （S5 设计第七节风险 3，live.rs 唯一改动）。
                if let Err(e) = watcher.watch(
                    &self.replay_dir,
                    RecursiveMode::NonRecursive,
                ) {
                    warn!("watcher.watch failed ({}), retrying in 2s: {:?}", e, self.replay_dir);
                    drop(watcher);
                    sleep(Duration::from_secs(2)).await;
                    continue;
                }

                loop {
                    match watcher_rx.recv().await {
                        Ok(Ok(event)) => {
                            debug!("File event: {:?}", event);
                            // Check if create on the file
                            if event.kind == EventKind::Create(CreateKind::Any) && event.paths[0] == info_json {
                                info!("Entering the game");
                                break;
                            }
                        }
                        _ => {
                            debug!("Watcher error");
                        }
                    }
                }
            }

            // Parse live chat
            match self.parse_live_chat(token).await {
                Ok(_) => {
                    info!("The game has ended, waiting for the next game");
                }
                Err(e) => {
                    info!("Error parsing live chat: {:?}", e);
                }
            }
        }
    }

    fn clean(&self) -> Result<()> {
        // Remove all files ending with `.temp`
        let files = self.replay_dir.read_dir()?;
        for file in files {
            let file = file?;
            let path = file.path();
            if let Some(ext) = path.extension() {
                if ext == "temp" {
                    debug!("Removing temp file: {:?}", path);
                    remove_file(path)?;
                }
            }
        }
        Ok(())
    }

    pub fn run(&self, token: CancellationToken) {
        // Start the websocket server
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        runtime.block_on(async move {
            tokio::select! {
                // Using cloned token to listen to cancellation requests
                _ = token.cancelled() => {}
                _ = self.monitor(&token) => {}
            }
        });

        // Clean up the temp files
        match self.clean() {
            Ok(_) => {
                info!("Temp files are cleaned");
            }
            Err(e) => {
                warn!("Error cleaning temp files: {:?}", e);
            }
        }

        info!("Live monitor is stopped");
    }
}
