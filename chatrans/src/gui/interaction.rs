//! 悬浮窗交互的纯逻辑部分：位置持久化、拖动阈值、边缘缩放。
//!
//! 从雷达 `OverlayWindow.xaml.cs` 逐条移植（行为权威），几何运算全部用
//! 逻辑像素（DIP），纯函数可单测；Win32 FFI（工作区/穿透切换/光标位置）
//! 放在本模块底部，app.rs 只做编排。

use std::path::Path;

use eframe::egui::{Pos2, Rect, Vec2};
use serde::{Deserialize, Serialize};
use tracing::warn;

// ── 常量（与雷达逐字一致）────────────────────────────────────────────────

/// 拖动阈值（DIP）：位移没超过这个数就当"点一下"，窗口不移动、也不落盘。
pub const DRAG_THRESHOLD: f32 = 4.0;
/// 边缘命中带宽度（DIP）。6px 够好按，又不会把顶栏按钮区吃掉。
pub const RESIZE_EDGE_HIT: f32 = 6.0;
/// 窗口与工作区边缘至少留这么多，避免贴边后被系统边框吃掉。
pub const EDGE_SNAP: f32 = 8.0;
/// 位置文件里的值与夹取结果差超过这个像素数，视为脏数据，直接回落默认位置。
pub const MAX_CLAMP_DRIFT: f32 = 200.0;
/// 锁定态顶栏热区高度（DIP）。
pub const TITLE_BAR_HOT_ZONE_HEIGHT: f32 = 32.0;
/// 锁定态悬停多久才临时接管鼠标（毫秒）。
///
/// 雷达原值 150ms，S13 起提到 260ms：实战里拖镜头是**连续移动**，光靠时间
/// 阈值拦不住"从面板上扫过 150ms"这种情形；再加上下面的"必须停住"条件后，
/// 260ms 既不会让有意点按钮的人觉得慢，也给扫过留足余量。
pub const HOVER_DWELL_MS: u128 = 260;

/// 判定"手停住了"的单帧位移阈值（屏幕像素）。
///
/// 面板按 50ms 一帧刷新，2.5px/帧 ≈ 50px/s：正常瞄准/转视角远大于它，
/// 手搭在鼠标上不动的抖动远小于它。这一步是防误触的关键——只靠时间，
/// 慢速推镜头一样会触发接管。
pub const HOVER_STILL_EPS: f32 = 2.5;

/// 悬停接管判定：**必须停住**（单帧位移 ≤ eps）且停满 [`HOVER_DWELL_MS`]。
/// 纯函数，单测覆盖；app.rs 只负责喂光标位移。
pub fn hover_takeover(moved_px: f32, dwell_ms: u128) -> bool {
    moved_px <= HOVER_STILL_EPS && dwell_ms >= HOVER_DWELL_MS
}

/// 最小尺寸：320×240 → **280×200**（S15）。
///
/// 再小就没意义了：顶栏 32 + 底部固定区约 76 之后，留给内容的只剩几十像素；
/// 280 宽下"游戏路径/模型/底栏"三行都还能各占一行不折行（显示区会自动换行）。
pub const MIN_PANEL_WIDTH: f32 = 280.0;
pub const MIN_PANEL_HEIGHT: f32 = 200.0;
pub const MAX_PANEL_WIDTH: f32 = 900.0;
pub const MAX_PANEL_HEIGHT: f32 = 800.0;
pub const DEFAULT_PANEL_WIDTH: f32 = 420.0;
pub const DEFAULT_PANEL_HEIGHT: f32 = 320.0;

// ── 位置持久化（placement.json，与雷达同目录同 schema）────────────────────

/// 窗口位置与尺寸（DIP）。字段名与雷达 placement.json 一致（小写）。
///
/// `locked`（S8 新增）记录锁定状态：`None` = 老文件没这一项 → 用产品默认
/// （锁定），`Some(true/false)` = 用户上次手动切到的状态。
/// `opacity`（S8）记录面板不透明度百分比：`None` = 老文件 → 用默认 100%。
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Placement {
    pub left: f64,
    pub top: f64,
    #[serde(default)]
    pub width: f64,
    #[serde(default)]
    pub height: f64,
    #[serde(default)]
    pub locked: Option<bool>,
    #[serde(default)]
    pub opacity: Option<u8>,
    /// 仅显示译文（S13）：`None` = 老文件 → 默认显示原文+译文。
    #[serde(default)]
    pub only_translation: Option<bool>,
}

/// 启动时决定锁定状态：环境变量注入（自动化测试）优先于落盘值，
/// 落盘缺失则回到产品默认（锁定）。
pub fn resolve_locked(saved: Option<bool>, env_unlocked: bool) -> bool {
    if env_unlocked {
        false
    } else {
        saved.unwrap_or(true)
    }
}

/// 启动时决定"仅显示译文"：缺失 = 显示原文+译文（保持老行为）。
pub fn resolve_only_translation(saved: Option<bool>) -> bool {
    saved.unwrap_or(false)
}

/// 启动时决定不透明度：落盘值越界（手改文件）夹回合法区间，缺失用默认。
pub fn resolve_opacity(saved: Option<u8>) -> u8 {
    saved
        .unwrap_or(crate::gui::theme::OPACITY_DEFAULT)
        .clamp(crate::gui::theme::OPACITY_MIN, crate::gui::theme::OPACITY_MAX)
}

/// 读位置文件。读不到 / 解析失败 / NaN / 缺 left/top → None（当作从没记过）。
/// width/height 是可选字段：老文件没有就返回 0，调用方保持默认尺寸。
pub fn load_placement(path: &Path) -> Option<Placement> {
    let text = std::fs::read_to_string(path).ok()?;
    let placement: Placement = serde_json::from_str(&text).ok()?;
    if placement.left.is_nan()
        || placement.top.is_nan()
        || placement.left.is_infinite()
        || placement.top.is_infinite()
    {
        return None;
    }
    Some(placement)
}

/// 真正写文件：临时文件 + rename 替换，保留 1 位小数（与雷达同格式）。
/// 位置存不下不算错：下次用默认位置就好。
pub fn save_placement(path: &Path, placement: &Placement) {
    let payload = serde_json::json!({
        "left": (placement.left * 10.0).round() / 10.0,
        "top": (placement.top * 10.0).round() / 10.0,
        "width": (placement.width * 10.0).round() / 10.0,
        "height": (placement.height * 10.0).round() / 10.0,
        // 锁定状态与位置同文件同一次写入：重启后一次读取就能全部恢复
        "locked": placement.locked.unwrap_or(true),
        "opacity": placement
            .opacity
            .unwrap_or(crate::gui::theme::OPACITY_DEFAULT),
        "only_translation": placement.only_translation.unwrap_or(false),
    })
    .to_string();
    let result = (|| -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let temp = path.with_extension("tmp");
        std::fs::write(&temp, payload.as_bytes())?;
        std::fs::rename(&temp, path)
    })();
    if let Err(e) = result {
        warn!("placement.json 写入失败（{}）: {:?}", e, path);
    }
}

/// 工作区（DIP，逻辑像素）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WorkArea {
    pub left: f32,
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
}

impl WorkArea {
    pub fn width(&self) -> f32 {
        self.right - self.left
    }
    pub fn height(&self) -> f32 {
        self.bottom - self.top
    }
}

/// 把位置/尺寸夹进工作区（四周留 EDGE_SNAP），返回夹取结果。
pub fn clamp_placement(placement: &Placement, work: &WorkArea) -> Placement {
    let width = if placement.width > 0.0 { placement.width as f32 } else { DEFAULT_PANEL_WIDTH };
    let height = if placement.height > 0.0 { placement.height as f32 } else { DEFAULT_PANEL_HEIGHT };

    let min_left = work.left + EDGE_SNAP;
    let min_top = work.top + EDGE_SNAP;
    let max_left = (work.right - width - EDGE_SNAP).max(min_left);
    let max_top = (work.bottom - height - EDGE_SNAP).max(min_top);

    Placement {
        left: placement.left.clamp(min_left as f64, max_left as f64),
        top: placement.top.clamp(min_top as f64, max_top as f64),
        width: placement.width,
        height: placement.height,
        // 夹取只动几何，界面状态原样带过
        locked: placement.locked,
        opacity: placement.opacity,
        only_translation: placement.only_translation,
    }
}

/// 默认位置：屏幕右侧、略高于中线。不放右下角 —— 那里是 WoWS 的小地图，
/// 也不放中下（消耗品/伤害飘字都在那）。
pub fn default_placement(work: &WorkArea, width: f32, height: f32) -> Placement {
    let left = work.right - width - 24.0;
    let mut top = work.top + (work.height() - height) * 0.42;
    if top < work.top + EDGE_SNAP {
        top = work.top + EDGE_SNAP;
    }
    Placement {
        left: left as f64,
        top: top as f64,
        width: width as f64,
        height: height as f64,
        // 默认位置不携带界面状态：由调用方（app）用当前真实值补上
        locked: None,
        opacity: None,
        only_translation: None,
    }
}

/// 启动恢复：读文件 → 尺寸夹进 min/max → 位置夹进工作区。
/// 差得离谱（>MAX_CLAMP_DRIFT）→ 回默认位置并要求写回；
/// 被夹过（>0.5）→ 把修正值写回，下次就不用再夹。
/// 返回 `(最终 placement, 是否需要写盘自愈)`。
pub fn restore_placement(path: &Path, work: &WorkArea) -> (Placement, bool) {
    let Some(saved) = load_placement(path) else {
        let default = default_placement(work, DEFAULT_PANEL_WIDTH, DEFAULT_PANEL_HEIGHT);
        return (default, true);
    };

    // 尺寸先落到最终值，后面的位置夹取才是按真实尺寸算的
    let width = if saved.width > 0.0 {
        saved.width.clamp(MIN_PANEL_WIDTH as f64, MAX_PANEL_WIDTH as f64)
    } else {
        DEFAULT_PANEL_WIDTH as f64
    };
    let height = if saved.height > 0.0 {
        saved.height.clamp(MIN_PANEL_HEIGHT as f64, MAX_PANEL_HEIGHT as f64)
    } else {
        DEFAULT_PANEL_HEIGHT as f64
    };
    let sized = Placement { width, height, ..saved };

    let clamped = clamp_placement(&sized, work);
    if (clamped.left - saved.left).abs() > MAX_CLAMP_DRIFT as f64
        || (clamped.top - saved.top).abs() > MAX_CLAMP_DRIFT as f64
    {
        // 差得离谱（换显示器 / 分辨率 / 脏文件）→ 不猜，直接回到默认位置
        let default = default_placement(work, width as f32, height as f32);
        return (default, true);
    }

    let drifted = (clamped.left - saved.left).abs() > 0.5 || (clamped.top - saved.top).abs() > 0.5;
    (clamped, drifted)
}

// ── 拖动（F8：带阈值的受控拖动）──────────────────────────────────────────

/// 拖动状态机。坐标一律 DIP（egui 的指针坐标本来就是逻辑像素，
/// 不需要雷达那套 GetCursorPos 物理像素 ÷ DPI 的换算）。
#[derive(Debug, Default)]
pub struct DragState {
    pub armed: bool,
    pub moved: bool,
    pub start_pointer: Pos2,
    pub start_window: Pos2,
}

impl DragState {
    /// 顶栏按下。锁定态不进入此函数（app.rs 在入口挡住）。
    pub fn begin(&mut self, pointer: Pos2, window: Pos2) {
        self.armed = true;
        self.moved = false;
        self.start_pointer = pointer;
        self.start_window = window;
    }

    /// 移动中：返回应设置的新窗口位置。阈值内一律不动（用户可能只是想点图标）。
    pub fn motion(&mut self, pointer: Pos2) -> Option<Pos2> {
        if !self.armed {
            return None;
        }
        let delta = pointer - self.start_pointer;
        if !self.moved {
            if delta.x.abs() < DRAG_THRESHOLD && delta.y.abs() < DRAG_THRESHOLD {
                return None;
            }
            self.moved = true;
        }
        Some(self.start_window + delta)
    }

    /// 结束拖动。只有真的移动过 ≥阈值 才落盘（返回 true 表示需要持久化）。
    pub fn end(&mut self) -> bool {
        let moved = self.moved;
        self.armed = false;
        self.moved = false;
        moved
    }
}

// ── 缩放（F11：自绘边缘缩放）────────────────────────────────────────────

/// 边缘标志。与雷达 ResizeEdges 一一对应。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct EdgeFlags(pub u8);

impl EdgeFlags {
    pub const NONE: EdgeFlags = EdgeFlags(0);
    pub const LEFT: EdgeFlags = EdgeFlags(1);
    pub const RIGHT: EdgeFlags = EdgeFlags(2);
    pub const TOP: EdgeFlags = EdgeFlags(4);
    pub const BOTTOM: EdgeFlags = EdgeFlags(8);

    pub fn contains(self, other: EdgeFlags) -> bool {
        self.0 & other.0 == other.0
    }

    pub fn union(self, other: EdgeFlags) -> EdgeFlags {
        EdgeFlags(self.0 | other.0)
    }

    pub fn is_none(self) -> bool {
        self.0 == 0
    }
}

/// 边缘命中判定：窗口内坐标 + 窗口尺寸 → 命中的边（6px 命中带）。
pub fn hit_test_edges(point: Pos2, size: Vec2) -> EdgeFlags {
    if point.x < 0.0 || point.y < 0.0 || point.x > size.x || point.y > size.y {
        return EdgeFlags::NONE;
    }
    let mut edges = EdgeFlags::NONE;
    if point.x <= RESIZE_EDGE_HIT {
        edges = edges.union(EdgeFlags::LEFT);
    }
    if point.x >= size.x - RESIZE_EDGE_HIT {
        edges = edges.union(EdgeFlags::RIGHT);
    }
    if point.y <= RESIZE_EDGE_HIT {
        edges = edges.union(EdgeFlags::TOP);
    }
    if point.y >= size.y - RESIZE_EDGE_HIT {
        edges = edges.union(EdgeFlags::BOTTOM);
    }
    edges
}

/// 缩放状态机。
#[derive(Debug)]
pub struct ResizeState {
    pub armed: bool,
    pub moved: bool,
    pub edges: EdgeFlags,
    pub start_pointer: Pos2,
    pub start_bounds: Rect,
}

impl Default for ResizeState {
    fn default() -> Self {
        ResizeState {
            armed: false,
            moved: false,
            edges: EdgeFlags::NONE,
            start_pointer: Pos2::ZERO,
            start_bounds: Rect::NOTHING,
        }
    }
}

impl ResizeState {
    /// 边缘按下开始缩放。锁定态不进入（app.rs 在入口挡住）。
    pub fn begin(&mut self, edges: EdgeFlags, pointer: Pos2, bounds: Rect) {
        self.armed = true;
        self.moved = false;
        self.edges = edges;
        self.start_pointer = pointer;
        self.start_bounds = bounds;
    }

    /// 移动中：返回新的窗口 bounds。与拖动共用同一个 4px 阈值。
    pub fn motion(&mut self, pointer: Pos2) -> Option<Rect> {
        if !self.armed {
            return None;
        }
        let delta = pointer - self.start_pointer;
        if !self.moved {
            if delta.x.abs() < DRAG_THRESHOLD && delta.y.abs() < DRAG_THRESHOLD {
                return None;
            }
            self.moved = true;
        }
        Some(apply_resize(self.start_bounds, self.edges, delta))
    }

    /// 结束缩放。返回 true 表示需要持久化。
    pub fn end(&mut self) -> bool {
        let moved = self.moved;
        self.armed = false;
        self.edges = EdgeFlags::NONE;
        self.moved = false;
        moved
    }
}

/// 按边缘与位移计算新 bounds：
///   · 右/下边跟随光标，左/上边反向；
///   · 尺寸夹进 min/max；
///   · 左/上边缘缩放时，右/下边钉住不动 —— 撞到最小尺寸后尤其明显。
pub fn apply_resize(start: Rect, edges: EdgeFlags, delta: Vec2) -> Rect {
    let mut left = start.left();
    let mut top = start.top();
    let mut width = start.width();
    let mut height = start.height();

    if edges.contains(EdgeFlags::RIGHT) {
        width = start.width() + delta.x;
    }
    if edges.contains(EdgeFlags::BOTTOM) {
        height = start.height() + delta.y;
    }
    if edges.contains(EdgeFlags::LEFT) {
        width = start.width() - delta.x;
    }
    if edges.contains(EdgeFlags::TOP) {
        height = start.height() - delta.y;
    }

    width = width.clamp(MIN_PANEL_WIDTH, MAX_PANEL_WIDTH);
    height = height.clamp(MIN_PANEL_HEIGHT, MAX_PANEL_HEIGHT);

    if edges.contains(EdgeFlags::LEFT) {
        left = start.right() - width;
    }
    if edges.contains(EdgeFlags::TOP) {
        top = start.bottom() - height;
    }

    Rect::from_min_size(Pos2::new(left, top), Vec2::new(width, height))
}

// ── Win32（工作区 / 点击穿透 / 光标）─────────────────────────────────────

#[cfg(windows)]
pub mod win32 {
    use std::ffi::c_void;

    use tracing::warn;

    #[repr(C)]
    struct RECT {
        left: i32,
        top: i32,
        right: i32,
        bottom: i32,
    }

    extern "system" {
        fn FindWindowW(class: *const u16, title: *const u16) -> isize;
        fn GetWindowLongPtrW(hwnd: isize, index: i32) -> isize;
        fn SetWindowLongPtrW(hwnd: isize, index: i32, value: isize) -> isize;
        fn GetCursorPos(point: *mut POINT) -> i32;
        fn GetWindowRect(hwnd: isize, rect: *mut RECT) -> i32;
        fn GetWindowThreadProcessId(hwnd: isize, pid: *mut u32) -> u32;
        fn SetWindowPos(
            hwnd: isize,
            after: isize,
            x: i32,
            y: i32,
            w: i32,
            h: i32,
            flags: u32,
        ) -> i32;
        fn SystemParametersInfoW(action: u32, param: u32, data: *mut c_void, init: u32) -> i32;
    }

    #[repr(C)]
    struct POINT {
        x: i32,
        y: i32,
    }

    const GWL_EXSTYLE: i32 = -20;
    const WS_EX_TRANSPARENT: isize = 0x0000_0020;
    const WS_EX_LAYERED: isize = 0x0008_0000;
    const WS_EX_TOOLWINDOW: isize = 0x0000_0080;
    const WS_EX_NOACTIVATE: isize = 0x0800_0000;
    const WS_EX_APPWINDOW: isize = 0x0004_0000;
    const SPI_GETWORKAREA: u32 = 0x0048;
    const HWND_TOPMOST: isize = -1;
    const HWND_NOTOPMOST: isize = -2;
    const SWP_NOMOVE: u32 = 0x0002;
    const SWP_NOSIZE: u32 = 0x0001;
    const SWP_NOACTIVATE: u32 = 0x0010;

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    /// 按标题找**本进程**的窗口句柄（eframe 不直接暴露 winit 句柄，与 spike
    /// 同一套办法；加 pid 校验防止同名资源管理器窗口冒领）。
    pub fn find_own_window(title: &str) -> isize {
        let hwnd = unsafe { FindWindowW(std::ptr::null(), wide(title).as_ptr()) };
        if hwnd == 0 {
            return 0;
        }
        let mut pid: u32 = 0;
        unsafe { GetWindowThreadProcessId(hwnd, &mut pid) };
        if pid == std::process::id() {
            hwnd
        } else {
            0
        }
    }

    /// 切换点击穿透（WS_EX_TRANSPARENT）。与雷达 ApplyClickThrough 同一机制。
    pub fn apply_click_through(hwnd: isize, on: bool) {
        unsafe {
            let cur = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
            let next = if on {
                cur | WS_EX_TRANSPARENT | WS_EX_LAYERED
            } else {
                cur & !WS_EX_TRANSPARENT
            };
            SetWindowLongPtrW(hwnd, GWL_EXSTYLE, next);
        }
    }

    /// 当前是否处于点击穿透。
    pub fn has_click_through(hwnd: isize) -> bool {
        unsafe { GetWindowLongPtrW(hwnd, GWL_EXSTYLE) & WS_EX_TRANSPARENT != 0 }
    }

    /// 面板的标准扩展样式整体校对（QA Bug#3）：
    ///   · WS_EX_TOOLWINDOW 置位 —— 不进 Alt-Tab（设计文档第 3 节第 10 条）；
    ///   · WS_EX_APPWINDOW 清位 —— winit 自己会挂上，导致面板出现在 Alt-Tab；
    ///   · WS_EX_LAYERED 置位、WS_EX_TRANSPARENT 按 `transparent` 参数；
    ///   · WS_EX_NOACTIVATE 按 `allow_activate` 参数（S5 QA 修复）：聊天
    ///     查看态保持置位（点击不激活、不抢游戏焦点）；设置视图打开期间
    ///     清除——用户明确要交互，点击必须能激活窗口、键盘才能进输入框；
    ///     关闭设置后恢复置位。
    /// winit 会在自己的样式刷新里整体重写扩展样式，所以每帧调用一次，
    /// 有差异才写（一次 GetWindowLongPtr + 比较，几乎零开销）。
    pub fn sync_overlay_exstyle(hwnd: isize, transparent: bool, allow_activate: bool) {
        unsafe {
            let cur = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
            let want = want_exstyle(cur, transparent, allow_activate);
            if want != cur {
                SetWindowLongPtrW(hwnd, GWL_EXSTYLE, want);
            }
        }
    }

    /// sync_overlay_exstyle 的目标样式纯函数（可单测）：在 `current` 的
    /// 基础上按三个开关位运算出 want 值。
    pub fn want_exstyle(current: isize, transparent: bool, allow_activate: bool) -> isize {
        let mut want = current;
        want |= WS_EX_TOOLWINDOW | WS_EX_LAYERED;
        want &= !WS_EX_APPWINDOW;
        if allow_activate {
            want &= !WS_EX_NOACTIVATE;
        } else {
            want |= WS_EX_NOACTIVATE;
        }
        if transparent {
            want |= WS_EX_TRANSPARENT;
        } else {
            want &= !WS_EX_TRANSPARENT;
        }
        want
    }

    /// 运行时切换置顶（雷达 Topmost 按钮同语义）。
    pub fn set_topmost(hwnd: isize, on: bool) {
        unsafe {
            SetWindowPos(
                hwnd,
                if on { HWND_TOPMOST } else { HWND_NOTOPMOST },
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
            );
        }
    }

    /// 光标的屏幕坐标（物理像素）。取不到返回 None。
    pub fn cursor_pos() -> Option<(i32, i32)> {
        let mut point = POINT { x: 0, y: 0 };
        if unsafe { GetCursorPos(&mut point) } != 0 {
            Some((point.x, point.y))
        } else {
            None
        }
    }

    extern "system" {
        fn GetSystemMetrics(index: i32) -> i32;
    }

    /// 主显示器工作区（物理像素）。SystemParametersInfoW 在部分环境返回失败
    /// （本机实测两种调用方都失败），退回 GetSystemMetrics 的全屏尺寸减任务
    /// 栏 —— 主显示器的工作区与它只差多显示器场景的边缘细节。
    pub fn work_area_physical() -> (i32, i32, i32, i32) {
        let mut rect = RECT { left: 0, top: 0, right: 0, bottom: 0 };
        let ok = unsafe {
            SystemParametersInfoW(SPI_GETWORKAREA, 0, &mut rect as *mut RECT as *mut c_void, 0)
        };
        if ok != 0 {
            return (rect.left, rect.top, rect.right, rect.bottom);
        }
        let (cx, cy) = unsafe { (GetSystemMetrics(0), GetSystemMetrics(1)) }; // SM_CXSCREEN / SM_CYSCREEN
        warn!(
            "SPI_GETWORKAREA failed ({}), falling back to full screen {}x{}",
            std::io::Error::last_os_error(),
            cx,
            cy
        );
        (0, 0, cx, cy)
    }

    /// 窗口矩形（物理像素，屏幕坐标）。取不到返回 None。
    pub fn window_rect(hwnd: isize) -> Option<(i32, i32, i32, i32)> {
        let mut rect = RECT { left: 0, top: 0, right: 0, bottom: 0 };
        if unsafe { GetWindowRect(hwnd, &mut rect) } != 0 {
            Some((rect.left, rect.top, rect.right, rect.bottom))
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn work() -> WorkArea {
        WorkArea { left: 0.0, top: 0.0, right: 1920.0, bottom: 1040.0 }
    }

    /// 悬停接管：镜头扫过时不许接管，只有"停住且够久"才接管。
    #[test]
    fn hover_takeover_requires_still_pointer() {
        // 快速/中速扫过：位移远超阈值 → 无论停多久都不接管
        assert!(!hover_takeover(40.0, 5_000));
        assert!(!hover_takeover(HOVER_STILL_EPS + 0.1, 5_000));
        // 停住了但时间不够 →
        assert!(!hover_takeover(0.4, HOVER_DWELL_MS - 1));
        // 停住且够久 → 接管
        assert!(hover_takeover(0.4, HOVER_DWELL_MS));
        assert!(hover_takeover(0.0, HOVER_DWELL_MS));
    }

    // ── placement ───────────────────────────────────────────────────────

    #[test]
    fn placement_round_trip() {
        let dir = std::env::temp_dir().join(format!("chatrans-placement-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("placement.json");

        assert!(load_placement(&path).is_none(), "文件不存在 → None");

        let p = Placement { left: 100.0, top: 55.5, width: 420.0, height: 320.0, locked: Some(false), opacity: Some(80), only_translation: Some(true) };
        save_placement(&path, &p);
        let loaded = load_placement(&path).unwrap();
        assert_eq!(loaded, p);
        assert!(!path.with_extension("tmp").exists(), "临时文件必须被替换掉");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_placement_rejects_garbage() {
        let dir = std::env::temp_dir().join(format!("chatrans-placement-bad-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("placement.json");

        std::fs::write(&path, "{not json").unwrap();
        assert!(load_placement(&path).is_none());

        std::fs::write(&path, r#"{"left": "x", "top": 1}"#).unwrap();
        assert!(load_placement(&path).is_none(), "类型不对也算坏");

        // 老版本没有 width/height → 返回 0，由调用方保持默认尺寸
        std::fs::write(&path, r#"{"left": 10, "top": 20}"#).unwrap();
        let p = load_placement(&path).unwrap();
        assert_eq!((p.left, p.top, p.width, p.height), (10.0, 20.0, 0.0, 0.0));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// S8：锁定状态与位置同文件落盘，重启后原样回来。
    #[test]
    fn locked_state_round_trip() {
        let dir = std::env::temp_dir().join(format!("chatrans-locked-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("placement.json");

        let unlocked = Placement { left: 10.0, top: 20.0, width: 420.0, height: 320.0, locked: Some(false), opacity: None, only_translation: None };
        save_placement(&path, &unlocked);
        assert_eq!(load_placement(&path).unwrap().locked, Some(false), "解锁状态要记住");

        let locked = Placement { locked: Some(true), ..unlocked };
        save_placement(&path, &locked);
        assert_eq!(load_placement(&path).unwrap().locked, Some(true), "锁定状态要记住");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 老版本文件没有 locked 字段：解析成 None（= 没记过），不能当坏文件丢掉位置。
    #[test]
    fn old_file_without_locked_is_none() {
        let dir = std::env::temp_dir().join(format!("chatrans-locked-old-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("placement.json");
        std::fs::write(&path, r#"{"left": 10, "top": 20, "width": 420, "height": 320}"#).unwrap();
        let p = load_placement(&path).unwrap();
        assert_eq!(p.locked, None);
        assert_eq!(p.left, 10.0, "位置照常恢复，不因缺字段丢失");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// S8：不透明度与位置同文件落盘，重启后原样回来。
    #[test]
    fn opacity_round_trip() {
        let dir = std::env::temp_dir().join(format!("chatrans-opacity-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("placement.json");

        let p = Placement { left: 0.0, top: 0.0, width: 420.0, height: 320.0, locked: None, opacity: Some(70), only_translation: None };
        save_placement(&path, &p);
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("\"opacity\":70"), "落盘内容可读: {text}");
        assert_eq!(load_placement(&path).unwrap().opacity, Some(70));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 老文件没有 opacity 字段 → None（不是 0！否则升级后面板会变全透明）。
    #[test]
    fn old_file_without_opacity_is_none() {
        let dir = std::env::temp_dir().join(format!("chatrans-opacity-old-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("placement.json");
        std::fs::write(&path, r#"{"left": 10, "top": 20, "width": 420, "height": 320, "locked": true}"#).unwrap();
        let p = load_placement(&path).unwrap();
        assert_eq!(p.opacity, None);
        assert_eq!(resolve_opacity(p.opacity), 100, "缺失 → 默认 100%");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 手改配置写出越界值：夹回 [40, 100]，不产生不可用界面。
    #[test]
    fn resolve_opacity_clamps() {
        assert_eq!(resolve_opacity(None), 100);
        assert_eq!(resolve_opacity(Some(0)), 40);
        assert_eq!(resolve_opacity(Some(15)), 40);
        assert_eq!(resolve_opacity(Some(65)), 65);
        assert_eq!(resolve_opacity(Some(200)), 100);
    }

    /// 启动决策：注入开关 > 落盘值 > 产品默认（锁定）。
    #[test]
    fn resolve_locked_precedence() {
        assert!(!resolve_locked(Some(true), true), "注入解锁优先于落盘锁定");
        assert!(!resolve_locked(None, true));
        assert!(resolve_locked(Some(true), false), "落盘锁定要恢复");
        assert!(!resolve_locked(Some(false), false), "落盘解锁要恢复");
        assert!(resolve_locked(None, false), "没有记录 → 默认锁定");
    }

    #[test]
    fn clamp_into_work_area() {
        let work = work();
        // 负坐标（历史上的脏值 -11）→ 夹到 work+8
        let p = Placement { left: -11.0, top: -5.0, width: 420.0, height: 320.0, locked: None, opacity: None, only_translation: None };
        let c = clamp_placement(&p, &work);
        assert_eq!((c.left, c.top), (8.0, 8.0));
        // 超出右/下边界
        let p = Placement { left: 5000.0, top: 5000.0, width: 420.0, height: 320.0, locked: None, opacity: None, only_translation: None };
        let c = clamp_placement(&p, &work);
        assert_eq!((c.left, c.top), (1920.0 - 420.0 - 8.0, 1040.0 - 320.0 - 8.0));
    }

    #[test]
    fn default_placement_right_side_above_middle() {
        let work = work();
        let p = default_placement(&work, 420.0, 320.0);
        assert!(p.left > 1400.0, "靠右侧：{:?}", p);
        let rel = (p.top - 8.0) / (work.height() - 320.0 - 8.0) as f64;
        assert!((rel - 0.42).abs() < 0.05, "略高于中线（42%）：{:?}", p);
        assert!((p.top as f32) >= work.top + EDGE_SNAP);
    }

    #[test]
    fn restore_self_heals_dirty_values() {
        let dir = std::env::temp_dir().join(format!("chatrans-restore-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("placement.json");
        let work = work();

        // 正常值：夹取不改变 → 不写盘
        std::fs::write(&path, r#"{"left": 100.0, "top": 100.0, "width": 420.0, "height": 320.0}"#).unwrap();
        let (p, write_back) = restore_placement(&path, &work);
        assert!(!write_back);
        assert_eq!((p.left, p.top), (100.0, 100.0));

        // 轻微越界（-11）：夹回 + 自愈写盘
        std::fs::write(&path, r#"{"left": -11.0, "top": 100.0, "width": 420.0, "height": 320.0}"#).unwrap();
        let (p, write_back) = restore_placement(&path, &work);
        assert!(write_back, "被夹过要写回，下次就不用再夹");
        assert_eq!(p.left, 8.0);

        // 差得离谱（换显示器/脏文件）：直接回落默认位置
        std::fs::write(&path, r#"{"left": 99999.0, "top": 100.0, "width": 420.0, "height": 320.0}"#).unwrap();
        let (p, write_back) = restore_placement(&path, &work);
        assert!(write_back);
        let default = default_placement(&work, 420.0, 320.0);
        assert_eq!((p.left, p.top), (default.left, default.top));

        // 老版本没有 width/height：保持默认 420×320
        std::fs::write(&path, r#"{"left": 100.0, "top": 100.0}"#).unwrap();
        let (p, write_back) = restore_placement(&path, &work);
        assert!(!write_back);
        assert_eq!((p.width, p.height), (420.0, 320.0));

        let _ = std::fs::remove_dir_all(&dir);
    }

    // ── drag ────────────────────────────────────────────────────────────

    #[test]
    fn drag_threshold_ignores_jitter() {
        let mut drag = DragState::default();
        drag.begin(Pos2::new(100.0, 100.0), Pos2::new(500.0, 300.0));
        // 阈值内（<4px）不动
        assert_eq!(drag.motion(Pos2::new(102.0, 101.0)), None);
        // 超过阈值开始跟随
        let new_pos = drag.motion(Pos2::new(110.0, 100.0)).unwrap();
        assert_eq!(new_pos, Pos2::new(510.0, 300.0), "窗口移动 = 起点 + 光标位移");
        assert!(drag.end(), "真的移动过 → 落盘");
    }

    #[test]
    fn drag_click_without_move_persists_nothing() {
        let mut drag = DragState::default();
        drag.begin(Pos2::new(100.0, 100.0), Pos2::new(500.0, 300.0));
        assert_eq!(drag.motion(Pos2::new(101.0, 102.0)), None);
        assert!(!drag.end(), "点一下不留痕迹（既不移动也不写盘）");
    }

    // ── resize ──────────────────────────────────────────────────────────

    fn bounds() -> Rect {
        Rect::from_min_size(Pos2::new(100.0, 100.0), Vec2::new(420.0, 320.0))
    }

    #[test]
    fn edge_hit_test() {
        let size = Vec2::new(420.0, 320.0);
        assert!(hit_test_edges(Pos2::new(2.0, 160.0), size).contains(EdgeFlags::LEFT));
        assert!(hit_test_edges(Pos2::new(418.0, 160.0), size).contains(EdgeFlags::RIGHT));
        assert!(hit_test_edges(Pos2::new(210.0, 3.0), size).contains(EdgeFlags::TOP));
        assert!(hit_test_edges(Pos2::new(210.0, 318.0), size).contains(EdgeFlags::BOTTOM));
        assert!(hit_test_edges(Pos2::new(210.0, 160.0), size).is_none(), "中间不是边缘");
        // 角：两条边同时命中
        let corner = hit_test_edges(Pos2::new(2.0, 2.0), size);
        assert!(corner.contains(EdgeFlags::LEFT) && corner.contains(EdgeFlags::TOP));
        // 窗口外不算
        assert!(hit_test_edges(Pos2::new(-1.0, 160.0), size).is_none());
    }

    #[test]
    fn resize_right_bottom_follows_cursor() {
        let out = apply_resize(bounds(), EdgeFlags::RIGHT, Vec2::new(50.0, 0.0));
        assert_eq!((out.left(), out.top()), (100.0, 100.0), "对边钉住");
        assert_eq!(out.width(), 470.0);

        let out = apply_resize(bounds(), EdgeFlags::BOTTOM, Vec2::new(0.0, -30.0));
        assert_eq!(out.height(), 290.0);
    }

    #[test]
    fn resize_left_anchors_right_edge() {
        let out = apply_resize(bounds(), EdgeFlags::LEFT, Vec2::new(50.0, 0.0));
        assert_eq!(out.width(), 370.0);
        assert_eq!(out.right(), 520.0, "右边缘钉住不动");
        assert_eq!(out.left(), 150.0);
    }

    #[test]
    fn resize_top_anchors_bottom_edge() {
        let out = apply_resize(bounds(), EdgeFlags::TOP, Vec2::new(0.0, 40.0));
        assert_eq!(out.height(), 280.0);
        assert_eq!(out.bottom(), 420.0, "下边缘钉住不动");
    }

    #[test]
    fn resize_min_clamp_keeps_anchor() {
        // 左边缘大幅向右拖（收窄）→ 撞最小宽度，右边缘仍然钉住
        let out = apply_resize(bounds(), EdgeFlags::LEFT, Vec2::new(500.0, 0.0));
        assert_eq!(out.width(), MIN_PANEL_WIDTH);
        assert_eq!(out.right(), 520.0, "撞到最小尺寸后对边也要钉住");
        assert_eq!(out.left(), 520.0 - MIN_PANEL_WIDTH);
    }

    #[test]
    fn resize_threshold_in_state_machine() {
        let mut resize = ResizeState::default();
        resize.begin(EdgeFlags::RIGHT, Pos2::new(418.0, 160.0), bounds());
        assert_eq!(resize.motion(Pos2::new(419.0, 160.0)), None, "阈值内不动");
        let out = resize.motion(Pos2::new(460.0, 160.0)).unwrap();
        assert_eq!(out.width(), 462.0);
        assert!(resize.end());
    }

    // ── 扩展样式（S5 QA 焦点修复）────────────────────────────────────────

    /// 位值对照（与 win32 模块常量一致，这里用字面量独立核对）。
    const WS_EX_TRANSPARENT: isize = 0x0000_0020;
    const WS_EX_LAYERED: isize = 0x0008_0000;
    const WS_EX_TOOLWINDOW: isize = 0x0000_0080;
    const WS_EX_NOACTIVATE: isize = 0x0800_0000;
    const WS_EX_APPWINDOW: isize = 0x0004_0000;

    #[cfg(windows)]
    #[test]
    fn want_exstyle_rules() {
        let fresh: isize = 0; // winit 新建窗口的近似基线

        // 1) 锁定聊天查看态：穿透 + 不可激活（不抢游戏焦点）
        let locked_chat = super::win32::want_exstyle(fresh, true, false);
        assert_eq!(
            locked_chat,
            WS_EX_TOOLWINDOW | WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_NOACTIVATE,
            "TOOLWINDOW/LAYERED/TRANSPARENT 置位，APPWINDOW 清位，NOACTIVATE 置位"
        );

        // 2) 解锁聊天查看态：不穿透但仍不可激活（验收：聊天查看态始终 NOACTIVATE）
        let unlocked_chat = super::win32::want_exstyle(fresh, false, false);
        assert_eq!(unlocked_chat & WS_EX_TRANSPARENT, 0, "解锁后不穿透");
        assert_ne!(unlocked_chat & WS_EX_NOACTIVATE, 0, "仍不可激活");
        assert_eq!(unlocked_chat & WS_EX_APPWINDOW, 0, "仍不进 Alt-Tab");

        // 3) 设置视图打开：清除 NOACTIVATE（点击可激活、键盘可进输入框）
        let settings_open = super::win32::want_exstyle(fresh, false, true);
        assert_eq!(settings_open & WS_EX_NOACTIVATE, 0, "可激活");
        assert_eq!(settings_open & WS_EX_TRANSPARENT, 0, "可交互");
        assert_ne!(settings_open & WS_EX_TOOLWINDOW, 0, "仍不进 Alt-Tab");

        // 4) 关闭设置立即恢复：从"设置打开"样式回到"锁定聊天"样式，NOACTIVATE 复位
        let restored = super::win32::want_exstyle(settings_open, true, false);
        assert_eq!(restored, locked_chat, "关闭设置后与锁定聊天态完全一致");

        // 5) 未知的既有位（其他组件挂的样式）不被误清
        let extra = 0x0000_0001isize;
        assert_ne!(
            super::win32::want_exstyle(extra, false, true) & extra,
            0,
            "既有未知位保留"
        );
    }
}
