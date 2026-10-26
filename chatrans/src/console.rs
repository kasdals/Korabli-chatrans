//! console 模式的事件出口（S5）。
//!
//! Web/SSE 移除后，控制台模式需要一个新的打印出口：消费 runner 分流出来的
//! `gui_events` 通道，把聊天原文 `println!` 到标准输出 —— 只打印原文，
//! 不做翻译（翻译能力归 GUI 面板，Interpreter 模块保留不删）。
//!
//! 通道是 async_channel：后台线程用 `recv_blocking` 消费，零 tokio 依赖。

use std::thread::JoinHandle;

use crate::processor::{BattleInfo, ChatMessage, LiveEvent};

/// 单行聊天的控制台格式：`[友] 名字·舰船: 原文`。判定不了阵营时不带徽标。
fn format_chat(msg: &ChatMessage) -> String {
    let badge = match msg.allied {
        Some(true) => "[友] ",
        Some(false) => "[敌] ",
        None => "",
    };
    match msg.ship.as_deref() {
        Some(ship) if !ship.trim().is_empty() => {
            format!("{}{}·{}: {}", badge, msg.sender, ship, msg.message)
        }
        _ => format!("{}{}: {}", badge, msg.sender, msg.message),
    }
}

/// 开局提示的控制台格式。字段缺失（解析失败）时给通用文案，不影响主流程。
fn format_battle(info: &BattleInfo) -> String {
    if info.map.is_empty() {
        "── 新对局开始 ──".to_string()
    } else {
        format!("── 新对局开始：{} ──", info.map)
    }
}

/// 起一个打印器线程消费事件通道，通道关闭（生产端全部退出）后线程结束。
pub fn spawn_printer(rx: async_channel::Receiver<LiveEvent>) -> JoinHandle<()> {
    std::thread::Builder::new()
        .name("console-printer".into())
        .spawn(move || loop {
            match rx.recv_blocking() {
                Ok(LiveEvent::BattleStart(info)) => println!("{}", format_battle(&info)),
                Ok(LiveEvent::Chat(msg)) => println!("{}", format_chat(&msg)),
                Err(_) => break,
            }
        })
        .expect("spawn console printer")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chat(allied: Option<bool>, ship: Option<&str>, sender: &str, text: &str) -> ChatMessage {
        ChatMessage {
            clock: 0.0,
            sender: sender.to_string(),
            audience: "battle_team".to_string(),
            message: text.to_string(),
            team: None,
            allied,
            ship: ship.map(str::to_string),
        }
    }

    #[test]
    fn chat_line_format_variants() {
        assert_eq!(
            format_chat(&chat(Some(true), Some("Minotaur"), "Alice", "gg wp")),
            "[友] Alice·Minotaur: gg wp"
        );
        assert_eq!(
            format_chat(&chat(Some(false), None, "Bob", "hello")),
            "[敌] Bob: hello"
        );
        assert_eq!(
            format_chat(&chat(None, None, "Carol", "привет")),
            "Carol: привет"
        );
        // 空白舰船名等同无舰船
        assert_eq!(
            format_chat(&chat(Some(true), Some("  "), "Dan", "hi")),
            "[友] Dan: hi"
        );
    }

    #[test]
    fn battle_line_format_variants() {
        assert_eq!(format_battle(&BattleInfo::default()), "── 新对局开始 ──");
        let mut info = BattleInfo::default();
        info.map = "北方海峡".to_string();
        assert_eq!(format_battle(&info), "── 新对局开始：北方海峡 ──");
    }
}
