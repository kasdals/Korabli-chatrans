//! 面板配色（S8）：**纯黑背景 + 纯白正文**，全部取值按 WCAG AA 校验。
//!
//! 为什么单独成模块：颜色原来散在 `app.rs`（面板）与 `settings.rs`（设置）
//! 两处、口径不一（`#F2F2F2` 主文 / `#8A8A8A` 标签 / `#6A6A6A` 图标）。
//! 统一到一张色板后，"改一次全局生效"，也让对比度可以集中核算。
//!
//! 对比度（相对纯黑 `#000000`，WCAG 2.1 相对亮度公式）：
//!   · `TEXT_MAIN`      #FFFFFF → 21.0:1
//!   · `TEXT_ORIGINAL`  #C9C9C9 → 12.6:1
//!   · `TEXT_STATUS`    #B4B4B4 →  9.9:1
//!   · `ICON_DISABLED`  #9A9A9A →  6.3:1（图标要求 ≥3:1）
//!   · `TEXT_OK`        #7CE0A8 → 12.0:1
//!   · `TEXT_WARN`      #FFB86B → 12.4:1
//!   · `TEXT_ERROR`     #FF8A8A → 10.1:1
//!
//! 面板底色用 **alpha=255 的纯黑**：半透明（原来 242）会让桌面/终端文字
//! 从背后透上来，小字号下直接变成"重影"，是最伤可读性的一项。

use eframe::egui::{self, Color32, Stroke};

// ── 底色与描边 ──────────────────────────────────────────────────────────

/// 面板底色：纯黑。透明度由 [`panel_bg`] 按用户设置算（默认 100% 不透明）。
pub const BG: Color32 = Color32::BLACK;

/// 透明度可调范围（百分比）。下限 40%：再低就只剩一层灰雾，
/// 白字在亮色游戏画面上会糊成一片，属于"能调但调了没法用"。
pub const OPACITY_MIN: u8 = 40;
/// 上限固定 100%（完全不透明）。
pub const OPACITY_MAX: u8 = 100;
/// 默认 100%：与"纯黑背景"的既定观感一致。
pub const OPACITY_DEFAULT: u8 = 100;
/// 分组/卡片底：比纯黑略亮一档，用于在纯黑上"切"出层次。
pub const SURFACE: Color32 = Color32::from_rgb(0x0D, 0x0D, 0x0D);
/// 面板圆角半径（DIP）。窗口本身是无边框透明窗，圆角靠"画"出来：
/// 面板底色按这个半径填充，四角之外保持 alpha=0 → 桌面透出来，成为圆角。
pub const PANEL_CORNER_RADIUS: u8 = 12;

/// 输入框底：比纯黑面板亮一档（设计图里输入框是"浮"在底上的）。
pub const FIELD_BG: Color32 = Color32::from_rgb(0x23, 0x23, 0x26);
/// 输入框描边。
pub const FIELD_BORDER: Color32 = Color32::from_rgb(0x35, 0x35, 0x3A);
/// 分隔线 / 面板外框。
pub const BORDER: Color32 = Color32::from_rgb(0x2E, 0x2E, 0x2E);
/// 普通按钮底（次级操作：浏览…／自动发现／获取模型列表）。
pub const BUTTON_BG: Color32 = Color32::from_rgb(0x26, 0x26, 0x2A);
/// 主按钮底（保存配置）——比次级亮一档，一眼能分出主次。
pub const BUTTON_PRIMARY_BG: Color32 = Color32::from_rgb(0x3A, 0x3A, 0x3F);
/// 按钮悬停底。
pub const BUTTON_HOVER_BG: Color32 = Color32::from_rgb(0x2E, 0x2E, 0x2E);
/// 生效模型行的底色：设计图里是一条中性灰带（不再用绿色，避免和"成功"混淆）。
pub const ROW_ACTIVE_BG: Color32 = Color32::from_rgb(0x26, 0x26, 0x2A);

// ── 文字 ────────────────────────────────────────────────────────────────

/// 正文（译文、标题、模型名）：纯白。
pub const TEXT_MAIN: Color32 = Color32::WHITE;
/// 次级正文（原文小字、空态说明）。
pub const TEXT_ORIGINAL: Color32 = Color32::from_rgb(0xC9, 0xC9, 0xC9);
/// 标签、状态行。
pub const TEXT_STATUS: Color32 = Color32::from_rgb(0xB4, 0xB4, 0xB4);
/// 警告（未配置 / 连不上 / 拉取中）。
pub const TEXT_WARN: Color32 = Color32::from_rgb(0xFF, 0xB8, 0x6B);
/// 失败提示。
pub const TEXT_ERROR: Color32 = Color32::from_rgb(0xFF, 0x8A, 0x8A);
/// 成功提示。
pub const TEXT_OK: Color32 = Color32::from_rgb(0x7C, 0xE0, 0xA8);

// ── 图标与语义色 ────────────────────────────────────────────────────────

/// 可用图标：纯白。
pub const ICON_ENABLED: Color32 = Color32::WHITE;
/// 次要图标（模型行重命名/删除）：6.3:1，比原来的 #6A6A6A（3.5:1）明显清楚。
pub const ICON_DISABLED: Color32 = Color32::from_rgb(0x9A, 0x9A, 0x9A);
/// 锁定态锁图标（保持琥珀色，作为唯一的"状态色"）。
pub const COLOR_LOCKED: Color32 = Color32::from_rgb(0xFF, 0xD4, 0x79);
/// 友方徽标。
pub const COLOR_ALLY: Color32 = Color32::from_rgb(0x6F, 0xE3, 0xB4);
/// 敌方徽标。
pub const COLOR_ENEMY: Color32 = Color32::from_rgb(0xFF, 0x9A, 0x6B);
/// 强调色（选中、链接、生效标记）。
pub const ACCENT: Color32 = Color32::from_rgb(0x6F, 0xE3, 0xB4);

// ── 兼容层：面板原先的名字（app.rs / settings.rs 沿用）───────────────────

/// 面板底色：纯黑 + 按百分比换算的不透明度（越界自动夹到
/// [`OPACITY_MIN`, `OPACITY_MAX`]，避免手改配置文件写出不可用的值）。
///
/// 只有**背景**变透明，文字/图标始终全不透明 —— 调低透明度得到的是
/// "文字浮在游戏画面上"，不会连带把字也调淡。
pub fn panel_bg(percent: u8) -> Color32 {
    let percent = percent.clamp(OPACITY_MIN, OPACITY_MAX) as u32;
    let alpha = ((percent * 255 + 50) / 100) as u8;
    Color32::from_rgba_unmultiplied(0, 0, 0, alpha)
}

/// 面板外框颜色（原 `panel_stroke()`；调用方自己包 `Stroke`）。
pub fn panel_stroke() -> Color32 {
    BORDER
}

/// 按钮底色（原 `settings::button_fill()`）。
pub fn button_fill() -> Color32 {
    BUTTON_BG
}

/// 生效模型行底色（原 `settings::row_highlight()`；原来是 alpha 15% 的淡绿，
/// 在纯黑上几乎不可见，改成实色深绿）。
pub fn row_highlight() -> Color32 {
    ROW_ACTIVE_BG
}

/// 把纯黑配色装进 egui 的 `Visuals`：一次性覆盖所有内置控件
/// （输入框、按钮、复选框、下拉、滚动条、选中高亮）。
///
/// 明暗两套都写成同一份：系统主题切到"亮色"时面板不会突然变白
/// ——这个窗口本来就是深色悬浮层，跟随系统只会导致白底白字。
pub fn apply(ctx: &egui::Context) {
    let visuals = build_visuals();
    ctx.set_theme(egui::ThemePreference::Dark);
    ctx.set_visuals_of(egui::Theme::Dark, visuals.clone());
    ctx.set_visuals_of(egui::Theme::Light, visuals);
}

fn build_visuals() -> egui::Visuals {
    let mut v = egui::Visuals::dark();

    v.dark_mode = true;
    v.panel_fill = BG;
    v.window_fill = BG;
    v.window_stroke = Stroke::new(1.0_f32, BORDER);
    v.extreme_bg_color = FIELD_BG;
    v.text_edit_bg_color = Some(FIELD_BG);
    v.code_bg_color = FIELD_BG;
    v.faint_bg_color = SURFACE;
    v.weak_text_color = Some(Color32::from_rgb(0x8A, 0x8A, 0x8A));
    v.warn_fg_color = TEXT_WARN;
    v.error_fg_color = TEXT_ERROR;
    v.hyperlink_color = ACCENT;
    // 禁用控件不要淡到看不见（ID 字段是只读展示，不是废信息）
    v.disabled_alpha = 0.8;

    v.selection.bg_fill = Color32::from_rgb(0x1E, 0x4A, 0x3A);
    v.selection.stroke = Stroke::new(1.0_f32, TEXT_MAIN);

    // 非交互态（标签、禁用输入框）：文字给到 #C0C0C0，不再用默认的 50% 灰
    v.widgets.noninteractive.bg_fill = SURFACE;
    v.widgets.noninteractive.weak_bg_fill = SURFACE;
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0_f32, BORDER);
    v.widgets.noninteractive.fg_stroke = Stroke::new(1.0_f32, Color32::from_rgb(0xC0, 0xC0, 0xC0));

    // 统一 6px 圆角（设计图里输入框/按钮都是小圆角）
    let radius = egui::CornerRadius::same(6);
    v.widgets.noninteractive.corner_radius = radius;
    v.widgets.inactive.corner_radius = radius;
    v.widgets.hovered.corner_radius = radius;
    v.widgets.active.corner_radius = radius;
    v.widgets.open.corner_radius = radius;

    // 常规控件（按钮、输入框）
    v.widgets.inactive.bg_fill = BUTTON_BG;
    v.widgets.inactive.weak_bg_fill = BUTTON_BG;
    v.widgets.inactive.bg_stroke = Stroke::new(1.0_f32, FIELD_BORDER);
    v.widgets.inactive.fg_stroke = Stroke::new(1.0_f32, TEXT_MAIN);

    // 悬停
    v.widgets.hovered.bg_fill = Color32::from_rgb(0x33, 0x33, 0x38);
    v.widgets.hovered.weak_bg_fill = BUTTON_HOVER_BG;
    v.widgets.hovered.bg_stroke = Stroke::new(1.0_f32, Color32::from_rgb(0x5A, 0x5A, 0x5A));
    v.widgets.hovered.fg_stroke = Stroke::new(1.0_f32, TEXT_MAIN);

    // 按下
    v.widgets.active.bg_fill = Color32::from_rgb(0x38, 0x38, 0x38);
    v.widgets.active.weak_bg_fill = Color32::from_rgb(0x38, 0x38, 0x38);
    v.widgets.active.bg_stroke = Stroke::new(1.0_f32, Color32::from_rgb(0x6A, 0x6A, 0x6A));
    v.widgets.active.fg_stroke = Stroke::new(1.0_f32, TEXT_MAIN);

    // 展开态（下拉/折叠头）
    v.widgets.open.bg_fill = BUTTON_HOVER_BG;
    v.widgets.open.weak_bg_fill = BUTTON_HOVER_BG;
    v.widgets.open.bg_stroke = Stroke::new(1.0_f32, Color32::from_rgb(0x5A, 0x5A, 0x5A));
    v.widgets.open.fg_stroke = Stroke::new(1.0_f32, TEXT_MAIN);

    v
}

#[cfg(test)]
mod tests {
    use super::*;

    /// WCAG 相对亮度（sRGB → 线性）。
    fn luminance(c: Color32) -> f64 {
        fn channel(v: u8) -> f64 {
            let s = v as f64 / 255.0;
            if s <= 0.04045 {
                s / 12.92
            } else {
                ((s + 0.055) / 1.055).powf(2.4)
            }
        }
        0.2126 * channel(c.r()) + 0.7152 * channel(c.g()) + 0.0722 * channel(c.b())
    }

    fn contrast(fg: Color32, bg: Color32) -> f64 {
        let (a, b) = (luminance(fg), luminance(bg));
        let (hi, lo) = if a > b { (a, b) } else { (b, a) };
        (hi + 0.05) / (lo + 0.05)
    }

    /// 正文与标签必须过 AA（4.5:1），图标过 3:1。
    #[test]
    fn text_meets_wcag_aa_on_pure_black() {
        for (name, color) in [
            ("TEXT_MAIN", TEXT_MAIN),
            ("TEXT_ORIGINAL", TEXT_ORIGINAL),
            ("TEXT_STATUS", TEXT_STATUS),
            ("TEXT_OK", TEXT_OK),
            ("TEXT_WARN", TEXT_WARN),
            ("TEXT_ERROR", TEXT_ERROR),
        ] {
            let ratio = contrast(color, BG);
            assert!(ratio >= 4.5, "{name} 对比度 {ratio:.1}:1 未达 AA");
        }
        for (name, color) in [("ICON_DISABLED", ICON_DISABLED), ("ACCENT", ACCENT)] {
            let ratio = contrast(color, BG);
            assert!(ratio >= 3.0, "{name} 对比度 {ratio:.1}:1 未达图标最低要求");
        }
    }

    /// 输入框里的文字同样要过 AA（背景是 #141414 而不是纯黑）。
    #[test]
    fn field_text_readable_on_field_bg() {
        assert!(contrast(TEXT_MAIN, FIELD_BG) >= 4.5);
        assert!(contrast(Color32::from_rgb(0xC0, 0xC0, 0xC0), FIELD_BG) >= 4.5);
    }

    /// 默认（100%）必须是完全不透明 —— 半透明会让桌面文字透上来。
    #[test]
    fn panel_background_default_is_fully_opaque() {
        assert_eq!(BG.a(), 255);
        assert_eq!(panel_bg(OPACITY_DEFAULT).a(), 255);
        assert_eq!(panel_bg(100).a(), 255);
    }

    /// 百分比 → alpha 的换算：单调、四舍五入、越界夹取。
    #[test]
    fn opacity_percent_maps_to_alpha() {
        assert_eq!(panel_bg(50).a(), 128, "50% 取四舍五入的 128");
        assert_eq!(panel_bg(OPACITY_MIN).a(), 102, "40% → 102");
        assert!(panel_bg(70).a() > panel_bg(50).a(), "单调递增");
        assert_eq!(panel_bg(0).a(), panel_bg(OPACITY_MIN).a(), "低于下限夹到下限");
        assert_eq!(panel_bg(255).a(), 255, "高于上限夹到 100%");
        // 无论透明度多少，RGB 都是纯黑（只有 alpha 变）
        for p in [OPACITY_MIN, 60, 80, 100] {
            let c = panel_bg(p);
            assert_eq!((c.r(), c.g(), c.b()), (0, 0, 0));
        }
    }
}
