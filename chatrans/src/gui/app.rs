//! eframe App：翻译悬浮窗面板。
//!
//! 结构：深色圆角面板 + 常驻顶栏（标题 + 5 个图标）+ 消息区/设置视图 + 底部状态行。
//! 聊天事件从内部通道（runner 的分流）直接消费，**不落盘不过网**；
//! 翻译经后台 tokio worker 线程走 [`TranslateCoordinator`]（并发 2 + 缓存 +
//! 失败静默），egui 主线程零阻塞。
//!
//! 交互语义逐条对齐雷达 `OverlayWindow.xaml.cs`（已验收实现）：
//!   · 默认锁定 = 点击穿透；顶栏 32px 悬停 150ms 才临时接管（只为了点图标）；
//!   · 锁定 = 位置与尺寸全锁死；接管期间同样不许拖；
//!   · 未锁定：4px 阈值拖动、6px 边缘缩放、对边钉住；
//!   · 位置持久化只在用户真改动时写盘，启动期自愈绕开锁定检查。
//! S5 新增（设计裁决 6）：设置视图打开期间强制接管整窗（每帧
//! `set_interactive(true)` 并跳过悬停接管判定），不改 `locked` 记账位，
//! 关闭后原有锁定语义立即恢复 —— `sync_exstyle` 逻辑无需改动。

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use eframe::egui::{self, Align, Color32, CursorIcon, Layout, PointerButton, Pos2, Rect, RichText, ScrollArea, Stroke, Vec2, ViewportCommand};
use tracing::{debug, error, info, warn};

use crate::app_config::AppConfig;
use crate::processor::LiveEvent;
use crate::runner::AppHandles;
use crate::translate::client;
use crate::translate::coordinator::TranslateCoordinator;
use crate::translate::store::TranslationStore;

use super::fonts;
use super::interaction::{
    self, win32, DragState, EdgeFlags, Placement, ResizeState, WorkArea,
    DEFAULT_PANEL_HEIGHT, DEFAULT_PANEL_WIDTH, MAX_PANEL_HEIGHT, MAX_PANEL_WIDTH,
    MIN_PANEL_HEIGHT, MIN_PANEL_WIDTH, TITLE_BAR_HOT_ZONE_HEIGHT,
};
use super::panel_state::{PanelState, Team};
use super::settings::{SettingsAction, SettingsSnapshot, SettingsView};
use super::theme;

/// 窗口标题（也是 FindWindowW 的定位依据，别改）。
pub const WINDOW_TITLE: &str = "Chatrans 翻译";

// ── 配色（S8 起统一到 gui::theme：纯黑底 + 纯白正文）────────────────────
//
// 这里保留旧名字的 re-export，settings.rs 仍按原路径取色，不再各自定义一份。

pub(crate) use super::theme::{
    panel_bg, panel_stroke, COLOR_ALLY, COLOR_ENEMY, COLOR_LOCKED, ICON_DISABLED, ICON_ENABLED,
    TEXT_MAIN, TEXT_ORIGINAL, TEXT_STATUS, TEXT_WARN,
};

// MDL2 / Fluent Icons 字形
const GLYPH_LOCK: char = '\u{E72E}';
const GLYPH_UNLOCK: char = '\u{E785}';
const GLYPH_CLEAR: char = '\u{E74D}';
const GLYPH_TOPMOST: char = '\u{E718}';
const GLYPH_CLOSE: char = '\u{E711}';
/// 设置齿轮（Segoe MDL2 Settings）。
const GLYPH_SETTINGS: char = '\u{E713}';

/// UI → worker 的请求（S5：翻译 + 设置视图的模型列表拉取）。
pub enum SettingsReq {
    Translate(String),
    FetchModels {
        endpoint: crate::translate::store::TranslationEndpoint,
        key: String,
    },
}

/// worker 回给 UI 线程的消息（S5：从 struct 改 enum）。
pub enum WorkerMsg {
    Translation {
        text: String,
        translated: Option<String>,
    },
    Models {
        endpoint_id: String,
        models: Vec<String>,
        error: Option<String>,
    },
}

/// 共享 store 的统一加锁姿势（coordinator 同款）：锁中毒时取回内部数据
/// 继续跑 —— 配置内容没有会被单次 panic 破坏的不变量，没必要连坐。
fn lock_or_recover(mutex: &Mutex<TranslationStore>) -> MutexGuard<'_, TranslationStore> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// GUI 模式入口（双击启动，S5 T04）：
/// 加载配置 → （首次运行）自动发现游戏路径 → 管线按需起 → eframe 占主线程。
/// 任何一步失败都降级（默认值/不起管线/进设置视图），**绝不 panic、绝不 exit**。
pub fn run_app() {
    run_app_inner(None);
}

/// 带参启动（`chatrans -r <dir>`，gui feature）：replay 目录以参数为准，
/// 其余行为与 [`run_app`] 一致。
pub fn run_app_with_args(client: crate::runner::ClientArgs) {
    run_app_inner(Some(client));
}

fn run_app_inner(client: Option<crate::runner::ClientArgs>) {
    let store: Arc<Mutex<TranslationStore>> = Arc::new(Mutex::new(TranslationStore::load()));
    let coordinator = Arc::new(TranslateCoordinator::new());

    // 1) 配置：store（translation.json + secrets.dat）与 app.json 都失败 →
    //    默认值继续跑（不 panic）。首次运行 = app.json 不存在。
    let mut app_config = AppConfig::load();
    let first_run = !AppConfig::default_path().exists();

    // 2) 路径未配置/无效：自动发现兜底（registry → scan）。发现成功**即
    //    自动写回 app.json**（不止首次运行；用户手动改坏路径后重启也能
    //    被兜底救回），source=scan 供诊断。
    if app_config.valid_game_dir().is_none() {
        if let Some(found) = crate::game_discovery::discover() {
            info!("Game dir auto-discovered: {:?}", found.game_dir);
            crate::game_discovery::apply_to_config(&mut app_config, &found);
            let _ = app_config.save();
        }
    }

    // UI → worker 的请求；worker → UI 的结果
    let (req_tx, req_rx) = async_channel::unbounded::<SettingsReq>();
    let (result_tx, result_rx) = std::sync::mpsc::channel::<WorkerMsg>();
    // 供应商连接状态（worker 写、状态行读）：「连不上：<原因>」态的数据源
    let endpoint_error: Arc<std::sync::Mutex<Option<String>>> =
        Arc::new(std::sync::Mutex::new(None));
    let worker = spawn_worker(
        store.clone(),
        coordinator.clone(),
        req_rx,
        result_tx,
        endpoint_error.clone(),
    );
    let _ = worker; // 随进程退出；req_tx 被 app 持有，退出时通道关闭、worker 自行结束

    // 3) 管线启动决策：`-r` 参数优先；否则按 app_config 推导 replays 目录。
    //    无效/未配置 → 不起管线（handles=None），面板进"未配置"态。
    let requested_dir: Option<String> = match &client {
        Some(c) => Some(c.replay_dir.clone()),
        None => app_config
            .valid_replays_dir()
            .map(|p| p.to_string_lossy().to_string()),
    };
    let handles = match requested_dir {
        Some(dir) => match crate::runner::start_with_dir(&dir) {
            Ok(h) => Some(h),
            Err(e) => {
                warn!("background pipeline not started: {}", e);
                None
            }
        },
        None => None,
    };

    let gui_events = handles.as_ref().map(|h| h.gui_events_clone());
    let demo = std::env::var("CHATRANS_GUI_DEMO").as_deref() == Ok("1");
    // 调试/QA 注入：启动即解锁，绕开"点击锁图标"环节，自动化测试用
    let start_unlocked = std::env::var("CHATRANS_GUI_UNLOCKED").as_deref() == Ok("1");
    let app = PanelApp::new(
        handles,
        gui_events,
        store.clone(),
        coordinator.clone(),
        req_tx,
        result_rx,
        endpoint_error,
        app_config,
        first_run,
        demo,
        start_unlocked,
    );

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(WINDOW_TITLE)
            .with_inner_size([DEFAULT_PANEL_WIDTH, DEFAULT_PANEL_HEIGHT])
            .with_min_inner_size([MIN_PANEL_WIDTH, MIN_PANEL_HEIGHT])
            .with_max_inner_size([MAX_PANEL_WIDTH, MAX_PANEL_HEIGHT])
            .with_transparent(true)
            .with_always_on_top()
            .with_decorations(false)
            .with_taskbar(false),
        renderer: eframe::Renderer::Glow,
        ..Default::default()
    };

    info!(
        "Starting translation panel (demo={}, first_run={})",
        demo, first_run
    );
    let result = eframe::run_native(
        WINDOW_TITLE,
        options,
        Box::new(move |cc| {
            fonts::install(&cc.egui_ctx);
            // 纯黑面板：一次性装好 Visuals（明暗两套同色，不跟随系统主题）
            theme::apply(&cc.egui_ctx);
            Ok(Box::new(app))
        }),
    );

    // run_native 返回 = 面板已退出。后台管线在 App::on_exit 里已经关停。
    if let Err(e) = result {
        error!("eframe run failed: {}", e);
        std::process::exit(1);
    }
}

/// 翻译 worker：独立线程 + 独立 tokio current_thread runtime（eframe 占着主
/// 线程，方案 C 约定 runtime 挪后台）。每个请求 spawn 一个 tokio task，
/// 并发上限与缓存由 [`TranslateCoordinator`] 统一管。
///
/// `error_sink` 是供应商连接状态的写入口：真实请求失败 → Some(人话原因摘要)，
/// 成功 → None。状态行据此显示「连不上：<原因>」态（QA Bug#4）。
///
/// S5：store 换成 `Arc<Mutex<..>>`，每个请求开始时短暂加锁读取配置与密钥
/// （编辑即时生效；单局聊天频率下开销可忽略），绝不跨 .await 持锁。
fn spawn_worker(
    store: Arc<Mutex<TranslationStore>>,
    coordinator: Arc<TranslateCoordinator>,
    req_rx: async_channel::Receiver<SettingsReq>,
    result_tx: std::sync::mpsc::Sender<WorkerMsg>,
    error_sink: Arc<std::sync::Mutex<Option<String>>>,
) -> std::thread::JoinHandle<()> {
    std::thread::Builder::new()
        .name("translate-worker".into())
        .spawn(move || {
            let rt = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(rt) => rt,
                Err(e) => {
                    error!("translate worker: tokio runtime build failed: {}", e);
                    return;
                }
            };
            rt.block_on(async move {
                let (done_tx, done_rx) = async_channel::unbounded::<WorkerMsg>();
                loop {
                    tokio::select! {
                        req = req_rx.recv() => match req {
                            Ok(SettingsReq::Translate(text)) => {
                                let store = store.clone();
                                let coordinator = coordinator.clone();
                                let done_tx = done_tx.clone();
                                let error_sink = error_sink.clone();
                                tokio::spawn(async move {
                                    // 短暂加锁：读配置快照 + 密钥，立刻放锁
                                    let (enabled, active, target, key) = {
                                        let store = lock_or_recover(&store);
                                        let config = store.config();
                                        (
                                            config.enabled,
                                            config.active().cloned(),
                                            config.target_language.clone(),
                                            config
                                                .active()
                                                .and_then(|ep| store.get_secret(&ep.secret_key()).map(str::to_string)),
                                        )
                                    };
                                    let outcome = match active {
                                        Some(endpoint) if enabled => {
                                            match key {
                                                Some(key) => {
                                                    let log_context = format!(
                                                        "ep={} model={} ",
                                                        endpoint.id, endpoint.model
                                                    );
                                                    let text_for_closure = text.clone();
                                                    coordinator
                                                        .translate_with(&text, &log_context, move || {
                                                            let text = text_for_closure;
                                                            let endpoint = endpoint.clone();
                                                            let error_sink = error_sink.clone();
                                                            async move {
                                                                let result = client::translate(
                                                                    &endpoint,
                                                                    Some(&key),
                                                                    &text,
                                                                    &target,
                                                                )
                                                                .await;
                                                                if result.ok {
                                                                    if let Ok(mut slot) =
                                                                        error_sink.lock()
                                                                    {
                                                                        *slot = None;
                                                                    }
                                                                    result.text
                                                                } else {
                                                                    let status = result
                                                                        .status_code
                                                                        .map(|s| format!("{} ", s))
                                                                        .unwrap_or_default();
                                                                    let error = result.error.unwrap_or_else(
                                                                        || "<no error text>".to_string(),
                                                                    );
                                                                    if let Ok(mut slot) =
                                                                        error_sink.lock()
                                                                    {
                                                                        *slot = Some(short_reason(
                                                                            &format!("{}{}", status, error),
                                                                        ));
                                                                    }
                                                                    warn!(
                                                                        "[翻译] fail reason ep={} {}{}",
                                                                        endpoint.id,
                                                                        status,
                                                                        error.trim()
                                                                    );
                                                                    None
                                                                }
                                                            }
                                                        })
                                                        .await
                                                }
                                                None => {
                                                    warn!("[翻译] skip (no key) ep={}", endpoint.id);
                                                    None
                                                }
                                            }
                                        }
                                        Some(_) => {
                                            warn!("[翻译] skip (总开关关闭)");
                                            None
                                        }
                                        None => {
                                            warn!("[翻译] skip (没有供应商)");
                                            None
                                        }
                                    };
                                    let _ = done_tx
                                        .send(WorkerMsg::Translation { text, translated: outcome })
                                        .await;
                                });
                            }
                            Ok(SettingsReq::FetchModels { endpoint, key }) => {
                                let done_tx = done_tx.clone();
                                tokio::spawn(async move {
                                    let endpoint_id = endpoint.id.clone();
                                    let (models, error) = match client::fetch_models(&endpoint, Some(&key)).await {
                                        Ok(models) => (models, None),
                                        Err(e) => {
                                            warn!("[设置] fetch_models failed ep={} {}", endpoint_id, e);
                                            (Vec::new(), Some(e))
                                        }
                                    };
                                    let _ = done_tx
                                        .send(WorkerMsg::Models { endpoint_id, models, error })
                                        .await;
                                });
                            }
                            Err(_) => break, // 通道关闭：UI 已退出
                        },
                        done = done_rx.recv() => match done {
                            Ok(msg) => {
                                let _ = result_tx.send(msg);
                            }
                            Err(_) => break,
                        },
                    }
                }
            });
        })
        .expect("spawn translate worker")
}

/// 把失败原因压成状态行能放下的单行摘要（只含 HTTP 状态与错误文本，
/// 不含 apiKey 与聊天正文）。
fn short_reason(raw: &str) -> String {
    let collapsed: String = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut text: String = collapsed.chars().take(48).collect();
    if collapsed.chars().count() > 48 {
        text.push('…');
    }
    text
}

pub struct PanelApp {
    /// 后台管线句柄。`None` = 未配置/未运行（T04 起"未配置"是合法态，
    /// 保存有效路径后 start）。`on_exit`（`&mut self`）里 take 出来消费。
    handles: Option<AppHandles>,
    store: Arc<Mutex<TranslationStore>>,
    coordinator: Arc<TranslateCoordinator>,
    /// 管线事件出口。与 handles 同生命周期：None = 无管线可听。
    event_rx: Option<async_channel::Receiver<LiveEvent>>,
    req_tx: async_channel::Sender<SettingsReq>,
    result_rx: std::sync::mpsc::Receiver<WorkerMsg>,

    /// 游戏路径配置（app.json 的内存镜像；设置保存时写回）。
    app_config: AppConfig,
    /// rfd 目录选择 / 自动发现的后台结果（None = 无在途任务）。
    dir_pick_rx: Option<std::sync::mpsc::Receiver<Option<String>>>,

    state: PanelState,
    /// 设置视图（视图切换 + 本地编辑态）。
    settings: SettingsView,
    /// 已发给 worker、还没等到结果的原文（去重，避免同一原文反复排队）。
    pending: HashSet<String>,

    // 窗口几何（DIP）。window_* 是我们自己的记账：改动只经我们发出的命令，
    // 每帧从 OS 读回既贵又抖。
    window_pos: Pos2,
    window_size: Vec2,
    placement_path: PathBuf,

    locked: bool,
    /// 面板不透明度（百分比，见 theme::OPACITY_*）。只影响背景 alpha，
    /// 文字/图标始终不透明。
    opacity: u8,
    /// 仅显示译文：勾选后每条聊天只画译文，不再画原文小字。
    only_translation: bool,
    /// 启动时 `CHATRANS_GUI_UNLOCKED=1` 注入：自动化测试用，优先级高于落盘状态。
    lock_env_override: bool,
    topmost: bool,
    interactive: bool,
    inside_since: Option<Instant>,
    /// 上一帧光标的屏幕坐标（判断"手在动还是在停"，防镜头扫过误触）。
    hover_prev: Option<(i32, i32)>,
    drag: DragState,
    resize: ResizeState,
    hwnd: isize,

    topbar_rect: Rect,
    button_zone: Rect,
    initialized: bool,
    /// 首帧初始化完成时刻：布局命令落地前（1s 内）不做 OS 矩形校准。
    initialized_at: Option<Instant>,

    /// 最近一次供应商请求失败的人话原因（None = 正常/从未失败）。
    /// worker 写、UI 读：状态行「连不上：<原因>」态（设计文档第 3 节第 6 条）。
    endpoint_error: Arc<std::sync::Mutex<Option<String>>>,

    demo: bool,
    demo_next: Instant,
    demo_idx: usize,
}

impl PanelApp {
    #[allow(clippy::too_many_arguments)]
    fn new(
        handles: Option<AppHandles>,
        event_rx: Option<async_channel::Receiver<LiveEvent>>,
        store: Arc<Mutex<TranslationStore>>,
        coordinator: Arc<TranslateCoordinator>,
        req_tx: async_channel::Sender<SettingsReq>,
        result_rx: std::sync::mpsc::Receiver<WorkerMsg>,
        endpoint_error: Arc<std::sync::Mutex<Option<String>>>,
        app_config: AppConfig,
        first_run: bool,
        demo: bool,
        start_unlocked: bool,
    ) -> PanelApp {
        let mut app = PanelApp {
            handles,
            store,
            coordinator,
            event_rx,
            req_tx,
            result_rx,
            app_config,
            dir_pick_rx: None,
            state: PanelState::new(),
            settings: SettingsView::new(),
            pending: HashSet::new(),
            window_pos: Pos2::new(100.0, 100.0),
            window_size: Vec2::new(DEFAULT_PANEL_WIDTH, DEFAULT_PANEL_HEIGHT),
            placement_path: TranslationStore::default_directory().join("placement.json"),
            // 默认锁定（与雷达一致）；真实状态在 first_frame_init 里按
            // `placement.json` 的 locked 字段恢复（S8 记忆功能）
            locked: !start_unlocked,
            opacity: theme::OPACITY_DEFAULT,
            only_translation: false,
            lock_env_override: start_unlocked,
            // 解锁启动时保持窗口可交互（锁定语义：未锁定 = 永远可交互）
            interactive: start_unlocked,
            topmost: true,
            inside_since: None,
            hover_prev: None,
            drag: DragState::default(),
            resize: ResizeState::default(),
            hwnd: 0,
            topbar_rect: Rect::NOTHING,
            button_zone: Rect::NOTHING,
            initialized: false,
            initialized_at: None,
            endpoint_error,
            demo,
            demo_next: Instant::now(),
            demo_idx: 0,
        };
        // 首次运行（app.json 不存在）：自动打开设置视图 + 状态行引导文案。
        // `CHATRANS_GUI_SETTINGS=1` 是同一入口的 QA 注入（自动化截图/验证用），
        // 与 DEMO/UNLOCKED 同族，不改变正常启动行为。
        let open_settings = std::env::var("CHATRANS_GUI_SETTINGS").as_deref() == Ok("1");
        if first_run || open_settings {
            let store = lock_or_recover(&app.store);
            app.settings.open(store.config(), &app.app_config, &store);
            app.settings.set_opacity(app.opacity);
            app.settings.set_only_translation(app.only_translation);
        }
        app
    }

    // ── 事件消费 ────────────────────────────────────────────────────────

    fn poll_events(&mut self) {
        let Some(rx) = &self.event_rx else {
            return; // 无管线（未配置/启动失败）：无事可听
        };
        while let Ok(event) = rx.try_recv() {
            match event {
                LiveEvent::Chat(msg) => {
                    self.state.push_chat(
                        &msg.sender,
                        msg.ship,
                        msg.allied,
                        &msg.audience,
                        &msg.message,
                        None,
                    );
                }
                LiveEvent::BattleStart(_info) => {
                    self.state.start_battle();
                }
            }
        }
    }

    /// rfd 目录选择 / 自动发现的后台结果回灌（每帧 poll，绝不阻塞 UI）。
    fn poll_dir_pick(&mut self) {
        let Some(rx) = &self.dir_pick_rx else {
            return;
        };
        match rx.try_recv() {
            Ok(result) => {
                self.dir_pick_rx = None;
                self.settings.set_game_dir(result);
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {}
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.dir_pick_rx = None;
            }
        }
    }

    // ── 管线生命周期（T04）──────────────────────────────────────────────

    /// 启动管线；失败（目录建不出等）降级为"未运行"态，绝不 panic。
    fn start_pipeline(&mut self, replays_dir: &str) {
        match crate::runner::start_with_dir(replays_dir) {
            Ok(h) => {
                self.event_rx = Some(h.gui_events_clone());
                self.handles = Some(h);
                info!("[管线] started with {:?}", replays_dir);
            }
            Err(e) => {
                warn!("[管线] start failed: {}", e);
                self.handles = None;
                self.event_rx = None;
            }
        }
    }

    /// 重启管线：旧管线 2s 限时退干净（AppHandles::shutdown），再起新的。
    fn restart_pipeline(&mut self, replays_dir: &str) {
        if let Some(h) = self.handles.take() {
            h.shutdown();
        }
        self.event_rx = None;
        info!("[管线] restarting with {:?}", replays_dir);
        self.start_pipeline(replays_dir);
    }

    fn poll_worker_results(&mut self) {
        while let Ok(msg) = self.result_rx.try_recv() {
            match msg {
                WorkerMsg::Translation { text, translated } => {
                    self.pending.remove(&text);
                    self.state.set_translated(&text, translated);
                }
                WorkerMsg::Models {
                    endpoint_id,
                    models,
                    error,
                } => {
                    self.settings.on_models_result(&endpoint_id, models, error);
                }
            }
        }
    }

    /// 需要翻译的行：缓存命中直接回填；没在途的发给 worker。
    /// 失败不缓存（coordinator 语义）→ pending 移除后，同原文再出现会再试。
    fn request_translations(&mut self) {
        let team_only = lock_or_recover(&self.store).config().team_only;
        let texts: Vec<String> = self
            .state
            .visible_rows()
            .into_iter()
            .filter(|row| row.needs_translation())
            .filter(|row| !team_only || row.is_team_channel)
            .map(|row| row.text.clone())
            .collect();

        for text in texts {
            if let Some(translated) = self.coordinator.cache_get(&text) {
                self.state.set_translated(&text, Some(translated));
                continue;
            }
            if self.pending.contains(&text) {
                continue;
            }
            if self.req_tx.try_send(SettingsReq::Translate(text.clone())).is_ok() {
                self.pending.insert(text);
            }
        }
    }

    // ── 演示注入（CHATRANS_GUI_DEMO=1）：无对局也能自证渲染/翻译链路 ────

    fn poll_demo(&mut self) {
        if !self.demo || Instant::now() < self.demo_next {
            return;
        }
        self.demo_next = Instant::now() + Duration::from_millis(2500);
        const DEMO_LINES: [(&str, Option<&str>, Option<bool>, &str, &str, Option<&str>); 6] = [
            ("Alice", Some("Minotaur"), Some(true), "battle_team", "gg wp push", None),
            ("Иван", Some("Großer Kurfürst"), Some(false), "battle_team", "нужна поддержка на B", None),
            ("橋本", Some("得梅因"), Some(true), "battle_team", "雷达开在 A 点了", None),
            ("xX_Sniper_Xx", None, Some(false), "battle_all", "noob team lol", None),
            ("Duke", Some("Yamato"), None, "battle_team", "борьба до конца", None),
            ("Carol", Some(" Des Moines "), None, "battle_team", "hello world", Some("你好，世界")),
        ];
        let (name, ship, allied, audience, text, translated) =
            DEMO_LINES[self.demo_idx % DEMO_LINES.len()];
        self.demo_idx += 1;
        if self.demo_idx == 1 {
            // 第一条先模拟开局，验证"开局清空 + 计数归零"
            self.state.start_battle();
        }
        self.state.push_chat(
            name,
            ship.map(str::trim).map(str::to_string),
            allied,
            audience,
            text,
            translated.map(str::to_string),
        );
    }

    // ── 设置视图动作执行 ────────────────────────────────────────────────

    fn handle_settings_actions(&mut self, actions: Vec<SettingsAction>) {
        for action in actions {
            match action {
                SettingsAction::FetchModels { endpoint, key } => {
                    let endpoint_id = endpoint.id.clone();
                    if self
                        .req_tx
                        .try_send(SettingsReq::FetchModels { endpoint, key })
                        .is_ok()
                    {
                        self.settings.mark_fetch_in_flight(&endpoint_id);
                    } else {
                        self.settings.on_models_result(
                            &endpoint_id,
                            Vec::new(),
                            Some("发送请求失败，请重试".to_string()),
                        );
                    }
                }
                SettingsAction::SetOpacity { percent, persist } => {
                    // 实时生效：下一帧 render 就用新的 alpha（不重建窗口、不重启管线）
                    self.opacity = percent.clamp(
                        theme::OPACITY_MIN,
                        theme::OPACITY_MAX,
                    );
                    self.settings.set_opacity(self.opacity);
                    if persist {
                        self.persist_placement();
                    }
                }
                SettingsAction::SetOnlyTranslation { value, persist } => {
                    self.only_translation = value;
                    self.settings.set_only_translation(value);
                    if persist {
                        self.persist_placement();
                    }
                }
                SettingsAction::BrowseGameDir => {
                    // rfd 同步对话框必须后台线程（设计文档第七节风险 4）
                    let (tx, rx) = std::sync::mpsc::channel();
                    crate::game_discovery::spawn_dir_picker(tx);
                    self.dir_pick_rx = Some(rx);
                }
                SettingsAction::DiscoverGameDir => {
                    let (tx, rx) = std::sync::mpsc::channel();
                    crate::game_discovery::spawn_discovery(tx);
                    self.dir_pick_rx = Some(rx);
                }
                SettingsAction::Save(snapshot) => {
                    self.apply_settings_save(snapshot);
                }
            }
        }
    }

    /// 保存三步（设计文档第五节"供应商编辑链"）：
    /// 1. 翻译配置 + 密钥写 store（translation.json 原子写 + DPAPI）；
    /// 2. 游戏路径变化才写 app.json（source=manual）；
    /// 3. 管线决策：路径变化 → restart（旧管线 2s 限时退）；路径无效 → 停；
    ///    管线没跑且路径有效 → 起。翻译配置本身即时生效（worker 每请求重读）。
    fn apply_settings_save(&mut self, snapshot: SettingsSnapshot) {
        let store_result = {
            let mut store = lock_or_recover(&self.store);
            *store.config_mut() = snapshot.config;
            for (key_ref, value) in snapshot.secrets {
                store.set_secret(&key_ref, Some(&value));
            }
            store.save()
        };

        let old_replays = self.app_config.valid_replays_dir();
        let mut path_changed = false;
        let mut config_result: Result<(), String> = Ok(());
        if snapshot.game_dir != self.app_config.game_dir.trim() {
            path_changed = true;
            self.app_config.game_dir = snapshot.game_dir;
            self.app_config.replays_dir = crate::app_config::replays_dir_of(std::path::Path::new(
                &self.app_config.game_dir,
            ))
            .to_string_lossy()
            .to_string();
            self.app_config.source = crate::app_config::SOURCE_MANUAL.to_string();
            config_result = self
                .app_config
                .save()
                .map_err(|e| format!("app.json 写入失败: {}", e));
        }

        match (store_result, config_result) {
            (Ok(()), Ok(())) => {
                let new_replays = self.app_config.valid_replays_dir();
                if self.handles.is_none() {
                    // 管线没在跑：路径有效就起（首次配置完成 / 修复坏路径）
                    if let Some(dir) = &new_replays {
                        let dir = dir.to_string_lossy().to_string();
                        self.start_pipeline(&dir);
                        self.settings
                            .on_saved(true, &format!("配置已保存，开始监听 {}", dir));
                    } else {
                        self.settings
                            .on_saved(false, "配置已保存，但游戏路径无效，未开始监听");
                    }
                } else if path_changed && new_replays != old_replays {
                    match &new_replays {
                        Some(dir) => {
                            let dir = dir.to_string_lossy().to_string();
                            self.restart_pipeline(&dir);
                            self.settings
                                .on_saved(true, &format!("配置已保存，监听路径已切换到 {}", dir));
                        }
                        None => {
                            // 新路径无效：停掉旧管线（旧路径可能已失效）
                            if let Some(h) = self.handles.take() {
                                h.shutdown();
                            }
                            self.event_rx = None;
                            self.settings
                                .on_saved(true, "配置已保存（游戏路径无效，已停止监听）");
                        }
                    }
                } else {
                    self.settings.on_saved(true, "配置已保存");
                }
            }
            (Err(e), _) => {
                error!("[设置] 配置保存失败: {}", e);
                self.settings.on_saved(false, &format!("保存失败：{}", e));
            }
            (_, Err(e)) => {
                error!("[设置] {}", e);
                self.settings.on_saved(false, &format!("保存失败：{}", e));
            }
        }
    }

    // ── 点击穿透（锁定态）───────────────────────────────────────────────

    fn set_interactive(&mut self, on: bool) {
        if self.interactive == on {
            return;
        }
        #[cfg(windows)]
        if self.hwnd != 0 {
            win32::apply_click_through(self.hwnd, !on);
        }
        self.interactive = on;
    }

    /// 锁定态：只有顶栏 32px 悬停够 150ms 才接管鼠标；移开立刻恢复穿透。
    /// 未锁定：永远可交互。与雷达 OnHoverTick 逐条对应。
    /// 设置视图打开时**不会被调用**（update 里直接强制接管）。
    fn update_click_through(&mut self, ctx: &egui::Context) {
        #[cfg(not(windows))]
        {
            let _ = ctx;
            self.interactive = true;
        }
        #[cfg(windows)]
        {
            if self.hwnd == 0 {
                return;
            }
            if !self.locked {
                self.inside_since = None;
                self.set_interactive(true);
                return;
            }

            let (Some((cx, cy)), Some((l, t, r, b))) =
                (win32::cursor_pos(), win32::window_rect(self.hwnd))
            else {
                return;
            };
            let inside_window = cx >= l && cx < r && cy >= t && cy < b;
            let hot_zone = (TITLE_BAR_HOT_ZONE_HEIGHT * ctx.pixels_per_point()) as i32;
            let inside_title_bar = inside_window && cy < t + hot_zone;

            // 这一帧光标走了多远（屏幕像素）。第一次见到光标按"刚动过"算。
            let moved = match self.hover_prev {
                Some((px, py)) => {
                    let dx = (cx - px) as f32;
                    let dy = (cy - py) as f32;
                    (dx * dx + dy * dy).sqrt()
                }
                None => f32::MAX,
            };
            self.hover_prev = Some((cx, cy));

            if inside_title_bar {
                if self.interactive {
                    // 已经接管：保持到移出顶栏为止。否则在按钮之间挪几像素
                    // 就会掉回穿透态，点击直接穿到游戏里，反而更容易误操作。
                    return;
                }
                // 尚未接管：**必须停住**才算数。镜头连续移动时每帧位移都远超
                // HOVER_STILL_EPS，计时不断被打断 —— 扫过面板永远不会接管。
                let still = moved <= interaction::HOVER_STILL_EPS;
                if !still || self.inside_since.is_none() {
                    self.inside_since = Some(Instant::now());
                }
                let dwell_ms = self
                    .inside_since
                    .map(|since| since.elapsed().as_millis())
                    .unwrap_or(0);
                if interaction::hover_takeover(moved, dwell_ms) {
                    debug!("[takeover] pointer still {dwell_ms}ms, interactive on");
                    self.set_interactive(true);
                }
            } else {
                // 移出顶栏：立即还穿透给游戏
                if self.interactive {
                    debug!("[takeover] pointer left title bar, interactive off");
                }
                self.inside_since = None;
                self.hover_prev = None;
                self.set_interactive(false);
            }
        }
    }

    /// 每帧整体校对扩展样式（QA Bug#3）：TOOLWINDOW 置位（不进 Alt-Tab）、
    /// APPWINDOW 清位（winit 自己会挂）、LAYERED 置位、TRANSPARENT 按交互态、
    /// NOACTIVATE 按视图态 —— 聊天查看态（锁定/解锁都算）保持置位（不抢
    /// 游戏焦点）；设置视图打开期间清除（用户明确要交互，点击可激活窗口、
    /// 键盘可进输入框，S5 QA 焦点修复），关闭后立即恢复。
    /// **必须独立于 update_click_through 的控制流**：解锁路径在 hover 逻辑里
    /// 提前 return，放在那里会让解锁态永远得不到样式校对（QA 回归实测坐实）。
    fn sync_exstyle(&mut self) {
        #[cfg(windows)]
        if self.hwnd != 0 {
            win32::sync_overlay_exstyle(self.hwnd, !self.interactive, self.settings.is_open());
        }
    }

    // ── 拖动 / 缩放 / 光标 ──────────────────────────────────────────────

    /// 光标的屏幕坐标（DIP）。拖/缩进行中的位移基准必须用它，不能用
    /// 「窗口记账 + egui 局部坐标」拼出来的屏幕坐标：winit 应用
    /// OuterPosition 命令滞后记账一帧，而左/顶缘缩放的位移恰好等于位置
    /// 误差本身 —— 滞后逐帧复利放大（QA Bug#1：单步 -40px 后窗口每帧多长
    /// ~200px 直到撞上限）。GetCursorPos 是即时屏幕坐标，与窗口怎么动、
    /// 记账准不准完全无关，也是设计文档第 3 节雷达原方案的选型原因。
    fn cursor_screen(&self, ctx: &egui::Context) -> Option<Pos2> {
        #[cfg(windows)]
        {
            let (x, y) = win32::cursor_pos()?;
            let scale = ctx.pixels_per_point() as f32;
            Some(Pos2::new(x as f32 / scale, y as f32 / scale))
        }
        #[cfg(not(windows))]
        {
            let _ = ctx;
            None
        }
    }

    /// 手势空闲时从 OS 读回真实窗口矩形校准记账。winit 应用位置/尺寸命令
    /// 有滞后，纯记账久了会漂；持久化与下一次手势的起点必须基于真实矩形。
    /// 手势进行中不校准（那时记账 = 我们的意图值，OS 还在路上）；
    /// 启动后 1s 内也不校准（首帧布局命令尚未落地）。
    fn sync_window_rect_from_os(&mut self, ctx: &egui::Context) {
        #[cfg(windows)]
        if self.hwnd != 0 {
            if let Some((l, t, r, b)) = win32::window_rect(self.hwnd) {
                let scale = ctx.pixels_per_point() as f32;
                self.window_pos = Pos2::new(l as f32 / scale, t as f32 / scale);
                self.window_size = Vec2::new((r - l) as f32 / scale, (b - t) as f32 / scale);
            }
        }
    }

    fn handle_pointer(&mut self, ctx: &egui::Context) {
        let (pressed, down, pointer_pos) = ctx.input(|input| {
            (
                input.pointer.button_pressed(PointerButton::Primary),
                input.pointer.button_down(PointerButton::Primary),
                input.pointer.interact_pos(),
            )
        });

        // 缩放进行中：位移基准 = GetCursorPos（见 cursor_screen 注释）
        if self.resize.armed {
            if !down {
                debug!("[gesture] resize END (pointer up)");
                self.end_resize(ctx);
            } else if let Some(screen) = self.cursor_screen(ctx) {
                if let Some(rect) = self.resize.motion(screen) {
                    debug!("[gesture] resize motion -> {}x{} @ ({},{})", rect.width(), rect.height(), rect.min.x, rect.min.y);
                    self.apply_bounds(ctx, rect);
                }
            }
            return;
        }

        // 拖动进行中：同上，位移基准 = GetCursorPos
        if self.drag.armed {
            if !down {
                self.end_drag(ctx);
            } else if let Some(screen) = self.cursor_screen(ctx) {
                if let Some(new_pos) = self.drag.motion(screen) {
                    self.window_pos = new_pos;
                    ctx.send_viewport_cmd(ViewportCommand::OuterPosition(new_pos));
                }
            }
            return;
        }

        // 空闲：光标形状提示 + 按下开始拖/缩
        if let Some(pos) = pointer_pos {
            // 锁定态：连缩放光标都不显示 —— 暗示"这里能拉"与"已锁定"自相矛盾
            let edges = if self.locked {
                EdgeFlags::NONE
            } else {
                interaction::hit_test_edges(pos, self.window_size)
            };
            ctx.set_cursor_icon(cursor_for(edges));

            if pressed && !self.locked {
                // 命中判定用 egui 局部坐标（窗口几何），位移起点用屏幕坐标
                // （GetCursorPos，理由见 cursor_screen 注释）
                if !edges.is_none() {
                    if let Some(screen) = self.cursor_screen(ctx) {
                        debug!("[gesture] resize BEGIN edges={:?}", edges);
                        self.resize.begin(
                            edges,
                            screen,
                            Rect::from_min_size(self.window_pos, self.window_size),
                        );
                    }
                } else if self.topbar_rect.contains(pos)
                    && !self.button_zone.contains(pos)
                {
                    if let Some(screen) = self.cursor_screen(ctx) {
                        self.drag.begin(screen, self.window_pos);
                    }
                }
            }
        }
    }

    fn apply_bounds(&mut self, ctx: &egui::Context, rect: Rect) {
        self.window_pos = rect.min;
        self.window_size = rect.size();
        ctx.send_viewport_cmd(ViewportCommand::InnerSize(self.window_size));
        ctx.send_viewport_cmd(ViewportCommand::OuterPosition(self.window_pos));
    }

    /// 结束拖动：先夹回工作区再写，保证文件里永远不会留下越界坐标。
    fn end_drag(&mut self, ctx: &egui::Context) {
        if self.drag.end() {
            self.clamp_window_pos(ctx);
            self.persist_placement();
        }
    }

    fn end_resize(&mut self, ctx: &egui::Context) {
        if self.resize.end() {
            self.clamp_window_pos(ctx);
            self.persist_placement();
        }
    }

    fn work_area(&self, ctx: &egui::Context) -> WorkArea {
        let scale = ctx.pixels_per_point() as f32;
        #[cfg(windows)]
        let (l, t, r, b) = win32::work_area_physical();
        #[cfg(not(windows))]
        let (l, t, r, b) = (0i32, 0i32, 1920i32, 1040i32);
        WorkArea {
            left: l as f32 / scale,
            top: t as f32 / scale,
            right: r as f32 / scale,
            bottom: b as f32 / scale,
        }
    }

    fn clamp_window_pos(&mut self, ctx: &egui::Context) {
        let work = self.work_area(ctx);
        let placement = clamp_pos(Placement {
            left: self.window_pos.x as f64,
            top: self.window_pos.y as f64,
            width: self.window_size.x as f64,
            height: self.window_size.y as f64,
            locked: Some(self.locked),
            opacity: Some(self.opacity),
            only_translation: Some(self.only_translation),
        }, &work);
        self.window_pos = Pos2::new(placement.left as f32, placement.top as f32);
        ctx.send_viewport_cmd(ViewportCommand::OuterPosition(self.window_pos));
    }

    /// 用户手势结束 / 锁定状态切换后的落盘（位置 + 锁定一次写入）。
    ///
    /// 拖动缩放走不到这里（锁定态入口已挡）；锁图标切换必须走这里，
    /// 否则"记住上次锁定状态"就只是一句空话。
    fn persist_placement(&mut self) {
        interaction::save_placement(
            &self.placement_path,
            &Placement {
                left: self.window_pos.x as f64,
                top: self.window_pos.y as f64,
                width: self.window_size.x as f64,
                height: self.window_size.y as f64,
                locked: Some(self.locked),
                opacity: Some(self.opacity),
                only_translation: Some(self.only_translation),
            },
        );
    }

    // ── 启动初始化（首帧）──────────────────────────────────────────────

    fn first_frame_init(&mut self, ctx: &egui::Context) {
        self.initialized = true;
        self.initialized_at = Some(Instant::now());

        // 定位自己的窗口（eframe 不直接暴露 winit 句柄，与 spike 同一套办法）
        #[cfg(windows)]
        {
            // 实测：ViewportBuilder 的 title 在无边框窗口上偶尔不生效
            // （GetWindowText 为空），首帧再补一发命令；标题生效需要一帧，
            // 所以 hwnd 的发现放到每帧的 ensure_hwnd 里重试。
            ctx.send_viewport_cmd(ViewportCommand::Title(WINDOW_TITLE.to_string()));
        }

        // 恢复上次位置：读不到/解析失败 → 默认位置；夹取自愈绕开锁定检查
        let work = self.work_area(ctx);
        let (placement, write_back) = interaction::restore_placement(&self.placement_path, &work);
        self.window_pos = Pos2::new(placement.left as f32, placement.top as f32);
        if placement.width > 0.0 {
            self.window_size = Vec2::new(placement.width as f32, placement.height as f32);
        }
        ctx.send_viewport_cmd(ViewportCommand::OuterPosition(self.window_pos));
        ctx.send_viewport_cmd(ViewportCommand::InnerSize(self.window_size));
        // 锁定状态恢复（S8）：落盘值 > 产品默认（锁定）；环境变量注入最高。
        // 必须在 ensure_hwnd 之前算好，首帧打穿透位时才不会用错状态。
        self.locked = interaction::resolve_locked(placement.locked, self.lock_env_override);
        self.interactive = !self.locked;
        // 透明度同理（S8）：越界值夹回合法区间，缺失用默认 100%
        self.opacity = interaction::resolve_opacity(placement.opacity);
        self.only_translation = interaction::resolve_only_translation(placement.only_translation);
        self.settings.set_opacity(self.opacity);
        self.settings.set_only_translation(self.only_translation);
        info!(
            "ui state restored: locked={} (saved={:?}, env_override={}), opacity={}% (saved={:?})",
            self.locked, placement.locked, self.lock_env_override, self.opacity, placement.opacity
        );

        if write_back {
            // 启动期自愈，不是用户手势 → 绕开锁定检查直接写
            interaction::save_placement(
                &self.placement_path,
                &Placement {
                    left: self.window_pos.x as f64,
                    top: self.window_pos.y as f64,
                    width: self.window_size.x as f64,
                    height: self.window_size.y as f64,
                    locked: Some(self.locked),
                    opacity: Some(self.opacity),
                    only_translation: Some(self.only_translation),
                },
            );
        }

        // 默认置顶（builder 已设 always_on_top；记账保持一致）
        self.topmost = true;
    }

    /// 按 pid + 标题定位窗口句柄；标题命令生效前一帧是空的，每帧重试直到拿到。
    fn ensure_hwnd(&mut self) {
        #[cfg(windows)]
        if self.hwnd == 0 {
            self.hwnd = win32::find_own_window(WINDOW_TITLE);
            if self.hwnd != 0 {
                info!("panel hwnd located: {}", self.hwnd);
                // 首次拿到句柄：把 OS 样式同步到逻辑状态。窗口刚创建时没有
                // 穿透位，而 set_interactive 只在状态变化时动作 —— 不在这里
                // 主动打上，锁定态的初始穿透永远不会生效（雷达在
                // SourceInitialized 里 ApplyClickThrough(true) 就是这个用途）。
                win32::apply_click_through(self.hwnd, !self.interactive);
            }
        }
    }

    // ── UI ──────────────────────────────────────────────────────────────

    fn render(&mut self, ctx: &egui::Context) {
        // 面板本体：深色圆角 + 1px 描边，内边距 0，让三个子区贴满
        egui::CentralPanel::default()
            // 背景**不用** Frame 的 fill：Frame 会按内容 min_rect 画矩形，
            // 内容一溢出（贴底的状态栏 + 滚动区），底边就跑到窗口外 ——
            // 表现就是「上面两角圆、下面两角方」。改成先手动画一块固定
            // 尺寸的圆角底（永远等于窗口），再画内容。
            .frame(egui::Frame::NONE)
            .show(ctx, |ui| {
                let panel_rect = ui.max_rect();
                ui.painter().rect(
                    panel_rect,
                    egui::CornerRadius::same(theme::PANEL_CORNER_RADIUS),
                    panel_bg(self.opacity),
                    Stroke::new(1.0_f32, panel_stroke()),
                    egui::StrokeKind::Inside,
                );
                // 内容不许画到背景之外（圆角处留白）
                ui.set_clip_rect(panel_rect);
                self.render_top_bar(ui);
                if self.settings.is_open() {
                    let actions = self.settings.ui(ui);
                    if !actions.is_empty() {
                        self.handle_settings_actions(actions);
                    }
                } else {
                    self.render_status_line(ui);
                    self.render_messages(ui);
                }
            });
    }

    fn render_top_bar(&mut self, ui: &mut egui::Ui) {
        let mut button_zone = Rect::NOTHING;

        egui::TopBottomPanel::top("topbar")
            .exact_height(TITLE_BAR_HOT_ZONE_HEIGHT)
            .frame(
                egui::Frame::default()
                    .fill(Color32::TRANSPARENT)
                    .inner_margin(egui::Margin::symmetric(10, 0)),
            )
            .show_inside(ui, |ui| {
                self.topbar_rect = ui.max_rect();
                ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                    // 设置视图的标题要够重（图里是整页唯一的大字），聊天视图保持克制
                    let settings_open = self.settings.is_open();
                    let title = if settings_open { "设置" } else { "翻译" };
                    let mut title_text = RichText::new(title)
                        .color(TEXT_MAIN)
                        .size(if settings_open { 15.0 } else { 13.0 });
                    if settings_open {
                        title_text = title_text.strong();
                    }
                    ui.label(title_text);
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        // 从右到左：关闭、置顶、清空、设置、锁（S5：齿轮在锁左侧）
                        if icon_button(ui, GLYPH_CLOSE, "×", "关闭面板（结束本程序）", ICON_ENABLED) {
                            ui.ctx().send_viewport_cmd(ViewportCommand::Close);
                        }
                        let topmost_resp = icon_button(
                            ui,
                            GLYPH_TOPMOST,
                            "顶",
                            if self.topmost { "当前：置顶" } else { "当前：不置顶" },
                            if self.topmost { ICON_ENABLED } else { ICON_DISABLED },
                        );
                        if topmost_resp {
                            self.topmost = !self.topmost;
                            // egui 0.32 的 ViewportCommand::AlwaysOnTop 是单次带到顶，
                            // 运行时切换用 Win32（与雷达 Topmost 同语义）
                            #[cfg(windows)]
                            if self.hwnd != 0 {
                                win32::set_topmost(self.hwnd, self.topmost);
                            }
                        }
                        if icon_button(ui, GLYPH_CLEAR, "清", "清空显示", ICON_ENABLED) {
                            self.state.clear_display();
                        }
                        if icon_button(
                            ui,
                            GLYPH_SETTINGS,
                            "设",
                            if self.settings.is_open() { "返回聊天" } else { "打开设置" },
                            ICON_ENABLED,
                        ) {
                            info!(
                                "[设置] gear clicked (open={} hwnd={})",
                                self.settings.is_open(),
                                self.hwnd
                            );
                            if self.settings.is_open() {
                                self.settings.close();
                            } else {
                                let store = lock_or_recover(&self.store);
                                self.settings.open(store.config(), &self.app_config, &store);
                                self.settings.set_opacity(self.opacity);
                                self.settings.set_only_translation(self.only_translation);
                                // 用户主动打开设置 = 明确要交互：NOACTIVATE 本帧起
                                // 已由 sync_exstyle 清除，这里再发一次激活请求
                                // 把焦点要到面板上（双保险，S5 QA 焦点修复）。
                                ui.ctx().send_viewport_cmd(ViewportCommand::Focus);
                            }
                        }
                        let (glyph, fallback, tooltip, color) = if self.locked {
                            (GLYPH_LOCK, "锁", "当前：锁定（位置与大小已锁死，拖不动）", COLOR_LOCKED)
                        } else {
                            (GLYPH_UNLOCK, "解", "当前：解锁（可拖动 / 可缩放）", ICON_ENABLED)
                        };
                        if icon_button(ui, glyph, fallback, tooltip, color) {
                            self.locked = !self.locked;
                            self.inside_since = None;
                            if self.locked {
                                self.set_interactive(false); // 恢复穿透
                            } else {
                                self.set_interactive(true);
                            }
                            // 记住这次选择：重启 / 重开设置都按它恢复
                            self.persist_placement();
                        }
                    });
                });
            });

        // 按钮热区：本帧按钮矩形并集，拖动判定要绕开它
        // （闭包里借用了 self，这里重新收集——用顶栏右侧固定宽度近似即可）
        let bar = self.topbar_rect;
        if bar.is_positive() {
            // 5 个按钮 × (按钮宽 ~24px + 间距)，右侧 150px 缓冲足够覆盖
            button_zone = Rect::from_min_size(
                Pos2::new(bar.right() - 150.0, bar.top()),
                Vec2::new(150.0, bar.height()),
            );
        }
        self.button_zone = button_zone;
    }

    fn render_status_line(&mut self, ui: &mut egui::Ui) {
        egui::TopBottomPanel::bottom("statusline")
            .exact_height(22.0)
            .frame(
                egui::Frame::default()
                    .fill(Color32::TRANSPARENT)
                    .inner_margin(egui::Margin::symmetric(10, 0)),
            )
            .show_inside(ui, |ui| {
                ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                    // 五态（S5 T04 扩展）：路径未配置/无效 → 引导进设置（可点击）；
                    // 管线死亡 → 后台管线未运行；供应商请求失败 → 连不上：<原因>
                    // （恢复成功自动回到正常态）；否则常规态。
                    let path_unconfigured = self.app_config.valid_game_dir().is_none();
                    let pipeline_dead = match &self.event_rx {
                        Some(rx) => rx.is_closed(),
                        None => true,
                    };
                    let endpoint_error: Option<String> = self
                        .endpoint_error
                        .lock()
                        .map(|slot| slot.clone())
                        .unwrap_or(None);
                    // 生效供应商：显示名优先，老配置（无 Name）回退 id。
                    // 以前它挂在右侧的 right_to_left 布局里，320 宽的窗口下会被
                    // 左侧状态挤没（一个字都不显示）—— 现在并进同一行，且可截断。
                    let config = lock_or_recover(&self.store).config().clone();
                    let provider = config.active().map(|ep| {
                        let name = ep.name.trim();
                        if name.is_empty() { ep.id.clone() } else { name.to_string() }
                    });

                    let (status, color) = if path_unconfigured {
                        ("未配置游戏路径 · 点这里打开设置".to_string(), TEXT_WARN)
                    } else if pipeline_dead {
                        ("后台管线未运行".to_string(), TEXT_WARN)
                    } else if let Some(reason) = endpoint_error {
                        (format!("连不上：{}", reason), TEXT_WARN)
                    } else {
                        (
                            format!(
                                "已连接 · 本局 {} 条 · 已译 {} 条",
                                self.state.chat_count(),
                                self.coordinator.translated_count()
                            ),
                            TEXT_STATUS,
                        )
                    };
                    let status = match (provider, config.enabled) {
                        (Some(name), true) => format!("{} · {}", name, status),
                        (Some(_), false) => format!("{} · 翻译未启用", status),
                        (None, _) => format!("{} · 没有模型供应商", status),
                    };
                    let mut label = egui::Label::new(
                        RichText::new(status).color(color).size(11.0),
                    )
                    // 窄窗口下截断成"…"，不再整块消失
                    .truncate();
                    if path_unconfigured {
                        label = label.sense(egui::Sense::click());
                    }
                    let resp = ui.add(label);
                    if path_unconfigured && resp.clicked() {
                        // 状态行引导：点开设置视图
                        let store = lock_or_recover(&self.store);
                        self.settings.open(store.config(), &self.app_config, &store);
                        self.settings.set_opacity(self.opacity);
                        self.settings.set_only_translation(self.only_translation);
                    }
                });
            });
    }

    fn render_messages(&mut self, ui: &mut egui::Ui) {
        // 先收集要渲染的数据，避免渲染闭包与 &mut self 打架
        struct RenderRow {
            badge: Option<(&'static str, Color32)>,
            body: String,
            original: Option<String>,
        }
        let rows: Vec<RenderRow> = self
            .state
            .visible_rows()
            .into_iter()
            .map(|row| {
                let (badge, body) = row.headline_parts();
                let badge = badge.map(|(text, team)| {
                    let color = match team {
                        Team::Allied => COLOR_ALLY,
                        Team::Enemy => COLOR_ENEMY,
                        Team::Unknown => TEXT_MAIN,
                    };
                    (text, color)
                });
                RenderRow {
                    badge,
                    body,
                    original: row.original_line().map(str::to_string),
                }
            })
            .collect();

        ScrollArea::vertical()
            .auto_shrink([false, false])
            .stick_to_bottom(true)
            .show(ui, |ui| {
                if rows.is_empty() {
                    ui.centered_and_justified(|ui| {
                        ui.label(
                            RichText::new("等待本局聊天…")
                                .color(TEXT_ORIGINAL)
                                .size(12.0),
                        );
                    });
                    return;
                }
                egui::Frame::default()
                    .inner_margin(egui::Margin::symmetric(10, 6))
                    .show(ui, |ui| {
                        for row in &rows {
                            ui.horizontal_wrapped(|ui| {
                                if let Some((badge_text, badge_color)) = row.badge {
                                    ui.label(RichText::new(badge_text).color(badge_color).size(13.0));
                                }
                                ui.label(RichText::new(&row.body).color(TEXT_MAIN).size(13.0));
                            });
                            // 仅显示译文：原文小字整行不画（行高也跟着降）
                            if !self.only_translation {
                                if let Some(original) = &row.original {
                                    ui.label(
                                        RichText::new(original)
                                            .color(TEXT_ORIGINAL)
                                            .size(11.0),
                                    );
                                }
                            }
                            ui.add_space(3.0);
                        }
                    });
            });
    }
}

impl eframe::App for PanelApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if !self.initialized {
            self.first_frame_init(ctx);
        }

        self.poll_events();
        self.poll_dir_pick();
        self.poll_worker_results();
        self.poll_demo();
        self.request_translations();
        self.ensure_hwnd();
        self.sync_exstyle();

        // 设置视图打开期间强制接管整窗（设计裁决 6）：表单交互（输入框/
        // 下拉/滚动）对 150ms 悬停接管语义完全不可用。跳过悬停判定、
        // 每帧 set_interactive(true)；locked 记账位不动，关闭后
        // update_click_through 立即恢复原有锁定语义。
        if self.settings.is_open() {
            self.set_interactive(true);
        } else {
            self.update_click_through(ctx);
        }

        // 手势空闲且布局命令已落地时，用 OS 真实矩形校准记账
        if !self.drag.armed
            && !self.resize.armed
            && self
                .initialized_at
                .map(|t| t.elapsed() >= Duration::from_secs(1))
                .unwrap_or(false)
        {
            self.sync_window_rect_from_os(ctx);
        }

        self.handle_pointer(ctx);
        self.render(ctx);

        // 悬停接管/演示注入都依赖时间推进；50ms 一帧足够跟手又不烧 CPU
        ctx.request_repaint_after(Duration::from_millis(50));
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        info!("Panel closed, shutting down background pipeline");
        if let Some(handles) = self.handles.take() {
            handles.shutdown();
        }
    }

    // 透明窗口的底色必须全透明，否则 glow 会用不透明色把透明度吃掉
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        [0.0, 0.0, 0.0, 0.0]
    }
}

/// 边缘 → 光标形状（雷达 CursorFor 的映射）。
fn cursor_for(edges: EdgeFlags) -> CursorIcon {
    let l = edges.contains(EdgeFlags::LEFT);
    let r = edges.contains(EdgeFlags::RIGHT);
    let t = edges.contains(EdgeFlags::TOP);
    let b = edges.contains(EdgeFlags::BOTTOM);
    match (l, r, t, b) {
        (true, false, false, false) | (false, true, false, false) => CursorIcon::ResizeHorizontal,
        (false, false, true, false) | (false, false, false, true) => CursorIcon::ResizeVertical,
        (true, false, true, false) | (false, true, false, true) => CursorIcon::ResizeNwSe,
        (true, false, false, true) | (false, true, true, false) => CursorIcon::ResizeNeSw,
        _ => CursorIcon::Default,
    }
}

/// 图标按钮：图标字体可用时用 MDL2 字形，否则退化为文字占位。
pub(crate) fn icon_button(
    ui: &mut egui::Ui,
    glyph: char,
    fallback: &str,
    tooltip: &str,
    color: Color32,
) -> bool {
    let text = match fonts::icon_family() {
        Some(family) => RichText::new(glyph.to_string())
            .family(family)
            .size(14.0),
        None => RichText::new(fallback).size(12.0),
    }
    .color(color);
    let response = ui.add(
        egui::Button::new(text)
            .fill(Color32::TRANSPARENT)
            .stroke(Stroke::NONE),
    );
    response.on_hover_text(tooltip).clicked()
}

/// 只夹位置不夹尺寸（拖动结束时用；尺寸在 apply_resize 里已经夹过）。
fn clamp_pos(mut placement: Placement, work: &WorkArea) -> Placement {
    let min_left = (work.left + interaction::EDGE_SNAP) as f64;
    let min_top = (work.top + interaction::EDGE_SNAP) as f64;
    let max_left = (work.right - interaction::EDGE_SNAP) as f64 - placement.width;
    let max_top = (work.bottom - interaction::EDGE_SNAP) as f64 - placement.height;
    placement.left = placement.left.clamp(min_left, max_left.max(min_left));
    placement.top = placement.top.clamp(min_top, max_top.max(min_top));
    placement
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 状态行「连不上：<原因>」的摘要必须单行、有界，且不丢关键信息。
    #[test]
    fn short_reason_collapses_and_truncates() {
        // 多行/多空白错误文本压成单行
        let multi = short_reason("HTTP 401 \n\n Unauthorized   \n bad key");
        assert_eq!(multi, "HTTP 401 Unauthorized bad key");
        // 超长截断到 48 字符 + 省略号
        let long = short_reason(&"x".repeat(200));
        assert!(long.chars().count() == 49);
        assert!(long.ends_with('…'));
        // 短文本原样
        assert_eq!(short_reason("超时（10s）"), "超时（10s）");
    }

    /// 共享 store 的加锁姿势：正常路径拿到数据；毒锁也能恢复（不 panic）。
    #[test]
    fn shared_store_lock_and_poison_recovery() {
        let store: Arc<Mutex<TranslationStore>> =
            Arc::new(Mutex::new(TranslationStore::new_in(PathBuf::from("."))));
        {
            let guard = lock_or_recover(&store);
            // new_in 走默认配置 → 恰好一张空白供应商卡片（空端点兜底）
            assert_eq!(guard.config().endpoints.len(), 1);
            assert_eq!(guard.config().endpoints[0].id, "endpoint1");
        }
        // 制造毒锁：持锁 panic
        {
            let store = store.clone();
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
                let _guard = store.lock().unwrap();
                panic!("poison the lock");
            }));
        }
        // 中毒后依然能拿锁读数据（S5 风险 1 的规避姿势）
        let guard = lock_or_recover(&store);
        assert_eq!(guard.config().endpoints.len(), 1);
        assert_eq!(guard.config().endpoints[0].id, "endpoint1");
    }
}
