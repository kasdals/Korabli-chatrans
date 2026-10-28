//! 舰船 ID → 显示名。
//!
//! 表是从游戏的 `GameParams.data` 导出的（`chatrans/ship_ids.tsv`），编译期嵌进二进制，
//! 所以运行时不需要任何外部文件。玩家记录里的字段 38（26.10 是 39）就是这张表的 key
//! （实测 `4179539920 → Minotaur`，与本机玩家在 `tempArenaInfo.json` 里的
//! `playerVehicle: PBSC110-Minotaur` 一致）。
//!
//! 另外一张表 `ship_index.tsv` 解决**反方向**的查询：`tempArenaInfo.json` 里的
//! `playerVehicle` 是形如 `PASC710-Salem` 的**配置名**，不是数字 ID。它由
//! 「脚本 index + 英文名」拼成，而 index 是唯一的，所以取第一个 `-` 之前的部分
//! 就能查到数字 ID，再走上面那张表拿到中文名。
//!
//! 第三张表 `ship_names.tsv` 直接是 `index → 中文名`，数据来自
//! [DDFantasyV/Korabli_localization_chs](https://github.com/DDFantasyV/Korabli_localization_chs)
//! 的 `global.mo`（`IDS_<index>_FULL`）。有了它，**还没有数字 ID 的新船**也能显示中文名 ——
//! 因为配置名查询不必再绕道 `ship_index.tsv`。查不到时才回退到 index → id → name。

use std::collections::HashMap;
use std::sync::OnceLock;

const SHIP_IDS: &str = include_str!("../ship_ids.tsv");
const SHIP_INDEX: &str = include_str!("../ship_index.tsv");
const SHIP_NAMES: &str = include_str!("../ship_names.tsv");

fn table() -> &'static HashMap<u32, &'static str> {
    static TABLE: OnceLock<HashMap<u32, &'static str>> = OnceLock::new();
    TABLE.get_or_init(|| {
        SHIP_IDS
            .lines()
            .filter(|line| !line.starts_with('#') && !line.trim().is_empty())
            .filter_map(|line| {
                let (id, name) = line.split_once('\t')?;
                Some((id.trim().parse::<u32>().ok()?, name.trim()))
            })
            .collect()
    })
}

fn index_table() -> &'static HashMap<&'static str, u32> {
    static TABLE: OnceLock<HashMap<&'static str, u32>> = OnceLock::new();
    TABLE.get_or_init(|| {
        SHIP_INDEX
            .lines()
            .filter(|line| !line.starts_with('#') && !line.trim().is_empty())
            .filter_map(|line| {
                let (index, id) = line.split_once('\t')?;
                Some((index.trim(), id.trim().parse::<u32>().ok()?))
            })
            .collect()
    })
}

fn names_table() -> &'static HashMap<&'static str, &'static str> {
    static TABLE: OnceLock<HashMap<&'static str, &'static str>> = OnceLock::new();
    TABLE.get_or_init(|| {
        SHIP_NAMES
            .lines()
            .filter(|line| !line.starts_with('#') && !line.trim().is_empty())
            .filter_map(|line| {
                let (index, name) = line.split_once('\t')?;
                Some((index.trim(), name.trim()))
            })
            .collect()
    })
}

/// 查舰船显示名。查不到（或 id 为 0）返回 `None`，调用方就不显示船名。
pub fn display_name(ship_id: u32) -> Option<&'static str> {
    if ship_id == 0 {
        return None;
    }
    table().get(&ship_id).copied()
}

/// 从 `tempArenaInfo.json` 的 `playerVehicle`（如 `PASC710-Salem`）查显示名。
///
/// 取第一个 `-`（或 `_`）之前的部分作为 index。查不到返回 `None`，
/// 调用方应原样显示配置名 —— **宁可显示 `PASC710-Salem`，也不要显示空白或错的船名**。
pub fn display_name_from_config(config: &str) -> Option<&'static str> {
    let index = config
        .split(['-', '_'])
        .next()
        .map(str::trim)
        .filter(|s| !s.is_empty())?;

    // 先按 index 直接查名字：覆盖最全，新船即使还没有数字 ID 也能显示中文名
    if let Some(name) = names_table().get(index).copied() {
        return Some(name);
    }
    // 回退到 index → 数字 ID → 名字，兜住名字表里没有的老条目
    display_name(*index_table().get(index)?)
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn has_chinese_name() {
        // PBSC110_Minotaur —— 有官方中文名
        assert_eq!(display_name(4179539920), Some("米诺陶"));
    }

    #[test]
    fn falls_back_to_english() {
        // PWSC110_Konung —— 本地化表里没有中文名，保留英文
        assert_eq!(display_name(4179539248), Some("Konung"));
    }

    #[test]
    fn unknown_id_is_none() {
        assert_eq!(display_name(0), None);
        assert_eq!(display_name(1), None);
    }

    #[test]
    fn resolves_config_name() {
        // 26.10 实测：tempArenaInfo.json 里 playerVehicle = "PASC710-Salem"
        assert_eq!(display_name_from_config("PASC710-Salem"), Some("塞勒姆"));
        // 配置名的另一种写法（下划线）也要认
        assert_eq!(display_name_from_config("PBSC110_Minotaur"), Some("米诺陶"));
        // 只有 index、没有后缀时也能查
        assert_eq!(display_name_from_config("PBSC110"), Some("米诺陶"));
    }

    #[test]
    fn unknown_config_name_is_none() {
        assert_eq!(display_name_from_config(""), None);
        assert_eq!(display_name_from_config("NOPE999-Whatever"), None);
    }

    #[test]
    fn new_ship_without_numeric_id_shows_chinese() {
        // PFSC211（拉罗谢尔）、PASB529（北达科他）是 26.11 的新船：
        // ship_index.tsv 里还没有它们的数字 ID，只能靠 ship_names.tsv 命中
        assert_eq!(
            display_name_from_config("PFSC211-La_Rochelle"),
            Some("拉罗谢尔")
        );
        assert_eq!(
            display_name_from_config("PASB529-North_Dakota"),
            Some("北达科他")
        );
    }

    #[test]
    fn names_table_covers_ships_missing_from_index() {
        // 这两条目前确实不在 index 表里 —— 如果哪天真补上了数字 ID，
        // 这个断言会失败，提醒把 ship_names.tsv 的位置重新理一遍
        assert!(names_table().contains_key("PFSC211"));
        assert!(!index_table().contains_key("PFSC211"));
    }
}
