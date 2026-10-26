//! 面板行模型与容量管理（移植自雷达 `ChatLine.cs` / `ChatStream.cs` / `OverlayWindow` 渲染语义）。
//!
//! 纯逻辑、零 egui 依赖 —— 单元测试直接覆盖渲染前的最后一层。
//!
//! 与雷达对齐的关键语义：
//!   · 容量上限 [`PANEL_CAPACITY`]（雷达 RealChatSource 默认 10），挤掉最旧的；
//!   · 系统行（连接提示/开局提示）**不进消息区**，由状态行表达；
//!   · 「清空」只清显示不断数据：记住最后一条 id，之后只显示它之后的；
//!   · 「本局 N 条」按收到的聊天行计数（系统行不计，开局清零）。

use std::collections::VecDeque;

/// 消息区容量上限（与雷达 RealChatSource.Capacity 一致）。
pub const PANEL_CAPACITY: usize = 10;

/// 行的阵营归属（决定左侧 `[友]`/`[敌]` 标签配色）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Team {
    Allied,
    Enemy,
    Unknown,
}

/// 消息区里的一行。不可变字段直接存；译文回填时整体替换
/// （雷达用 record `with`，这里用 `&mut` 一次性赋值，渲染每帧全量重画，
/// 不存在半更新状态）。
#[derive(Debug, Clone)]
pub struct ChatRow {
    pub id: u64,
    pub team: Team,
    pub name: String,
    pub ship: Option<String>,
    /// 原文。翻译请求、缓存键、原文小字都用它。
    pub text: String,
    /// 面板自己的译文（coordinator 回填）。事件自带 translated 非空时直接占位。
    pub translated: Option<String>,
    /// 是否队伍频道（TeamOnly 过滤用；audience 判定不了时视为 true，不误伤）。
    pub is_team_channel: bool,
}

impl ChatRow {
    /// 主行显示文本：有译文用译文，没有用原文。
    pub fn display_text(&self) -> &str {
        self.translated.as_deref().unwrap_or(&self.text)
    }

    /// 第一行：`[友] 名字·舰船: 译文`（照抄雷达 `ChatLine.Headline`）。
    pub fn headline(&self) -> String {
        match self.headline_parts() {
            (Some((badge, _)), body) => format!("{}{}", badge, body),
            (None, body) => body,
        }
    }

    /// 拆开的标题：(徽标, "名字·舰船: 显示文本")。渲染时徽标要单独配色。
    pub fn headline_parts(&self) -> (Option<(&'static str, Team)>, String) {
        let badge = match self.team {
            Team::Allied => Some(("[友] ", Team::Allied)),
            Team::Enemy => Some(("[敌] ", Team::Enemy)),
            Team::Unknown => None,
        };
        (badge, format!("{}: {}", self.who(), self.display_text()))
    }

    /// `名字·舰船`（无舰船或空舰船时只有名字）。
    pub fn who(&self) -> String {
        match self.ship.as_deref() {
            Some(ship) if !ship.is_empty() => format!("{}·{}", self.name, ship),
            _ => self.name.clone(),
        }
    }

    /// 第二行要不要显示：与译文相同（无需翻译）就省略（照抄雷达 `ChatLine.OriginalLine`）。
    pub fn original_line(&self) -> Option<&str> {
        if self.text.trim().is_empty() {
            return None;
        }
        if self.text.trim() == self.display_text().trim() {
            return None;
        }
        Some(&self.text)
    }

    /// 这一行还需要面板去翻译吗（系统行在上游就不入列，无需再判）。
    pub fn needs_translation(&self) -> bool {
        self.translated.is_none() && !self.text.trim().is_empty()
    }
}

/// 消息区状态。
pub struct PanelState {
    rows: VecDeque<ChatRow>,
    next_id: u64,
    /// 本局收到的聊天条数（系统行不计；开局清零）。状态行「本局 N 条」用。
    chat_count: u64,
    /// 「清空」标记：只显示 id 大于它的行（找不到——被容量挤掉——就从头显示）。
    clear_mark: Option<u64>,
}

impl Default for PanelState {
    fn default() -> Self {
        PanelState::new()
    }
}

impl PanelState {
    pub fn new() -> PanelState {
        PanelState {
            rows: VecDeque::new(),
            next_id: 1,
            chat_count: 0,
            clear_mark: None,
        }
    }

    /// 收到一行聊天。`allied: Option<bool>` 与雷达 SSE 契约一致：None = 判定不了。
    /// `translated` 非空时直接采用（不重复翻）。
    pub fn push_chat(
        &mut self,
        name: &str,
        ship: Option<String>,
        allied: Option<bool>,
        audience: &str,
        text: &str,
        translated: Option<String>,
    ) {
        let team = match allied {
            Some(true) => Team::Allied,
            Some(false) => Team::Enemy,
            None => Team::Unknown,
        };
        // 频道判定：audience 里带 "team" 才算队伍频道；空的/判定不了 → 视为 true，
        // 宁可多翻也不漏翻（与雷达"判定不了不画竖条但不丢消息"同一取舍方向）。
        let lowered = audience.to_ascii_lowercase();
        let is_team_channel = lowered.is_empty() || lowered.contains("team");

        self.rows.push_back(ChatRow {
            id: self.next_id,
            team,
            name: name.to_string(),
            ship,
            text: text.to_string(),
            translated,
            is_team_channel,
        });
        self.next_id += 1;
        self.chat_count += 1;
        while self.rows.len() > PANEL_CAPACITY {
            self.rows.pop_front();
        }
    }

    /// 新对局开始：清空上一局的行、计数与清空标记（照抄雷达 OnBattle）。
    pub fn start_battle(&mut self) {
        self.rows.clear();
        self.chat_count = 0;
        self.clear_mark = None;
    }

    /// 「清空」按钮：只清显示，不断数据。之后只显示当前最后一行之后的内容。
    pub fn clear_display(&mut self) {
        self.clear_mark = self.rows.back().map(|r| r.id);
    }

    /// 可见行（应用清空标记之后）。标记行被容量挤掉时从头显示 ——
    /// 永久空白比"清空失效"更让人困惑（雷达注释原话）。
    pub fn visible_rows(&self) -> Vec<&ChatRow> {
        match self.clear_mark {
            Some(mark) => self
                .rows
                .iter()
                .filter(|r| r.id > mark)
                .collect(),
            None => self.rows.iter().collect(),
        }
    }

    /// 本局聊天条数（状态行用）。
    pub fn chat_count(&self) -> u64 {
        self.chat_count
    }

    /// 按原文回填译文，返回更新的行数。同一原文的多行一起更新（缓存语义）。
    pub fn set_translated(&mut self, text: &str, translated: Option<String>) -> usize {
        let mut updated = 0;
        for row in self.rows.iter_mut() {
            if row.text == text && row.translated.is_none() {
                row.translated = translated.clone();
                updated += 1;
            }
        }
        updated
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state_with_lines() -> PanelState {
        let mut state = PanelState::new();
        state.push_chat("Alice", Some("Minotaur".into()), Some(true), "battle_team", "gg wp", None);
        state.push_chat("Bob", None, Some(false), "battle_all", "hello", None);
        state.push_chat("Carol", None, None, "", "привет", None);
        state
    }

    #[test]
    fn headline_format() {
        let state = state_with_lines();
        let rows: Vec<_> = state.visible_rows();
        assert_eq!(rows[0].headline(), "[友] Alice·Minotaur: gg wp");
        assert_eq!(rows[1].headline(), "[敌] Bob: hello");
        assert_eq!(rows[2].headline(), "Carol: привет"); // 判定不了 → 无标签、不丢消息
    }

    #[test]
    fn original_line_rules() {
        let mut state = PanelState::new();
        state.push_chat("A", None, None, "", "hello", None);
        state.push_chat("B", None, None, "", "hello", Some("hello".into())); // 译文==原文
        state.push_chat("C", None, None, "", "hello", Some("你好".into()));
        state.push_chat("D", None, None, "", "  ", None); // 空白原文
        let rows: Vec<_> = state.visible_rows();
        assert_eq!(rows[0].original_line(), None, "没有译文 → 只显示原文一行");
        assert_eq!(rows[1].original_line(), None, "译文==原文 → 不显示第二行");
        assert_eq!(rows[2].original_line(), Some("hello"));
        assert_eq!(rows[3].original_line(), None);
    }

    #[test]
    fn capacity_trims_oldest() {
        let mut state = PanelState::new();
        for i in 0..(PANEL_CAPACITY + 5) {
            state.push_chat(&format!("n{}", i), None, None, "", &format!("m{}", i), None);
        }
        let rows: Vec<_> = state.visible_rows();
        assert_eq!(rows.len(), PANEL_CAPACITY);
        assert_eq!(rows[0].text, "m5", "最旧的 5 条被挤掉");
        assert_eq!(rows.last().unwrap().text, format!("m{}", PANEL_CAPACITY + 4));
    }

    #[test]
    fn clear_display_only_hides() {
        let mut state = state_with_lines();
        let count_before = state.chat_count();
        state.clear_display();
        assert!(state.visible_rows().is_empty(), "清空后消息区为空");
        assert_eq!(state.chat_count(), count_before, "清空不影响计数");
        // 新消息照常显示
        state.push_chat("D", None, None, "", "new", None);
        assert_eq!(state.visible_rows().len(), 1);
    }

    #[test]
    fn clear_mark_evicted_falls_back_to_all() {
        let mut state = PanelState::new();
        for i in 0..3 {
            state.push_chat("n", None, None, "", &format!("m{}", i), None);
        }
        state.clear_display(); // 标记指向 id=3
        for i in 0..PANEL_CAPACITY {
            state.push_chat("n", None, None, "", &format!("new{}", i), None);
        }
        // 标记 id=3 已被容量挤掉 → 从头显示全部（不会永久空白）
        let rows: Vec<_> = state.visible_rows();
        assert_eq!(rows.len(), PANEL_CAPACITY);
        assert_eq!(rows[0].text, "new0", "被挤掉的标记意味着全部回退为可见");
    }

    #[test]
    fn battle_start_resets_everything() {
        let mut state = state_with_lines();
        state.clear_display();
        state.start_battle();
        assert!(state.visible_rows().is_empty());
        assert_eq!(state.chat_count(), 0);
        state.push_chat("A", None, None, "", "hi", None);
        assert_eq!(state.visible_rows().len(), 1, "清空标记也被重置");
    }

    #[test]
    fn set_translated_updates_all_matching_rows() {
        let mut state = PanelState::new();
        state.push_chat("A", None, None, "", "dup", None);
        state.push_chat("B", None, None, "", "dup", None);
        state.push_chat("C", None, None, "", "dup", Some("已翻".into()));
        let updated = state.set_translated("dup", Some("重复".into()));
        assert_eq!(updated, 2, "已有译文的不覆盖");
        let rows: Vec<_> = state.visible_rows();
        assert_eq!(rows[0].display_text(), "重复");
        assert_eq!(rows[2].display_text(), "已翻");
    }

    #[test]
    fn team_channel_detection() {
        let mut state = PanelState::new();
        state.push_chat("A", None, None, "battle_team", "a", None);
        state.push_chat("B", None, None, "Battle_All", "b", None);
        state.push_chat("C", None, None, "", "c", None);
        let rows: Vec<_> = state.visible_rows();
        assert!(rows[0].is_team_channel);
        assert!(!rows[1].is_team_channel);
        assert!(rows[2].is_team_channel, "判定不了 → 视为队伍频道，不误伤");
    }
}
