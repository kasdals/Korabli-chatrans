//! OpenAI / Anthropic 双协议客户端（移植自雷达 addon 的 `ProtocolClient.cs`）。
//!
//! 职责：URL 规范化、请求体构造、双协议头、响应解析、错误挖人话。
//! 两个硬规矩：
//!   1. `HTTP_PROXY` 等环境变量**碰不到**这条链路 —— 没配供应商代理时一律
//!      `.no_proxy()`（雷达上被代理劫持坑过，方案 C 从根上堵死）；
//!   2. 空白 key / 空白模型名 / 空白原文不发网络，直接返回失败结果。
//!
//! 超时 10s（对局节奏），模型名/温度等参数照抄 addon 默认值。

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use serde_json::json;
use tracing::warn;

use super::store::{TranslationEndpoint, WireProtocol};

/// 翻译用的固定提示词。质量靠这个，不靠模型"聪明"。（照抄 addon。）
pub fn system_prompt(target_language: &str) -> String {
    "你是《战舰世界》游戏内聊天的翻译器。把用户给出的一行聊天翻译成".to_string()
        + target_language
        + "。\n"
        + "只输出译文本身：不要解释、不要引号、不要重复原文、不要加\"译文：\"前缀。\n"
        + "玩家昵称、军团名、舰船型号名保持原文不译；脏话按原意直译，保留原本语气。\n"
        + "原文已经是"
        + target_language
        + "时，原样返回。"
}

/// 一次翻译/探测的结果。失败也必须给出人话原因，不能只吞掉。
#[derive(Debug, Clone)]
pub struct TranslateResult {
    pub ok: bool,
    pub text: Option<String>,
    pub error: Option<String>,
    pub elapsed: Duration,
    pub status_code: Option<u16>,
}

impl TranslateResult {
    pub fn fail(error: impl Into<String>, elapsed: Duration, status: Option<u16>) -> TranslateResult {
        TranslateResult {
            ok: false,
            text: None,
            error: Some(error.into()),
            elapsed,
            status_code: status,
        }
    }

    fn ok(text: String, elapsed: Duration, status: u16) -> TranslateResult {
        TranslateResult {
            ok: true,
            text: Some(text),
            error: None,
            elapsed,
            status_code: Some(status),
        }
    }
}

/// `.../v1`、`.../v2` 这种版本段是否已存在。（照抄 addon 的判定：
/// 段长 ≥2、v/V 开头、后面全是数字。）
pub fn has_version_segment(url: &str) -> bool {
    let path = match url.find("://") {
        Some(scheme_index) => match url[scheme_index + 3..].find('/') {
            Some(slash) => &url[scheme_index + 3 + slash..],
            None => "",
        },
        None => url,
    };
    path.split('/')
        .filter(|s| !s.is_empty())
        .any(|segment| {
            segment.len() >= 2
                && matches!(segment.as_bytes()[0], b'v' | b'V')
                && segment[1..].bytes().all(|b| b.is_ascii_digit())
        })
}

/// 把用户填的 base URL 规范成最终请求地址。
///
/// 真实世界里最常见的四种写法都要认（照抄 addon）：
///   https://api.deepseek.com            → 补 /v1 + 路径
///   https://api.deepseek.com/v1         → 补路径
///   https://api.deepseek.com/v1/        → 去尾斜杠后补路径
///   https://api.deepseek.com/v1/chat/completions → 已经是完整地址，原样用
pub fn resolve_request_url(
    base_url: &str,
    protocol: WireProtocol,
) -> Result<String, String> {
    let url = base_url.trim().trim_end_matches('/');
    if url.is_empty() {
        return Err("API Base URL 不能为空".to_string());
    }

    let complete_path = match protocol {
        WireProtocol::OpenAi => "/chat/completions",
        WireProtocol::Anthropic => "/v1/messages",
    };
    if url.to_ascii_lowercase().ends_with(&complete_path.to_ascii_lowercase()) {
        return Ok(url.to_string());
    }

    let mut url = url.to_string();
    if !has_version_segment(&url) {
        url.push_str("/v1");
    }
    match protocol {
        WireProtocol::Anthropic => url.push_str("/messages"),
        WireProtocol::OpenAi => url.push_str("/chat/completions"),
    }
    Ok(url)
}

/// 组装翻译请求体。OpenAI：system 进 messages 数组；Anthropic：system 顶层分离。
/// max_tokens=256、temperature=0 照抄 addon 默认值。
pub fn build_request_body(
    endpoint: &TranslationEndpoint,
    text: &str,
    target_language: &str,
) -> String {
    let prompt = system_prompt(target_language);
    match endpoint.protocol {
        WireProtocol::Anthropic => json!({
            "model": endpoint.model,
            "max_tokens": 256,
            "temperature": 0,
            "system": prompt,
            "messages": [{ "role": "user", "content": text }],
        }),
        WireProtocol::OpenAi => json!({
            "model": endpoint.model,
            "temperature": 0,
            "max_tokens": 256,
            "messages": [
                { "role": "system", "content": prompt },
                { "role": "user", "content": text },
            ],
        }),
    }
    .to_string()
}

/// 从响应里挖译文。两家协议各自的形状；解析不动就返回 None。
pub fn extract_translated_text(protocol: WireProtocol, payload: &str) -> Option<String> {
    let root: serde_json::Value = serde_json::from_str(payload).ok()?;
    match protocol {
        WireProtocol::Anthropic => {
            let content = root.get("content")?.as_array()?;
            content
                .iter()
                .find(|block| block.get("type").and_then(|t| t.as_str()) == Some("text"))
                .and_then(|block| block.get("text"))
                .and_then(|t| t.as_str())
                .map(|s| s.to_string())
        }
        WireProtocol::OpenAi => root
            .get("choices")?
            .as_array()?
            .first()?
            .get("message")?
            .get("content")?
            .as_str()
            .map(|s| s.to_string()),
    }
}

/// 从错误响应里挖一句人话。两家都是 `error.message`；`error` 直接是字符串也认。
/// 不是 JSON（网关返回的 HTML 之类）→ None，让调用方用状态码。
pub fn extract_error_message(payload: &str) -> Option<String> {
    let root: serde_json::Value = serde_json::from_str(payload).ok()?;
    let error = root.get("error")?;
    match error {
        serde_json::Value::String(s) => Some(s.clone()),
        serde_json::Value::Object(_) => error
            .get("message")
            .and_then(|m| m.as_str())
            .map(|s| s.to_string()),
        _ => None,
    }
}

/// 把 `k=v;k2=v2` 这种一行输入解析成字典。UI 只有一行输入框，这里替它拆。
pub fn parse_header_line(line: Option<&str>) -> BTreeMap<String, String> {
    let mut result = BTreeMap::new();
    let Some(line) = line else {
        return result;
    };
    if line.trim().is_empty() {
        return result;
    }
    for part in line.split(';') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        if let Some(index) = part.find('=') {
            let key = part[..index].trim();
            let value = part[index + 1..].trim();
            if !key.is_empty() {
                result.insert(key.to_string(), value.to_string());
            }
        }
    }
    result
}

/// 为一个供应商构建 HTTP 客户端。
///
/// 代理语义与 addon 一致：供应商填了 proxy 就走它；没填就 `.no_proxy()` ——
/// 显式禁用环境代理，`HTTP_PROXY` 再也劫持不到翻译链路（方案 C 的核心收益之一）。
pub fn build_http_client(endpoint: &TranslationEndpoint) -> reqwest::Client {
    let timeout = Duration::from_secs(endpoint.timeout_seconds.max(3));
    let mut builder = reqwest::Client::builder()
        .timeout(timeout)
        .connect_timeout(timeout);

    let proxy = endpoint.proxy.trim();
    if proxy.is_empty() {
        builder = builder.no_proxy();
    } else {
        match reqwest::Proxy::all(proxy) {
            Ok(p) => builder = builder.proxy(p),
            // 代理地址写错就当直连，别让配置问题变成崩溃
            Err(e) => {
                warn!("供应商 {} 的代理地址非法（{}），按直连处理", endpoint.id, e);
                builder = builder.no_proxy();
            }
        }
    }

    builder
        .build()
        .unwrap_or_else(|e| {
            warn!("HTTP client build failed ({}), falling back to no-proxy default", e);
            reqwest::Client::builder()
                .no_proxy()
                .build()
                .unwrap_or_else(|_| reqwest::Client::new())
        })
}

/// 安全地加请求头：非法名字/值直接跳过并留日志，绝不 panic
/// （`RequestBuilder::header` 对非法输入是 panic 语义，必须先验证）。
fn with_header(
    builder: reqwest::RequestBuilder,
    name: &str,
    value: &str,
) -> reqwest::RequestBuilder {
    match (
        reqwest::header::HeaderName::from_bytes(name.as_bytes()),
        reqwest::header::HeaderValue::from_str(value),
    ) {
        (Ok(n), Ok(v)) => builder.header(n, v),
        _ => {
            warn!("[翻译] 忽略非法请求头: {:?}", name);
            builder
        }
    }
}

/// 发一次翻译请求。`api_key` 为空时直接返回失败，不发网络。
pub async fn translate(
    endpoint: &TranslationEndpoint,
    api_key: Option<&str>,
    text: &str,
    target_language: &str,
) -> TranslateResult {
    let started = Instant::now();

    let Some(api_key) = api_key.map(str::trim).filter(|k| !k.is_empty()) else {
        return TranslateResult::fail("未配置 API Key", started.elapsed(), None);
    };
    if endpoint.model.trim().is_empty() {
        return TranslateResult::fail("未填写模型名", started.elapsed(), None);
    }
    if text.trim().is_empty() {
        return TranslateResult::fail("原文为空", started.elapsed(), None);
    }

    let request_url = match resolve_request_url(&endpoint.base_url, endpoint.protocol) {
        Ok(url) => url,
        Err(e) => return TranslateResult::fail(e, started.elapsed(), None),
    };

    let body = build_request_body(endpoint, text, target_language);
    let client = build_http_client(endpoint);

    let mut builder = client
        .post(&request_url)
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .body(body);

    // 双协议各自的认证头（照抄 addon；anthropic-version 固定 2023-06-01）
    builder = match endpoint.protocol {
        WireProtocol::Anthropic => {
            if reqwest::header::HeaderValue::from_str(api_key).is_err() {
                return TranslateResult::fail("API Key 含非法字符", started.elapsed(), None);
            }
            builder
                .header("x-api-key", api_key)
                .header("anthropic-version", "2023-06-01")
        }
        WireProtocol::OpenAi => {
            let bearer = format!("Bearer {}", api_key);
            if reqwest::header::HeaderValue::from_str(&bearer).is_err() {
                return TranslateResult::fail("API Key 含非法字符", started.elapsed(), None);
            }
            builder.header(reqwest::header::AUTHORIZATION, bearer)
        }
    };

    // 自定义头放最后：中转站/网关常要求覆盖某些字段（包括 Authorization）。
    // reqwest 的 header() 对同名键是替换语义，与 addon 的 Remove+Add 一致。
    for (name, value) in &endpoint.extra_headers {
        if name.trim().is_empty() {
            continue;
        }
        builder = with_header(builder, name.trim(), value.trim());
    }

    match builder.send().await {
        Ok(response) => {
            let status = response.status();
            let payload = response.text().await.unwrap_or_default();
            if !status.is_success() {
                let reason = extract_error_message(&payload).unwrap_or_else(|| {
                    format!(
                        "HTTP {} {}",
                        status.as_u16(),
                        status.canonical_reason().unwrap_or("")
                    )
                });
                return TranslateResult::fail(reason, started.elapsed(), Some(status.as_u16()));
            }
            match extract_translated_text(endpoint.protocol, &payload) {
                Some(translated) if !translated.trim().is_empty() => {
                    TranslateResult::ok(translated.trim().to_string(), started.elapsed(), status.as_u16())
                }
                _ => TranslateResult::fail("响应里没有译文", started.elapsed(), Some(status.as_u16())),
            }
        }
        Err(e) if e.is_timeout() => TranslateResult::fail(
            format!("超时（{}s）", endpoint.timeout_seconds),
            started.elapsed(),
            None,
        ),
        Err(e) => TranslateResult::fail(e.to_string(), started.elapsed(), None),
    }
}

/// 探测：翻一条固定文本，用来验证 key / base url / 模型三件套是否都对。
pub async fn probe(
    endpoint: &TranslationEndpoint,
    api_key: Option<&str>,
    target_language: &str,
) -> TranslateResult {
    translate(endpoint, api_key, "hello world", target_language).await
}

/// 模型列表请求的 URL（S5 设置视图"获取模型列表"用）。
///
/// 与翻译 URL 的规范化策略一致：去尾斜杠；OpenAI 协议缺版本段时补 `/v1`
/// （`https://api.deepseek.com` → `https://api.deepseek.com/v1/models`）。
/// Anthropic 固定 `{base}/v1/models`（官方 base 本来就不带版本段）。
pub fn resolve_models_url(base_url: &str, protocol: WireProtocol) -> Result<String, String> {
    let url = base_url.trim().trim_end_matches('/');
    if url.is_empty() {
        return Err("API Base URL 不能为空".to_string());
    }
    match protocol {
        WireProtocol::OpenAi => {
            let mut url = url.to_string();
            if !has_version_segment(&url) {
                url.push_str("/v1");
            }
            url.push_str("/models");
            Ok(url)
        }
        WireProtocol::Anthropic => Ok(format!("{}/v1/models", url)),
    }
}

/// 从 `/models` 响应里挖模型 id 列表（OpenAI 与 Anthropic 同形状）：
/// `{"data":[{"id":..},..]}`。保序 + 去重 + 丢空白项；形状不对 → None
/// （调用方降级到手动填模型）。
pub fn parse_models_payload(payload: &str) -> Option<Vec<String>> {
    let root: serde_json::Value = serde_json::from_str(payload).ok()?;
    let data = root.get("data")?.as_array()?;
    let mut models: Vec<String> = Vec::new();
    for item in data {
        let id = item.get("id")?.as_str()?;
        let id = id.trim();
        if !id.is_empty() && !models.iter().any(|m| m == id) {
            models.push(id.to_string());
        }
    }
    Some(models)
}

/// 拉取供应商的可用模型列表（S5 设置视图）。失败返回人话 `Err`，
/// UI 据此降级提示 + 保留手动填模型的路径；绝不 panic。
///
/// no_proxy 纪律与翻译链路一致：复用 [`build_http_client`] ——
/// 只有供应商显式配置了 proxy 才走代理，环境变量永远劫持不到。
pub async fn fetch_models(
    endpoint: &TranslationEndpoint,
    api_key: Option<&str>,
) -> Result<Vec<String>, String> {
    let Some(api_key) = api_key.map(str::trim).filter(|k| !k.is_empty()) else {
        return Err("未配置 API Key".to_string());
    };
    let request_url = resolve_models_url(&endpoint.base_url, endpoint.protocol)?;

    let client = build_http_client(endpoint);
    let mut builder = client.get(&request_url);
    builder = match endpoint.protocol {
        WireProtocol::Anthropic => {
            if reqwest::header::HeaderValue::from_str(api_key).is_err() {
                return Err("API Key 含非法字符".to_string());
            }
            builder
                .header("x-api-key", api_key)
                .header("anthropic-version", "2023-06-01")
        }
        WireProtocol::OpenAi => {
            let bearer = format!("Bearer {}", api_key);
            if reqwest::header::HeaderValue::from_str(&bearer).is_err() {
                return Err("API Key 含非法字符".to_string());
            }
            builder.header(reqwest::header::AUTHORIZATION, bearer)
        }
    };
    for (name, value) in &endpoint.extra_headers {
        if name.trim().is_empty() {
            continue;
        }
        builder = with_header(builder, name.trim(), value.trim());
    }

    match builder.send().await {
        Ok(response) => {
            let status = response.status();
            let payload = response.text().await.unwrap_or_default();
            if !status.is_success() {
                let reason = extract_error_message(&payload).unwrap_or_else(|| {
                    format!(
                        "HTTP {} {}",
                        status.as_u16(),
                        status.canonical_reason().unwrap_or("")
                    )
                });
                return Err(reason);
            }
            match parse_models_payload(&payload) {
                Some(models) if !models.is_empty() => Ok(models),
                _ => Err("响应里没有模型列表（部分兼容站不支持 /models，请手动填）".to_string()),
            }
        }
        Err(e) if e.is_timeout() => {
            Err(format!("超时（{}s）", endpoint.timeout_seconds))
        }
        Err(e) => Err(e.to_string()),
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    const EP_OPENAI: fn(&str) -> TranslationEndpoint = |base| TranslationEndpoint {
        id: "t".to_string(),
        protocol: WireProtocol::OpenAi,
        base_url: base.to_string(),
        model: "deepseek-chat".to_string(),
        ..Default::default()
    };

    const EP_ANTHROPIC: fn(&str) -> TranslationEndpoint = |base| TranslationEndpoint {
        id: "t".to_string(),
        protocol: WireProtocol::Anthropic,
        base_url: base.to_string(),
        model: "claude-3-haiku".to_string(),
        ..Default::default()
    };

    /// addon 注释里点名的四种写法（OpenAI 协议）。
    #[test]
    fn resolve_url_openai_four_common_forms() {
        let p = WireProtocol::OpenAi;
        assert_eq!(
            resolve_request_url("https://api.deepseek.com", p).unwrap(),
            "https://api.deepseek.com/v1/chat/completions"
        );
        assert_eq!(
            resolve_request_url("https://api.deepseek.com/v1", p).unwrap(),
            "https://api.deepseek.com/v1/chat/completions"
        );
        assert_eq!(
            resolve_request_url("https://api.deepseek.com/v1/", p).unwrap(),
            "https://api.deepseek.com/v1/chat/completions"
        );
        assert_eq!(
            resolve_request_url("https://api.deepseek.com/v1/chat/completions", p).unwrap(),
            "https://api.deepseek.com/v1/chat/completions"
        );
    }

    #[test]
    fn resolve_url_anthropic_forms() {
        let p = WireProtocol::Anthropic;
        assert_eq!(
            resolve_request_url("https://api.anthropic.com", p).unwrap(),
            "https://api.anthropic.com/v1/messages"
        );
        assert_eq!(
            resolve_request_url("https://api.anthropic.com/v1", p).unwrap(),
            "https://api.anthropic.com/v1/messages"
        );
        assert_eq!(
            resolve_request_url("https://api.anthropic.com/v1/messages", p).unwrap(),
            "https://api.anthropic.com/v1/messages"
        );
    }

    #[test]
    fn resolve_url_edge_cases() {
        // 已有 v2 版本段：不重复补 /v1
        assert_eq!(
            resolve_request_url("https://x.com/v2", WireProtocol::OpenAi).unwrap(),
            "https://x.com/v2/chat/completions"
        );
        // 大小写不敏感的完整路径识别
        assert_eq!(
            resolve_request_url("https://x.com/V1/CHAT/COMPLETIONS", WireProtocol::OpenAi).unwrap(),
            "https://x.com/V1/CHAT/COMPLETIONS"
        );
        // v2beta 不是版本段（后缀有字母），要补 /v1 —— 与 addon 判定一致
        assert_eq!(
            resolve_request_url("https://x.com/v2beta", WireProtocol::OpenAi).unwrap(),
            "https://x.com/v2beta/v1/chat/completions"
        );
        // 周围空白要去掉
        assert_eq!(
            resolve_request_url("  https://x.com  ", WireProtocol::OpenAi).unwrap(),
            "https://x.com/v1/chat/completions"
        );
        // 空 URL 报错，不 panic
        assert!(resolve_request_url("", WireProtocol::OpenAi).is_err());
        assert!(resolve_request_url("   ", WireProtocol::OpenAi).is_err());
    }

    #[test]
    fn has_version_segment_cases() {
        assert!(has_version_segment("https://x.com/v1"));
        assert!(has_version_segment("https://x.com/V2/other"));
        assert!(has_version_segment("https://x.com/api/v10"));
        assert!(!has_version_segment("https://x.com"));
        assert!(!has_version_segment("https://x.com/violin")); // v 后不全是数字
        assert!(!has_version_segment("https://x.com/v")); // 段长 <2
        // 无 scheme 时整串当 path —— 与 addon 的 C# 判定一致
        assert!(has_version_segment("x.com/v1"));
    }

    /// OpenAI 请求体：system 在 messages 里，参数照抄 addon 默认值。
    #[test]
    fn openai_request_body_shape() {
        let body = build_request_body(&EP_OPENAI("https://x.com"), "gg wp", "zh");
        let v: serde_json::Value = serde_json::from_str(&body).expect("valid json");
        assert_eq!(v["model"], "deepseek-chat");
        assert_eq!(v["temperature"], 0);
        assert_eq!(v["max_tokens"], 256);
        assert_eq!(v["messages"][0]["role"], "system");
        assert!(v["messages"][0]["content"]
            .as_str()
            .unwrap()
            .starts_with("你是《战舰世界》游戏内聊天的翻译器"));
        assert_eq!(v["messages"][1]["role"], "user");
        assert_eq!(v["messages"][1]["content"], "gg wp");
        assert!(v.get("system").is_none());
    }

    /// Anthropic 请求体：system 顶层分离，x-api-key 语义。
    #[test]
    fn anthropic_request_body_shape() {
        let body = build_request_body(&EP_ANTHROPIC("https://x.com"), "gg wp", "en");
        let v: serde_json::Value = serde_json::from_str(&body).expect("valid json");
        assert_eq!(v["model"], "claude-3-haiku");
        assert!(!v["system"].as_str().unwrap().is_empty());
        assert_eq!(v["messages"].as_array().unwrap().len(), 1);
        assert_eq!(v["messages"][0]["role"], "user");
        assert_eq!(v["messages"][0]["content"], "gg wp");
    }

    /// 回归护栏：聊天内容带引号/换行/emoji 时请求体必须仍是合法 JSON
    /// 且原文一字不改（serde_json::json! 的转义由库保证）。
    #[test]
    fn request_body_survives_hostile_text() {
        let hostile = "he said \"hi\"\nC:\\path\t🌟 中文";
        for ep in [EP_OPENAI("https://x.com"), EP_ANTHROPIC("https://x.com")] {
            let body = build_request_body(&ep, hostile, "zh");
            let v: serde_json::Value = serde_json::from_str(&body).expect("valid json");
            let content = match ep.protocol {
                WireProtocol::OpenAi => v["messages"][1]["content"].clone(),
                WireProtocol::Anthropic => v["messages"][0]["content"].clone(),
            };
            assert_eq!(content.as_str(), Some(hostile));
        }
    }

    #[test]
    fn extract_openai_text() {
        let payload = r#"{"choices":[{"message":{"role":"assistant","content":"好的 "}}]}"#;
        assert_eq!(
            extract_translated_text(WireProtocol::OpenAi, payload),
            Some("好的 ".to_string())
        );
        assert_eq!(extract_translated_text(WireProtocol::OpenAi, r#"{"choices":[]}"#), None);
        assert_eq!(extract_translated_text(WireProtocol::OpenAi, "<html>gateway</html>"), None);
    }

    #[test]
    fn extract_anthropic_text() {
        let payload = r#"{"content":[{"type":"text","text":"好 的"},{"type":"tool_use"}]}"#;
        assert_eq!(
            extract_translated_text(WireProtocol::Anthropic, payload),
            Some("好 的".to_string())
        );
        // 没有 text 块
        assert_eq!(
            extract_translated_text(WireProtocol::Anthropic, r#"{"content":[{"type":"tool_use"}]}"#),
            None
        );
    }

    #[test]
    fn extract_error_message_cases() {
        assert_eq!(
            extract_error_message(r#"{"error":{"message":"Incorrect API key","type":"x"}}"#),
            Some("Incorrect API key".to_string())
        );
        assert_eq!(
            extract_error_message(r#"{"error":"plain string error"}"#),
            Some("plain string error".to_string())
        );
        assert_eq!(extract_error_message("<html>bad gateway</html>"), None);
    }

    #[test]
    fn parse_header_line_cases() {
        let headers = parse_header_line(Some("X-Foo=1; X-Bar = a=b ;;bad; =novalue"));
        assert_eq!(headers.get("X-Foo").map(String::as_str), Some("1"));
        assert_eq!(headers.get("X-Bar").map(String::as_str), Some("a=b"));
        assert_eq!(headers.len(), 2);
        assert!(parse_header_line(None).is_empty());
        assert!(parse_header_line(Some("   ")).is_empty());
    }

    /// S5 模型列表 URL：OpenAI 补版本段，Anthropic 固定 /v1/models。
    #[test]
    fn resolve_models_url_forms() {
        assert_eq!(
            resolve_models_url("https://api.deepseek.com", WireProtocol::OpenAi).unwrap(),
            "https://api.deepseek.com/v1/models"
        );
        assert_eq!(
            resolve_models_url("https://api.deepseek.com/v1/", WireProtocol::OpenAi).unwrap(),
            "https://api.deepseek.com/v1/models"
        );
        assert_eq!(
            resolve_models_url("https://x.com/v2", WireProtocol::OpenAi).unwrap(),
            "https://x.com/v2/models"
        );
        assert_eq!(
            resolve_models_url("https://api.anthropic.com", WireProtocol::Anthropic).unwrap(),
            "https://api.anthropic.com/v1/models"
        );
        assert!(resolve_models_url("  ", WireProtocol::OpenAi).is_err());
    }

    /// /models 响应解析（OpenAI/Anthropic 同形状）：保序、去重、丢空白。
    #[test]
    fn parse_models_payload_cases() {
        let openai = r#"{"object":"list","data":[{"id":"deepseek-chat","object":"model"},{"id":"deepseek-reasoner","object":"model"},{"id":"deepseek-chat","object":"model"},{"id":"  ","object":"model"}]}"#;
        assert_eq!(
            parse_models_payload(openai),
            Some(vec!["deepseek-chat".to_string(), "deepseek-reasoner".to_string()])
        );
        let anthropic = r#"{"data":[{"id":"claude-3-haiku-20240307"},{"id":"claude-3-sonnet-20240229"}]}"#;
        assert_eq!(
            parse_models_payload(anthropic),
            Some(vec![
                "claude-3-haiku-20240307".to_string(),
                "claude-3-sonnet-20240229".to_string()
            ])
        );
        // 形状不对 → None（调用方降级手动填）
        assert_eq!(parse_models_payload(r#"{"data":[]}"#), Some(vec![]));
        assert_eq!(parse_models_payload(r#"{"error":{"message":"no"}}"#), None);
        assert_eq!(parse_models_payload("<html>gateway</html>"), None);
    }

    /// fetch_models 前置校验：无 key 直接 Err 不发网络；坏 URL Err 不 panic。
    #[tokio::test]
    async fn fetch_models_input_validation() {
        let ep = EP_OPENAI("https://x.invalid");
        assert_eq!(
            fetch_models(&ep, None).await,
            Err("未配置 API Key".to_string())
        );
        assert_eq!(
            fetch_models(&ep, Some("   ")).await,
            Err("未配置 API Key".to_string())
        );
        let no_url = TranslationEndpoint {
            id: "t".to_string(),
            base_url: String::new(),
            ..Default::default()
        };
        // 坏 URL：Err 人话，不 panic（也不会真的发网络）
        assert!(fetch_models(&no_url, Some("sk-test")).await.is_err());
    }
}
