use replay_parser::analyzer::decoder::{DecodedPacket, DecodedPacketPayload};
use replay_parser::analyzer::Analyzer;
use replay_parser::packet2::Packet;
use std::{
    collections::HashMap,
    convert::TryInto,
    sync::{Arc, Mutex},
};
use async_channel::Sender;
use tracing::debug;

use crate::ships;

#[derive(Debug, Clone)]
pub struct ChatMessage {
    pub clock: f32,
    pub sender: String,
    pub audience: String,
    pub message: String,
    /// 发送者所在队伍（0/1），拿不到就是 None
    pub team: Option<u8>,
    /// 是否与我在同一队。None = 判定不了（没拿到玩家列表，或不知道自己在哪队）
    pub allied: Option<bool>,
    /// 发送者的舰船显示名（如 "Minotaur"）
    pub ship: Option<String>,
}

/// 从 `tempArenaInfo.json` 读出来的对局信息。
#[derive(Debug, Clone, Default)]
pub struct BattleInfo {
    pub map: String,
    pub vehicle: String,
    pub player: String,
    pub started_at: String,
}

/// 后台线程发给服务器的事件。
///
/// 不只是聊天：还需要能告诉客户端"新的一局开始了"。少了这个，用户在等下一局
/// 的时候网页上什么都不会变，很容易误判成程序挂了 —— 之前聊天被静默丢弃那次
/// 就是这个观感。
#[derive(Debug, Clone)]
pub enum LiveEvent {
    Chat(ChatMessage),
    BattleStart(BattleInfo),
}

/// 玩家列表里我关心的一行。
#[derive(Debug, Clone)]
struct PlayerEntry {
    team: u8,
    ship: Option<String>,
}

/// 本机玩家名。由 `LiveMonitor` 在读到 `tempArenaInfo.json` 时写入，
/// 用来判断"哪一队是我方"，从而把聊天标成友/敌。
pub type MyName = Arc<Mutex<Option<String>>>;

pub struct ChatLoggerBuilder;

impl ChatLoggerBuilder {
    pub fn new() -> ChatLoggerBuilder {
        ChatLoggerBuilder
    }

    pub fn build(
        &self,
        tx: Sender<LiveEvent>,
        my_name: MyName,
    ) -> Box<dyn Analyzer> {
        Box::new(ChatLogger {
            usernames: HashMap::new(),
            players: HashMap::new(),
            my_team: None,
            my_name,
            tx,
        })
    }
}

pub struct ChatLogger {
    /// playerid -> 昵称
    usernames: HashMap<i32, String>,
    /// playerid -> (队伍, 舰船)
    players: HashMap<i32, PlayerEntry>,
    my_team: Option<u8>,
    my_name: MyName,
    tx: Sender<LiveEvent>,
}

impl ChatLogger {
    /// 从玩家列表里找出自己那条记录，记下我方队伍。
    fn resolve_my_team(&mut self) {
        if self.my_team.is_some() {
            return;
        }
        let my = match self.my_name.lock().unwrap().clone() {
            Some(name) if !name.is_empty() => name,
            _ => return,
        };
        for (id, name) in self.usernames.iter() {
            if name == &my {
                self.my_team = self.players.get(id).map(|p| p.team);
                debug!("My team resolved from player list: {:?}", self.my_team);
                return;
            }
        }
        debug!("Cannot find myself ({:?}) in the player list", my);
    }
}

impl Analyzer for ChatLogger {
    fn finish(&self) {}

    fn process(&mut self, packet: &Packet<'_, '_>) {
        let decoded = DecodedPacket::from(false, packet);
        match decoded.payload {
            DecodedPacketPayload::Chat {
                sender_id,
                audience,
                message,
                ..
            } => {
                // 昵称映射来自玩家列表。拿不到名字就用 #sender_id 兜底 ——
                // 展示聊天本身不该依赖队伍/名字解析成功。
                let sender = match self.usernames.get(&sender_id) {
                    Some(name) => name.clone(),
                    None => {
                        debug!(
                            "Unknown sender_id {}, falling back to placeholder (known players: {})",
                            sender_id,
                            self.usernames.len()
                        );
                        format!("#{}", sender_id)
                    }
                };
                let entry = self.players.get(&sender_id);
                let team = entry.map(|p| p.team);
                let allied = match (team, self.my_team) {
                    (Some(t), Some(mine)) => Some(t == mine),
                    _ => None,
                };
                let ship = entry.and_then(|p| p.ship.clone());
                debug!(
                    "{}: {} (team={:?} allied={:?} ship={:?}): {} {}",
                    decoded.clock, sender, team, allied, ship, audience, message
                );
                let _ = self.tx.send_blocking(LiveEvent::Chat(ChatMessage {
                    clock: decoded.clock,
                    sender,
                    audience: audience.to_string(),
                    message: message.to_string(),
                    team,
                    allied,
                    ship,
                }));
            }
            DecodedPacketPayload::EntityInfo { players, .. } => {
                for player in players.iter() {
                    let id: i32 = match player.playerid.try_into() {
                        Ok(id) => id,
                        Err(_) => continue,
                    };
                    let team = player.teamid.clamp(0, u8::MAX as i64) as u8;
                    let ship =
                        ships::display_name(player.shipid.clamp(0, u32::MAX as i64) as u32)
                            .map(|s| s.to_string());
                    self.usernames.insert(id, player.username.clone());
                    self.players.insert(id, PlayerEntry { team, ship });
                }
                debug!(
                    "Player list updated: {} player(s), my team so far = {:?}",
                    self.usernames.len(),
                    self.my_team
                );
                self.resolve_my_team();
            }
            _ => {}
        }
    }
}
