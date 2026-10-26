pub mod processor;
pub mod live;
pub mod console;
pub mod interpreter;
pub mod ships;
pub mod runner;
pub mod translate;
pub mod app_config;
pub mod game_discovery;
pub mod logging;

/// 翻译悬浮窗面板（方案 C / S2）。仅 `gui` feature 编译，
/// 默认构建完全不依赖 egui/eframe 依赖树。
#[cfg(feature = "gui")]
pub mod gui;
