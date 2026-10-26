//! 26.9 / 26.10 客户端的玩家信息载荷解析。
//!
//! 老版本里这个包（类型 0x1D）是一串「属性 id(u32 LE) + msg_type(u8) + 裸数据」，
//! [`crate::packet2`] 靠固定标记 `[40, 2, 0, 0, 0, 105]` 切分。26.9 改成了**载荷里嵌
//! zlib 流**，解开后是 BigWorld RPC 字段流（`0x92 <字段号> <变长值>`）。老代码找不到
//! 标记，于是解出 0 个实体 —— 这就是聊天里只能显示 `#数字ID` 的根因。
//!
//! 这里把 zlib 流解开，按字段号抽取玩家记录。字段号偏移**随版本变**：
//!
//! ```text
//! 包内字段号 = ReplayPlayerProperty 的数值 + 字段偏移
//! 26.9  -> 偏移 2
//! 26.10 -> 偏移 3   （Lesta 在属性表中间又插了一条，后面全体后移一位）
//! ```
//!
//! 26.10 实测（用「本机玩家自己在 `tempArenaInfo.json` 里的信息」当预言式真值校验）：
//! `Id(12)→15`、`Name(27)→30`、`Realm(32)→35`、`ShipParamsId(36)→39`、
//! `SkinId(37)→40`、`TeamId(38)→41`。两个独立信号互相印证：
//! 字段 39 = `3550394352` → 舰船表里是「塞勒姆」，而 `playerVehicle` 正是 `PASC710-Salem`；
//! 字段 15 = `134251365` → 与聊天包里的 `senderId` 完全一致（这就是名字能对上说话人的关键）。
//!
//! 偏移不再是写死的常量：先试已知偏移，都不行再扫描一遍，见 [`FIELD_OFFSETS`] 与
//! [`parse_records`]。这样下次 Lesta 再插属性也不用改代码。
//!
//! 记录之间按「字段出现顺序」对齐（第 i 个名字属于第 i 个 ID）。这一点用独立信号
//! 验证过：本机玩家那条记录的名字、舰船 ID 与占用同一序号的 `tempArenaInfo.json`
//! 字段同时吻合。

use std::io::Read;

use serde_derive::Serialize;

/// 已知的字段偏移，**新的排在前面**（先按当前版本试）。
const FIELD_OFFSETS: [u8; 2] = [3, 2];

/// 兜底扫描范围：已知偏移都不匹配时，逐个试 0..=MAX_SCAN_OFFSET。
const MAX_SCAN_OFFSET: u8 = 6;

/// `ReplayPlayerProperty` 的数值（与 `decoder.rs` 里的枚举一致，这里只需要用到的几个）。
const P_ID: u8 = 12;
const P_NAME: u8 = 27;
const P_REALM: u8 = 32;
const P_SHIP_ID: u8 = 36;
const P_SKIN_ID: u8 = 37;
const P_TEAM: u8 = 38;

/// 单个 zlib 流解压后的上限，防止解出天文数字的垃圾数据。
const MAX_INFLATED: u64 = 1 << 22;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FieldValue {
    Int(u32),
    Str(String),
}

/// 一名玩家在列表里的信息。
#[derive(Debug, Clone, Serialize)]
pub struct PlayerRecord {
    /// 账号 ID —— 与聊天包里的 `senderId` 是同一个值域（已用真值验证）
    pub account_id: u32,
    pub name: String,
    pub team: u8,
    pub ship_id: u32,
    pub skin_id: u32,
    pub realm: String,
}

/// 尝试从载荷里解析出玩家列表。解析不出来返回 `None`，调用方应回退到老逻辑。
pub fn parse(payload: &[u8]) -> Option<Vec<PlayerRecord>> {
    for data in inflate_candidates(payload) {
        // 先按已知偏移试（当前版本排最前），都不匹配再全局扫一遍。
        let scanned: Vec<u8> = (0..=MAX_SCAN_OFFSET).collect();
        for &offset in FIELD_OFFSETS.iter().chain(scanned.iter()) {
            if let Some(list) = parse_records(&data, offset) {
                tracing::debug!(
                    "Player list parsed with field offset +{}: {} player(s)",
                    offset,
                    list.len()
                );
                return Some(list);
            }
        }
    }
    None
}

/// 载荷里可能嵌了不止一个 zlib 流，而且**不一定从偏移 0 开始**
/// （实测实时包的 zlib 头前有十几个字节前缀），所以扫一遍全部试。
fn inflate_candidates(payload: &[u8]) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    if payload.len() < 3 {
        return out;
    }
    for i in 0..payload.len() - 2 {
        if payload[i] != 0x78 {
            continue;
        }
        // zlib 头：CMF=0x78，FLG 的低 5 位是校验位，合法取值就这几个
        if !matches!(payload[i + 1], 0x01 | 0x5e | 0x9c | 0xda) {
            continue;
        }
        let mut decoder = flate2::read::ZlibDecoder::new(&payload[i..]);
        let mut buf = Vec::new();
        if decoder.by_ref().take(MAX_INFLATED).read_to_end(&mut buf).is_ok() && buf.len() >= 64 {
            out.push(buf);
        }
    }
    out
}

/// 解析一条 RPC 值。`data` 从值本身开始（即 `0x92 <字段号>` 之后）。
/// 返回 (值, 占用的字节数)。
fn decode_value(data: &[u8]) -> Option<(FieldValue, usize)> {
    let tag = *data.first()?;
    match tag {
        // 小整数直接用标记本身表示
        0x00..=0x7f => Some((FieldValue::Int(tag as u32), 1)),
        // 短字符串：0xa0 + 长度
        0xa0..=0xbf => {
            let n = (tag - 0xa0) as usize;
            let bytes = data.get(1..1 + n)?;
            Some((
                FieldValue::Str(String::from_utf8_lossy(bytes).into_owned()),
                1 + n,
            ))
        }
        // 变长整数，**大端**（实测：cd 29 d0 = 10704）
        0xcc => Some((FieldValue::Int(*data.get(1)? as u32), 2)),
        0xcd => {
            let b = data.get(1..3)?;
            Some((FieldValue::Int(u16::from_be_bytes([b[0], b[1]]) as u32), 3))
        }
        0xce => {
            let b = data.get(1..5)?;
            Some((
                FieldValue::Int(u32::from_be_bytes([b[0], b[1], b[2], b[3]])),
                5,
            ))
        }
        0xc2 => Some((FieldValue::Int(0), 1)),
        0xc3 => Some((FieldValue::Int(1), 1)),
        _ => None,
    }
}

/// 抽出某个字段的全部取值（按出现顺序）。
///
/// 关键约束：**值后面必须紧跟下一个字段标记 `0x92`**（记录末尾是块尾）。
/// 有了这条，字符串/容器内容里的巧合字节几乎不会被误判成字段。
fn scan_field(data: &[u8], field: u8) -> Vec<FieldValue> {
    let mut out = Vec::new();
    if data.len() < 4 {
        return out;
    }
    for i in 0..data.len() - 3 {
        if data[i] != 0x92 || data[i + 1] != field {
            continue;
        }
        let Some((value, len)) = decode_value(&data[i + 2..]) else {
            continue;
        };
        let next = i + 2 + len;
        if next >= data.len() || data[next] == 0x92 {
            out.push(value);
        }
    }
    out
}

fn scan_u32s(data: &[u8], field: u8) -> Vec<u32> {
    scan_field(data, field)
        .into_iter()
        .filter_map(|v| match v {
            FieldValue::Int(i) => Some(i),
            FieldValue::Str(_) => None,
        })
        .collect()
}

fn scan_strs(data: &[u8], field: u8) -> Vec<String> {
    scan_field(data, field)
        .into_iter()
        .filter_map(|v| match v {
            FieldValue::Str(s) => Some(s),
            FieldValue::Int(_) => None,
        })
        .collect()
}

/// 按指定字段偏移抽一遍玩家记录。
///
/// 三条（名字 / 账号 ID / 队伍）必须都齐全且数量一致才认，否则返回 `None` ——
/// 偏移猜错时这三个字段几乎不可能同时自洽。
fn parse_records(data: &[u8], offset: u8) -> Option<Vec<PlayerRecord>> {
    let names = scan_strs(data, P_NAME + offset);
    let ids = scan_u32s(data, P_ID + offset);
    let teams = scan_u32s(data, P_TEAM + offset);

    let n = names.len();
    if n == 0 || ids.len() != n || teams.len() != n {
        return None;
    }

    let ships = scan_u32s(data, P_SHIP_ID + offset);
    let skins = scan_u32s(data, P_SKIN_ID + offset);
    let realms = scan_strs(data, P_REALM + offset);

    let mut records = Vec::with_capacity(n);
    for k in 0..n {
        records.push(PlayerRecord {
            account_id: ids[k],
            name: names[k].clone(),
            team: teams[k].min(u8::MAX as u32) as u8,
            ship_id: ships.get(k).copied().unwrap_or_default(),
            skin_id: skins.get(k).copied().unwrap_or_default(),
            realm: realms.get(k).cloned().unwrap_or_default(),
        });
    }
    Some(records)
}

#[cfg(test)]
mod test {
    use super::*;
    use std::io::Write;

    fn zlib(data: &[u8]) -> Vec<u8> {
        let mut e = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        e.write_all(data).unwrap();
        e.finish().unwrap()
    }

    fn put_int(field: u8, v: u32, out: &mut Vec<u8>) {
        out.extend([0x92, field, 0xce]);
        out.extend(v.to_be_bytes());
    }

    fn put_str(field: u8, s: &str, out: &mut Vec<u8>) {
        assert!(s.len() < 32, "测试数据只覆盖 fixstr");
        out.extend([0x92, field, 0xa0 + s.len() as u8]);
        out.extend(s.as_bytes());
    }

    fn put_record(offset: u8, id: u32, name: &str, team: u32, ship: u32, out: &mut Vec<u8>) {
        put_int(P_ID + offset, id, out);
        put_str(P_NAME + offset, name, out);
        put_str(P_REALM + offset, "RU", out);
        put_int(P_SHIP_ID + offset, ship, out);
        put_int(P_SKIN_ID + offset, ship, out);
        put_int(P_TEAM + offset, team, out);
    }

    /// 26.10：字段偏移 +3
    #[test]
    fn parses_offset_three() {
        let mut raw = Vec::new();
        put_record(3, 134_251_365, "Podvodnik_Rus_1984", 0, 3_550_394_352, &mut raw);
        put_record(3, 10_704, "KPOH54", 1, 4_179_539_920, &mut raw);

        let list = parse(&zlib(&raw)).expect("偏移 +3 应该解出");
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].account_id, 134_251_365);
        assert_eq!(list[0].name, "Podvodnik_Rus_1984");
        assert_eq!(list[0].team, 0);
        assert_eq!(list[0].ship_id, 3_550_394_352);
        assert_eq!(list[0].realm, "RU");
        assert_eq!(list[1].name, "KPOH54");
        assert_eq!(list[1].team, 1);
    }

    /// 26.9：字段偏移 +2（老版本必须继续能解）
    #[test]
    fn parses_offset_two() {
        let mut raw = Vec::new();
        put_record(2, 134_451_334, "NetbuZ", 0, 4_179_539_920, &mut raw);
        // 两条记录：inflate_candidates 会丢弃解出 <64 字节的流，样本得够长
        put_record(2, 10_704, "KPOH54", 1, 4_179_539_248, &mut raw);

        let list = parse(&zlib(&raw)).expect("偏移 +2 应该解出");
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].name, "NetbuZ");
        assert_eq!(list[0].account_id, 134_451_334);
        assert_eq!(list[1].name, "KPOH54");
        assert_eq!(list[1].team, 1);
        // 关键：+3 的误匹配不能把 +2 的数据解成别的东西
        assert!(list.iter().all(|p| p.realm == "RU"));
    }

    /// 26.10 里「本机玩家自己的属性包」也带 0x92 字段流，但它不是名册，必须拒绝，
    /// 否则会把属性当成玩家名显示出去。
    #[test]
    fn rejects_non_roster_payload() {
        let mut raw = Vec::new();
        put_int(1, 1, &mut raw);
        put_str(8, "NEKOM", &mut raw);
        put_int(28, 73, &mut raw);
        put_int(29, 55_700, &mut raw);
        raw.extend(std::iter::repeat(b'x').take(80));
        assert!(parse(&zlib(&raw)).is_none());
    }

    #[test]
    fn rejects_empty_and_garbage() {
        assert!(parse(&[]).is_none());
        assert!(parse(&zlib(b"not a field stream, just text, padding padding padding")).is_none());
    }
}
