use tracing::{debug, warn};

use crate::interpreter::aliyun_cli::AliyunCli;

pub enum Language {
    ZH,
    EN,
}

impl From<String> for Language {
    fn from(s: String) -> Self {
        match s.as_str() {
            "zh" => Language::ZH,
            "en" => Language::EN,
            _ => {
                warn!("Invalid language: {}, default to `zh`", s);
                Language::ZH
            }
        }
    }
}

impl Language {
    pub fn to_string(&self) -> String {
        match self {
            Language::ZH => "zh".to_string(),
            Language::EN => "en".to_string(),
        }
    }
}

pub struct Interpreter {
    language: Language,
    aliyun_cli: Option<AliyunCli>,
}

impl Interpreter {
    pub fn new(
        language: String,
        access_key_id: Option<String>,
        access_key_secret: Option<String>,
    ) -> Interpreter {
        let language = Language::from(language);
        let aliyun_cli = match (access_key_id, access_key_secret) {
            (Some(id), Some(secret)) => Some(AliyunCli::new(id, secret)),
            _ => None,
        };

        Interpreter {
            language,
            aliyun_cli,
        }
    }

    /// 翻译一段聊天文本。
    ///
    /// 约定：**任何失败都返回 `None`，让调用方原样显示原文**。
    /// 这里不 panic、也不往聊天里塞 "[WARN] ..." 之类的占位符 ——
    /// 翻译是增值功能，坏了不该把聊天本身也搞乱。
    pub async fn translate(&self, text: String) -> Option<String> {
        let cli = self.aliyun_cli.as_ref()?;

        // 空文本/纯空白一定被阿里云拒（实测 Code=10004「参数出错」），
        // 本地先挡掉：既不浪费一次请求，也不在日志里刷无意义的告警。
        if text.trim().is_empty() {
            debug!("Skipping translation for empty chat message");
            return None;
        }

        match cli.translate(&text, &self.language.to_string()).await {
            Ok(translated) => Some(translated),
            Err(e) => {
                // 把原文一起打出来，这样日志能直接告诉你"哪句话翻失败了、为什么"
                warn!("Translation failed for {:?}: {}", text, e);
                None
            }
        }
    }
}
