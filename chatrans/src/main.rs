//! 薄入口：初始化日志、解析命令行，其余全部在库层 [`chatrans::runner`]。
//!
//! （方案 C / R1 拆库：eframe 占主线程事件循环，库层保证"无 GUI 可跑"。）
//! S5 起 gui feature 下**无参数 = 双击启动 = GUI**：这个判定必须在
//! `ClientArgs::parse()` 之前做，否则 clap 因缺 `-r` 报错退出（双击闪退，
//! 设计文档第一节第 6 条）。带参数仍走 clap：`-r <dir>` 起 GUI 管线，
//! `--headless` 走 console（只打印原文）。

#![cfg_attr(all(windows, not(debug_assertions), feature = "gui"), windows_subsystem = "windows")]

use clap::Parser;

use chatrans::runner::{self, ClientArgs};

fn main() {
    // 带参数（-r / --headless / --help …）= 命令行用法：附着到用户自己的
    // 控制台再初始化日志，输出照常显示在终端里。无参数（双击）= GUI，
    // 一个控制台窗口都不留（release 下已是 windows 子系统）。
    if std::env::args().len() > 1 {
        chatrans::logging::attach_parent_console();
    }
    chatrans::logging::init();

    // 双击启动（无任何参数）：gui feature 下直接进 GUI，绝不经过 clap。
    #[cfg(feature = "gui")]
    if std::env::args().len() == 1 {
        chatrans::gui::run_app();
        return;
    }

    let client = ClientArgs::parse();

    // GUI 模式：eframe 占主线程，后台管线在 start_with_dir 里已起好，
    // 面板退出时经 App::on_exit 关停。
    #[cfg(feature = "gui")]
    {
        if !client.headless {
            chatrans::gui::run_app_with_args(client);
            return;
        }
    }

    runner::run(client);
}
