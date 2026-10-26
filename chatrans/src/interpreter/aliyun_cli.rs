/**
 * Reference: https://help.aliyun.com/zh/machine-translation/developer-reference/signature-mechanism
 */
use md5::{Md5, Digest};
use base64::prelude::*;
use sha1::Sha1;
use hmac::{Hmac, Mac};
use std::time::Duration;
use url::Url;
use uuid::Uuid;

const SIGNATURE_METHOD: &str = "HMAC-SHA1";
const SIGNATURE_VERSION: &str = "2019-01-02";

/// 阿里云机器翻译的「网页版」接口。
///
/// 注意两个反直觉的地方：
/// 1. 它出错时 HTTP 状态码**也是 200**（例如 Code=10004「参数出错」），
///    所以判断成败只能看应答体里的 `Code`，不能看状态码。
/// 2. 成功时 `Code` 是字符串 `"200"`，不是数字。
pub const SERVER_URL: &str = "http://mt.cn-hangzhou.aliyuncs.com/api/translate/web/ecommerce";

pub struct AliyunCli {
    access_key_id: String,
    access_key_secret: String,
    http: reqwest::Client,
}

/// 把应答体截断成一段适合写进日志的预览。
fn preview(s: &str) -> String {
    let s = s.trim();
    if s.chars().count() <= 300 {
        s.to_string()
    } else {
        let head: String = s.chars().take(300).collect();
        format!("{}…", head)
    }
}

/// 给常见错误码配一句人话，免得用户只看到 "参数出错" 无从下手。
fn hint_for(code: &str) -> &'static str {
    match code {
        "10004" => "（源文本为空/全空白，或请求体非法）",
        "InvalidAccessKeyId.NotFound" => "（AccessKeyId 不存在、拼错或已被禁用）",
        "InvalidAccessKeyId.Inactive" => "（AccessKey 已被禁用）",
        "SignatureDoesNotMatch" => "（AccessKeySecret 不匹配）",
        "Forbidden" | "NoPermission" => {
            "（该密钥没有机器翻译权限，建议给 RAM 子账号挂 AliyunMTFullAccess）"
        }
        "Forbidden.ServiceNotActivated" => "（账号还没开通机器翻译服务）",
        _ => "",
    }
}

impl AliyunCli {
    pub fn new(access_key_id: String, access_key_secret: String) -> AliyunCli {
        // 超时是必须的：翻译调用在消息管线里被 await，没有超时的话
        // 一次网络卡住就会把后面所有聊天消息一起堵死，网页再也不刷新。
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(8))
            .timeout(Duration::from_secs(15))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());
        AliyunCli {
            access_key_id,
            access_key_secret,
            http,
        }
    }

    /// Calculate the MD5 hash of a string and encode it in base64
    fn md5_base64(s: &str) -> String {
        // Create a new hasher to calculate the MD5 hash
        let mut hasher = Md5::new();
        hasher.update(s.as_bytes());
        // Get the hash result
        let md5_result = hasher.finalize();

        // Encode the hash result in base64
        BASE64_STANDARD.encode(&md5_result)
    }

    /// Calculate the HMAC-SHA1 hash of a string and encode it in base64
    fn hmacsha1_base64(data: &str, key: &str) -> String {
        // Create a new HMAC-SHA1 hasher
        let mut hasher: Hmac<Sha1> = Mac::new_from_slice(key.as_bytes()).unwrap();
        // Update the hasher with the data
        hasher.update(data.as_bytes());
        // Get the hash result
        let result = hasher.finalize().into_bytes();

        // Encode the hash result in base64
        BASE64_STANDARD.encode(&result)
    }

    // Get time with format: "E, dd MMM yyyy HH:mm:ss z"
    fn get_time() -> String {
        chrono::Utc::now().format("%a, %d %b %Y %H:%M:%S GMT").to_string()
    }

    /// 组装翻译接口的请求体。
    ///
    /// 用 serde_json 而不是 `format!` 拼字符串：聊天内容里只要出现一个 `"`，
    /// 手工拼装的报文就不再是合法 JSON，阿里云会回 10004「参数出错」；
    /// 出现 `\t`、`\n` 这类还会被静默解析成控制字符，把原文悄悄改掉。
    pub fn build_body(text: &str, target_language: &str) -> String {
        serde_json::json!({
            "FormatType": "text",
            "SourceLanguage": "auto",
            "TargetLanguage": target_language,
            "SourceText": text,
            "Scene": "social",
        })
        .to_string()
    }

    /// Get the access url
    ///
    /// 网络/协议层的失败都返回 `Err(String)`，**不再 panic** —— 调用方在
    /// 消息管线里，一次 panic 会连带把整个转发任务干掉。
    pub async fn send_post(&self, url: &str, body: &str) -> Result<String, String> {
        let url = Url::parse(url).map_err(|e| format!("invalid Aliyun url: {}", e))?;

        // Set the request parameters
        let method = "POST";
        let accept = "application/json";
        let content_type = "application/json;chrset=utf-8";
        let path = url.path();
        let date = Self::get_time();
        let host = url.host_str().unwrap_or_default().to_string();
        let uuid = Uuid::new_v4().to_string();

        // Apply md5 and base64 to the body
        let body_md5 = Self::md5_base64(body);
        let string_to_sign = format!(
            "{}\n{}\n{}\n{}\n{}\nx-acs-signature-method:{}\nx-acs-signature-nonce:{}\nx-acs-version:{}\n{}",
            method,
            accept,
            body_md5,
            content_type,
            date,
            SIGNATURE_METHOD,
            uuid,
            SIGNATURE_VERSION,
            path
        );
        // Apply hmac-sha1 and base64 to the string to sign
        let signature = Self::hmacsha1_base64(&string_to_sign, &self.access_key_secret);
        // Set the authorization header
        let authorization = format!("acs {}:{}", self.access_key_id, signature);

        // Connect to the server
        let response = self
            .http
            .post(url.clone())
            .header("Accept", accept)
            .header("Content-Type", content_type)
            .header("Content-MD5", body_md5)
            .header("Date", date)
            .header("Host", host)
            .header("Authorization", authorization)
            .header("x-acs-signature-nonce", uuid)
            .header("x-acs-signature-method", SIGNATURE_METHOD)
            .header("x-acs-version", SIGNATURE_VERSION)
            .body(body.to_string())
            .send()
            .await
            .map_err(|e| format!("request to Aliyun failed: {}", e))?;

        response
            .text()
            .await
            .map_err(|e| format!("failed to read Aliyun response: {}", e))
    }

    /// 翻译一段文本。失败时返回的字符串里带上阿里云原始的错误码/描述，
    /// 方便直接判断是密钥问题、权限问题，还是源文本问题。
    pub async fn translate(&self, text: &str, target_language: &str) -> Result<String, String> {
        let body = Self::build_body(text, target_language);
        let raw = self.send_post(SERVER_URL, &body).await?;

        let value: serde_json::Value = serde_json::from_str(&raw)
            .map_err(|e| format!("Aliyun returned a non-JSON response ({}): {}", e, preview(&raw)))?;

        // `Code` 可能是字符串 "200"，也可能是数字 200，两种都认
        let code = match &value["Code"] {
            serde_json::Value::String(s) => s.clone(),
            serde_json::Value::Number(n) => n.to_string(),
            other => other.to_string(),
        };

        if code == "200" {
            match value["Data"]["Translated"].as_str() {
                Some(translated) if !translated.is_empty() => Ok(translated.to_string()),
                _ => Err(format!(
                    "Aliyun replied Code=200 but carried no translation: {}",
                    preview(&raw)
                )),
            }
        } else {
            Err(format!(
                "Aliyun error Code={} Message={:?} RequestId={} {}",
                code,
                value["Message"].as_str().unwrap_or(""),
                value["RequestId"].as_str().unwrap_or("-"),
                hint_for(&code),
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 回归护栏：聊天里带引号/反斜杠时，请求体必须仍是合法 JSON，且原文一字不改。
    #[test]
    fn body_stays_valid_json_with_quotes_and_backslashes() {
        let src = "he said \"hi\" and C:\\path\\to\\file";
        let body = AliyunCli::build_body(src, "zh");
        let value: serde_json::Value =
            serde_json::from_str(&body).expect("request body must be valid JSON");
        assert_eq!(value["SourceText"], src);
        assert_eq!(value["TargetLanguage"], "zh");
        assert_eq!(value["Scene"], "social");
    }

    /// 需要真实密钥的联网测试（默认不跑）：
    ///   $env:TR_AK="..."; $env:TR_SK="..."
    ///   cargo test --release -p chatrans -- --ignored aliyun_live_translate --nocapture
    #[tokio::test]
    #[ignore]
    async fn aliyun_live_translate() {
        let (id, secret) = match (std::env::var("TR_AK"), std::env::var("TR_SK")) {
            (Ok(id), Ok(secret)) => (id, secret),
            _ => {
                eprintln!("TR_AK / TR_SK not set, skipping live test");
                return;
            }
        };
        let cli = AliyunCli::new(id, secret);
        for text in ["hello world", "привет мир", "he said \"hi\""] {
            match cli.translate(text, "zh").await {
                Ok(t) => println!("OK   {:?} -> {:?}", text, t),
                Err(e) => panic!("FAIL {:?} -> {}", text, e),
            }
        }
    }
}
