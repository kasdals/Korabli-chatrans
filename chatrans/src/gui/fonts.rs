//! CJK 字体与图标字体加载（R2 风险应对，必做：egui 默认字体没有汉字）。
//!
//! - 正文：`C:\Windows\Fonts\msyh.ttc`（微软雅黑，Win11 必有），
//!   粗体 `msyhbd.ttc` 有就挂上，没有不致命；
//! - 图标：`SegFluent-Icons.ttf`（Win11）→ `segmdl2.ttf`（Win10）依次探测，
//!   挂到独立 family `icons`，顶栏按钮专用。都找不到时按钮退化为文字占位。

use std::sync::OnceLock;

use eframe::egui::{self, FontData, FontDefinitions, FontFamily};

const ICON_FAMILY: &str = "icons";

const CJK_FONTS: &[(&str, &str)] = &[
    ("C:\\Windows\\Fonts\\msyh.ttc", "msyh"),
    ("C:\\Windows\\Fonts\\msyhbd.ttc", "msyhbd"),
];

const ICON_FONTS: &[&str] = &[
    "C:\\Windows\\Fonts\\SegFluent-Icons.ttf",
    "C:\\Windows\\Fonts\\segmdl2.ttf",
];

/// 把 CJK 字体插到 Proportional 队首（fallback 顺序优先于 egui 默认字体），
/// 图标字体注册成独立 family。缺文件只 warn 不 panic。
pub fn install(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();

    let mut cjk_loaded = false;
    for (path, name) in CJK_FONTS {
        match std::fs::read(path) {
            Ok(bytes) => {
                fonts
                    .font_data
                    .insert((*name).to_string(), FontData::from_owned(bytes).into());
                fonts
                    .families
                    .entry(FontFamily::Proportional)
                    .or_default()
                    .push((*name).to_string());
                cjk_loaded = true;
            }
            Err(e) => {
                tracing::warn!("字体加载失败 {}: {}（中文字形可能显示为方框）", path, e);
            }
        }
    }
    if !cjk_loaded {
        tracing::error!("没有任何 CJK 字体可用，中文将显示为方框！");
    } else if let Some(list) = fonts.families.get_mut(&FontFamily::Proportional) {
        // regular（msyh）提到队首，粗体与 egui 默认字体依次做 fallback
        if let Some(pos) = list.iter().position(|n| n == "msyh") {
            let entry = list.remove(pos);
            list.insert(0, entry);
        }
    }

    let mut icon_loaded = false;
    for path in ICON_FONTS {
        match std::fs::read(path) {
            Ok(bytes) => {
                fonts
                    .font_data
                    .insert(ICON_FAMILY.to_string(), FontData::from_owned(bytes).into());
                fonts
                    .families
                    .insert(FontFamily::Name(ICON_FAMILY.into()), vec![ICON_FAMILY.to_string()]);
                icon_loaded = true;
                break;
            }
            Err(_) => continue,
        }
    }
    if !icon_loaded {
        tracing::warn!("Segoe Fluent Icons / MDL2 字体不可用，顶栏按钮将显示文字占位");
    }
    let _ = ICONS_READY.set(icon_loaded);

    ctx.set_fonts(fonts);
    tracing::info!(
        "字体就绪: cjk={}, icons={}",
        cjk_loaded,
        icon_loaded
    );
}

/// 顶栏图标字体是否就绪（install 时记录）。
static ICONS_READY: OnceLock<bool> = OnceLock::new();

/// 顶栏图标用的字体 family；图标字体没加载到时返回 None，
/// 调用方退化为文字占位按钮。
pub fn icon_family() -> Option<FontFamily> {
    if *ICONS_READY.get().unwrap_or(&false) {
        Some(FontFamily::Name(ICON_FAMILY.into()))
    } else {
        None
    }
}
