//! 26.10 客户端的聊天载荷解析。
//!
//! 26.9 及更早，`Avatar.onChatMessageRegular` 是三个独立参数
//! （`senderId:PLAYER_ID`、`channelId:STRING`、`message:STRING`）。26.10 起
//! `Avatar.def` 把它改成了**单个参数** `messageDef:CHAT_MESSAGE_BLOB`
//! （`alias.xml` 里定义为 `USER_TYPE / BLOB`，由
//! `CommonDefs.chat_messages_def.converter` 实现），也就是一段不透明字节。
//!
//! 实测这段字节是 **MsgPack**：
//!
//! ```text
//! 92                     fixarray(2)
//!   01                   整数 1 —— 消息种类判别位（1 = 普通聊天）
//!   93                   fixarray(3)
//!     a6 "你好"            消息正文
//!     ce 08 00 83 65       发送者账号 ID（与玩家列表里的 account_id 同值域）
//!     ab "battle_team"     频道：battle_team / battle_common / …
//! ```
//!
//! 注意 26.10 里**正文在数组第 0 位、发送者第 1 位、频道第 2 位**，
//! 与 `onChatMessageNotification`（仍是 senderId/channelId/message 顺序）不同，
//! 别按老顺序取值。
//!
//! 这里只做「读」而且**零拷贝**：返回的 `&str` 直接指向 blob 内部的字节，
//! 因此调用方可以把它当作借用自回放缓冲区的切片使用。
//!
//! 格式不对（判别位不是 1、元素类型不符、被截断……）一律返回 `None`，
//! 由调用方降级处理 —— **宁可什么都不显示，也不要显示错的东西，更不能 panic**。

/// 一条解出来的聊天消息。字段全部借用自传入的 blob。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChatMessage<'a> {
    /// 发送者账号 ID
    pub sender_id: u32,
    /// 频道（`battle_team`、`battle_common`……）
    pub channel: &'a str,
    /// 消息正文
    pub message: &'a str,
}

/// 普通聊天的判别位（MsgPack 数组第 0 位）。
const KIND_REGULAR: u64 = 1;

/// 解析 `CHAT_MESSAGE_BLOB`。失败返回 `None`。
pub fn parse(blob: &[u8]) -> Option<ChatMessage<'_>> {
    let mut cur = Cursor::new(blob);

    // 外层：fixarray(2) = [判别位, 内层数组]
    let outer = cur.array_len()?;
    if outer != 2 {
        return None;
    }
    if cur.uint()? != KIND_REGULAR {
        return None;
    }

    // 内层：fixarray(3) = [正文, 发送者 ID, 频道]
    let inner = cur.array_len()?;
    if inner != 3 {
        return None;
    }
    let message = cur.str_bytes()?;
    let sender_id = cur.uint()?;
    let channel = cur.str_bytes()?;

    Some(ChatMessage {
        sender_id: u32::try_from(sender_id).ok()?,
        channel: std::str::from_utf8(channel).ok()?,
        message: std::str::from_utf8(message).ok()?,
    })
}

/// 极简的 MsgPack 顺序读取器。只实现聊天载荷里实际出现的类型。
struct Cursor<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    fn new(data: &'a [u8]) -> Self {
        Cursor { data, pos: 0 }
    }

    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let end = self.pos.checked_add(n)?;
        let slice = self.data.get(self.pos..end)?;
        self.pos = end;
        Some(slice)
    }

    fn byte(&mut self) -> Option<u8> {
        self.take(1).map(|b| b[0])
    }

    /// 数组长度：fixarray / array16 / array32
    fn array_len(&mut self) -> Option<usize> {
        match self.byte()? {
            0x90..=0x9f => Some((self.data[self.pos - 1] - 0x90) as usize),
            0xdc => {
                let b = self.take(2)?;
                Some(u16::from_be_bytes([b[0], b[1]]) as usize)
            }
            0xdd => {
                let b = self.take(4)?;
                Some(u32::from_be_bytes([b[0], b[1], b[2], b[3]]) as usize)
            }
            _ => None,
        }
    }

    /// 无符号整数（MsgPack 的 int 系列，实测发送者 ID 走的是 uint32）
    fn uint(&mut self) -> Option<u64> {
        match self.byte()? {
            // positive fixint
            0x00..=0x7f => Some((self.data[self.pos - 1] & 0x7f) as u64),
            0xcc => Some(self.byte()? as u64),
            0xcd => {
                let b = self.take(2)?;
                Some(u16::from_be_bytes([b[0], b[1]]) as u64)
            }
            0xce => {
                let b = self.take(4)?;
                Some(u32::from_be_bytes([b[0], b[1], b[2], b[3]]) as u64)
            }
            0xcf => {
                let b = self.take(8)?;
                Some(u64::from_be_bytes([
                    b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7],
                ]))
            }
            _ => None,
        }
    }

    /// 字符串字节：fixstr / str8 / str16 / str32
    fn str_bytes(&mut self) -> Option<&'a [u8]> {
        let n = match self.byte()? {
            0xa0..=0xbf => (self.data[self.pos - 1] - 0xa0) as usize,
            0xd9 => self.byte()? as usize,
            0xda => {
                let b = self.take(2)?;
                u16::from_be_bytes([b[0], b[1]]) as usize
            }
            0xdb => {
                let b = self.take(4)?;
                u32::from_be_bytes([b[0], b[1], b[2], b[3]]) as usize
            }
            _ => return None,
        };
        self.take(n)
    }
}

#[cfg(test)]
mod test {
    use super::*;

    /// 实测抓到的 26.10 载荷（「你好」发到战斗队伍频道）
    const SAMPLE: &[u8] = &[
        0x92, 0x01, 0x93, 0xa6, 0xe4, 0xbd, 0xa0, 0xe5, 0xa5, 0xbd, 0xce, 0x08, 0x00, 0x83, 0x65,
        0xab, 0x62, 0x61, 0x74, 0x74, 0x6c, 0x65, 0x5f, 0x74, 0x65, 0x61, 0x6d,
    ];

    #[test]
    fn parses_real_2610_payload() {
        let m = parse(SAMPLE).expect("should parse");
        assert_eq!(m.message, "你好");
        assert_eq!(m.channel, "battle_team");
        assert_eq!(m.sender_id, 134_251_365);
    }

    #[test]
    fn parses_short_message() {
        // 第二条实测样本：正文 "111"
        let blob = &[
            0x92, 0x01, 0x93, 0xa3, 0x31, 0x31, 0x31, 0xce, 0x08, 0x00, 0x83, 0x65, 0xab, 0x62,
            0x61, 0x74, 0x74, 0x6c, 0x65, 0x5f, 0x74, 0x65, 0x61, 0x6d,
        ];
        let m = parse(blob).expect("should parse");
        assert_eq!(m.message, "111");
        assert_eq!(m.sender_id, 134_251_365);
    }

    #[test]
    fn rejects_wrong_kind() {
        let mut blob = SAMPLE.to_vec();
        blob[1] = 0x02; // 判别位不是 1
        assert!(parse(&blob).is_none());
    }

    #[test]
    fn rejects_truncated() {
        for n in 0..SAMPLE.len() {
            assert!(parse(&SAMPLE[..n]).is_none(), "len={} 不该解析成功", n);
        }
    }

    #[test]
    fn rejects_garbage() {
        assert!(parse(&[]).is_none());
        assert!(parse(b"\xff\xff\xff").is_none());
        // 26.9 那种「三个字符串参数」的裸拼接不是合法 MsgPack 数组，不能误判
        assert!(parse(b"hello world").is_none());
    }
}
