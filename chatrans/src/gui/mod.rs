//! 翻译悬浮窗面板（方案 C / S2，仅 `gui` feature 编译）。
//!
//! 行为移植自雷达已验收的 `OverlayWindow` 实现，逐条对照设计文档第 3 节。

pub mod app;
pub mod fonts;
pub mod interaction;
pub mod panel_state;
pub mod settings;
pub mod theme;

pub use app::{run_app, run_app_with_args};
