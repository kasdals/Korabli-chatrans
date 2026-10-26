//! 配置与密钥的落盘。
//!
//! 目录沿用方案 C 约定：`%LOCALAPPDATA%\KorabliOne\`
//!   · `translation.json` —— 非敏感项（协议 / base_url / 模型 / 目标语言…）
//!   · `secrets.dat`      —— 整个 key 映射字典 DPAPI 加密后落盘，不出现明文
//!
//! 写盘一律"临时文件 + rename 替换"，坏配置隔离留证（`.broken-时间戳`），
//! 绝不拖垮宿主。注意：`secrets.dat` 的 DPAPI 熵字符串是既有数据的历史
//! 兼容面（见 dpapi::PROTECT_DESCRIPTION），不可更改，否则已存密钥无法解密。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::{fs, io};

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use tracing::warn;

// ── 配置模型（与雷达 `Models.cs` 同 schema，PascalCase 字段名）──────────────

/// 只认两种线协议。落盘写成 PascalCase 字符串（`"OpenAi"` / `"Anthropic"`），
/// 与雷达 `JsonStringEnumConverter` 的输出保持一致；读取时大小写都认。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WireProtocol {
    /// POST {base}/chat/completions，Authorization: Bearer
    OpenAi,
    /// POST {base}/v1/messages，x-api-key + anthropic-version
    Anthropic,
}

impl Serialize for WireProtocol {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let name = match self {
            WireProtocol::OpenAi => "OpenAi",
            WireProtocol::Anthropic => "Anthropic",
        };
        serializer.serialize_str(name)
    }
}

impl<'de> Deserialize<'de> for WireProtocol {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        match s.to_ascii_lowercase().as_str() {
            "openai" | "open_ai" | "open-ai" => Ok(WireProtocol::OpenAi),
            "anthropic" => Ok(WireProtocol::Anthropic),
            other => Err(serde::de::Error::custom(format!("未知协议: {:?}", other))),
        }
    }
}

/// 一个可翻译的供应商。密钥不在这个对象里，只存 [`secret_key`] 指向保险库条目。
///
/// 字段是雷达 schema 的超集：`Name` / `Enabled` 是方案 C 新增（有默认值），
/// 雷达多出来的 `DisplayName` / `Active` 死数据会被 serde 忽略，两个方向都兼容。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct TranslationEndpoint {
    /// 内部标识，用于日志与"当前供应商"引用；不会发给服务端。
    pub id: String,
    /// 下拉框里显示的名字（方案 C 新增；雷达文件里没有时默认为空）。
    pub name: String,
    pub protocol: WireProtocol,
    pub base_url: String,
    pub model: String,
    /// 供应商级开关（方案 C 新增；雷达文件里没有时默认开）。
    pub enabled: bool,
    /// 对局中要快，默认 10 秒（不是聊天客户端的 120 秒）。
    pub timeout_seconds: u64,
    /// 形如 http://127.0.0.1:7890，只对本供应商生效；留空表示直连。
    pub proxy: String,
    /// 自定义请求头（合并到默认头之后，同名会覆盖）。
    pub extra_headers: BTreeMap<String, String>,
    /// 模型列表（S5 新增；行开关 + 首个 enabled 写回 `model`）。
    pub models: Vec<EndpointModel>,
}

impl Default for TranslationEndpoint {
    fn default() -> Self {
        TranslationEndpoint {
            id: String::new(),
            name: String::new(),
            protocol: WireProtocol::OpenAi,
            base_url: String::new(),
            model: String::new(),
            enabled: true,
            timeout_seconds: 10,
            proxy: String::new(),
            extra_headers: BTreeMap::new(),
            models: Vec::new(),
        }
    }
}

impl TranslationEndpoint {
    /// 密钥在保险库里的条目名（与雷达 `TranslationEndpoint.SecretKey` 一致）。
    pub fn secret_key(&self) -> String {
        secret_key(&self.id)
    }

    /// 当前生效模型 = 首个 enabled 模型的 id（S5 Enabled 映射语义）。
    /// 模型列表为空（雷达旧文件）或全部关闭 → None。
    pub fn effective_model(&self) -> Option<String> {
        self.models
            .iter()
            .find(|m| m.enabled)
            .map(|m| m.id.clone())
    }

    /// Enabled→model 写回（保存配置时调用）：首个 enabled 模型的 id 写回
    /// `endpoint.model`；全部关闭则 model 保持原值不动（翻译链路已有
    /// "无 key/无模型不发请求"兜底）。
    pub fn apply_enabled_model(&mut self) {
        if let Some(id) = self.effective_model() {
            self.model = id;
        }
    }
}

/// 供应商模型列表的一行（S5 新增，雷达 schema 超集；旧文件无 `Models`
/// 字段 → serde default 空 vec，行为不变）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "PascalCase", default)]
pub struct EndpointModel {
    /// 模型 id（发给服务端的 model 参数值）。
    pub id: String,
    /// 行开关；首个 enabled 的模型即"当前生效模型"。
    pub enabled: bool,
}

impl Default for EndpointModel {
    fn default() -> Self {
        EndpointModel {
            id: String::new(),
            enabled: true,
        }
    }
}

/// 密钥条目名：`ep:{id}`，空 id 归到 `default`。与雷达实现逐字一致。
pub fn secret_key(id: &str) -> String {
    let trimmed = id.trim();
    if trimmed.is_empty() {
        "ep:default".to_string()
    } else {
        format!("ep:{}", trimmed)
    }
}

/// 附加能力的全部配置。与雷达 `TranslationConfig` 同 schema。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct TranslationConfig {
    /// 是否启用对局内翻译。
    pub enabled: bool,
    pub target_language: String,
    /// 频道范围：`true` = 只翻译队伍频道，`false` = 翻译所有聊天频道。
    ///
    /// 存储语义（与雷达一致，字段名冻结）默认 `false`；界面上是**反相开关**
    /// 「翻译所有聊天（全体聊天）」，默认勾选 —— 勾选 = 这里为 false。
    pub team_only: bool,
    pub active_endpoint_id: String,
    pub endpoints: Vec<TranslationEndpoint>,
}

impl Default for TranslationConfig {
    fn default() -> Self {
        let mut cfg = TranslationConfig {
            enabled: false,
            target_language: "zh".to_string(),
            // 默认翻译所有聊天频道（界面上的「翻译所有聊天」默认勾选）
            team_only: false,
            active_endpoint_id: String::new(),
            endpoints: Vec::new(),
        };
        cfg.ensure_defaults();
        cfg
    }
}

impl TranslationConfig {
    /// 空白供应商卡片（第 `slot` 张；id 形如 `endpoint1`）。
    /// 新用户打开设置时直接看到 API Key 表单，而不是只有「添加更多」按钮。
    pub fn blank_endpoint(slot: usize) -> TranslationEndpoint {
        TranslationEndpoint {
            id: format!("endpoint{}", slot + 1),
            name: format!("模型供应商 {}", slot + 1),
            ..TranslationEndpoint::default()
        }
    }

    /// 最小可用性兜底（幂等）：端点列表为空时补一张空白供应商卡片；
    /// 生效供应商未指定时指向第一个端点。endpoints 非空时原样。
    pub fn ensure_defaults(&mut self) {
        if self.endpoints.is_empty() {
            let ep = Self::blank_endpoint(0);
            self.active_endpoint_id = ep.id.clone();
            self.endpoints.push(ep);
        } else if self.active_endpoint_id.is_empty() {
            self.active_endpoint_id = self.endpoints[0].id.clone();
        }
    }
}

impl TranslationConfig {
    /// 当前生效的供应商（语义同雷达 `TranslationConfig.Active`：
    /// 先按 id 找，找不到就退回第一个）。
    pub fn active(&self) -> Option<&TranslationEndpoint> {
        self.endpoints
            .iter()
            .find(|e| e.id.eq_ignore_ascii_case(&self.active_endpoint_id))
            .or_else(|| self.endpoints.first())
    }
}

// ── DPAPI 最小封装（手写 FFI，与雷达 Dpapi.cs 同姿势，不引大依赖）───────────

#[cfg(windows)]
mod dpapi {
    use std::ptr;

    const PROTECT_DESCRIPTION: &str = "TomatoRadar.Translation";

    #[repr(C)]
    struct CryptBlob {
        cb_data: u32,
        pb_data: *mut u8,
    }

    extern "system" {
        fn CryptProtectData(
            p_data_in: *mut CryptBlob,
            sz_data_descr: *const u16,
            p_optional_entropy: *mut CryptBlob,
            pv_reserved: *mut std::ffi::c_void,
            p_prompt_struct: *mut std::ffi::c_void,
            dw_flags: u32,
            p_data_out: *mut CryptBlob,
        ) -> i32;
        fn CryptUnprotectData(
            p_data_in: *const CryptBlob,
            ppsz_data_descr: *mut *mut u16,
            p_optional_entropy: *const CryptBlob,
            pv_reserved: *const std::ffi::c_void,
            p_prompt_struct: *const std::ffi::c_void,
            dw_flags: u32,
            p_data_out: *mut CryptBlob,
        ) -> i32;
        fn LocalFree(h_mem: *mut std::ffi::c_void) -> *mut std::ffi::c_void;
    }

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    fn blob_of(data: &[u8]) -> CryptBlob {
        // FFI 只读这块内存，转成裸指针避免可变引用别名问题
        CryptBlob {
            cb_data: data.len() as u32,
            pb_data: data.as_ptr() as *mut u8,
        }
    }

    fn take_blob(blob: CryptBlob) -> Vec<u8> {
        unsafe {
            let out = if blob.pb_data.is_null() || blob.cb_data == 0 {
                Vec::new()
            } else {
                std::slice::from_raw_parts(blob.pb_data, blob.cb_data as usize).to_vec()
            };
            if !blob.pb_data.is_null() {
                LocalFree(blob.pb_data as *mut _);
            }
            out
        }
    }

    /// DPAPI CurrentUser 加密。空输入 → 空输出（与雷达 Dpapi.Protect 一致）。
    pub fn protect(plain: &[u8]) -> Result<Vec<u8>, String> {
        if plain.is_empty() {
            return Ok(Vec::new());
        }
        let desc = wide(PROTECT_DESCRIPTION);
        let mut input = blob_of(plain);
        let mut output = CryptBlob {
            cb_data: 0,
            pb_data: ptr::null_mut(),
        };
        let ok = unsafe {
            CryptProtectData(
                &mut input,
                desc.as_ptr(),
                ptr::null_mut(),
                ptr::null_mut(),
                ptr::null_mut(),
                0,
                &mut output,
            )
        };
        if ok == 0 {
            return Err(format!(
                "CryptProtectData failed: {}",
                std::io::Error::last_os_error()
            ));
        }
        Ok(take_blob(output))
    }

    /// DPAPI 解密。换用户/换机器解不开是正常情况，由调用方决定当作"没有密钥"。
    pub fn unprotect(cipher: &[u8]) -> Result<Vec<u8>, String> {
        if cipher.is_empty() {
            return Ok(Vec::new());
        }
        let mut input = blob_of(cipher);
        let mut output = CryptBlob {
            cb_data: 0,
            pb_data: ptr::null_mut(),
        };
        let mut descr: *mut u16 = ptr::null_mut();
        let ok = unsafe {
            CryptUnprotectData(
                &mut input,
                &mut descr,
                ptr::null(),
                ptr::null(),
                ptr::null(),
                0,
                &mut output,
            )
        };
        // 描述串由系统分配，无论成败都要释放（雷达 C# 实现漏了这步，这里补上）
        unsafe {
            if !descr.is_null() {
                LocalFree(descr as *mut _);
            }
        }
        if ok == 0 {
            return Err(format!(
                "CryptUnprotectData failed: {}",
                std::io::Error::last_os_error()
            ));
        }
        Ok(take_blob(output))
    }
}

// ── Store ───────────────────────────────────────────────────────────────────

/// 配置与密钥的持有者。GUI / 调度方共用同一个实例。
pub struct TranslationStore {
    directory: PathBuf,
    config: TranslationConfig,
    secrets: BTreeMap<String, String>,
}

impl TranslationStore {
    /// 方案 C 的主配置目录：`%LOCALAPPDATA%\KorabliOne\`。
    pub fn default_directory() -> PathBuf {
        local_app_data().join("KorabliOne")
    }

    /// 从默认位置加载：只读 KorabliOne（`%LOCALAPPDATA%\KorabliOne\`）。
    /// 写盘同目录。
    pub fn load() -> TranslationStore {
        let primary = Self::default_directory();
        Self::load_with(
            &[primary.clone()],
            &[primary.clone()],
            primary,
        )
    }

    /// 可注入版本的加载（测试用）：配置取第一个存在 `translation.json` 的目录，
    /// 密钥取第一个存在 `secrets.dat` 的目录，两边互不绑定。
    pub fn load_with(
        config_candidates: &[PathBuf],
        secrets_candidates: &[PathBuf],
        write_directory: PathBuf,
    ) -> TranslationStore {
        let mut config = TranslationConfig::default();
        for dir in config_candidates {
            let path = dir.join("translation.json");
            if !path.exists() {
                continue;
            }
            match fs::read_to_string(&path) {
                Ok(json) => match serde_json::from_str::<TranslationConfig>(&json) {
                    Ok(parsed) => {
                        config = parsed;
                    }
                    Err(e) => {
                        // 配置坏了不要拖垮宿主：把坏文件改名留证，再用默认值继续
                        warn!("translation.json 解析失败（{}），已隔离并用默认配置", e);
                        quarantine(&path);
                    }
                },
                Err(e) => warn!("translation.json 读取失败（{}），使用默认配置", e),
            }
            break;
        }

        // 空端点兜底（幂等）：文件不存在 / 文件里删光了供应商，都保证
        // 设置页至少有一张空白卡片可填（见 ensure_defaults 注释）。
        config.ensure_defaults();

        let secrets = load_secrets_from(secrets_candidates);

        TranslationStore {
            directory: write_directory,
            config,
            secrets,
        }
    }

    /// 在指定目录新建一个空 store（测试 / 首次落盘用）。
    pub fn new_in(directory: PathBuf) -> TranslationStore {
        TranslationStore {
            directory,
            config: TranslationConfig::default(),
            secrets: BTreeMap::new(),
        }
    }

    pub fn data_directory(&self) -> &Path {
        &self.directory
    }

    pub fn config_path(&self) -> PathBuf {
        self.directory.join("translation.json")
    }

    pub fn secrets_path(&self) -> PathBuf {
        self.directory.join("secrets.dat")
    }

    pub fn config(&self) -> &TranslationConfig {
        &self.config
    }

    pub fn config_mut(&mut self) -> &mut TranslationConfig {
        &mut self.config
    }

    /// 配置 + 密钥一起落盘。
    pub fn save(&mut self) -> io::Result<()> {
        fs::create_dir_all(&self.directory)?;

        let json = serde_json::to_string_pretty(&self.config)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        atomic_write(&self.config_path(), json.as_bytes())?;

        // 密钥为空时不留文件（与雷达 SaveSecrets 一致：删掉 = 完全卸载密钥）
        if self.secrets.is_empty() {
            if self.secrets_path().exists() {
                fs::remove_file(self.secrets_path())?;
            }
        } else {
            let plain = serde_json::to_vec(&self.secrets)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
            #[cfg(windows)]
            let cipher = dpapi::protect(&plain).map_err(io::Error::other)?;
            #[cfg(not(windows))]
            let cipher = {
                // 非 Windows 平台没有 DPAPI：明文兜底只用于开发/CI，
                // 文件权限语义由调用方自行保证（本项目只在 Windows 上发布）。
                warn!("non-Windows platform: secrets.dat is written WITHOUT encryption");
                plain
            };
            atomic_write(&self.secrets_path(), &cipher)?;
        }
        Ok(())
    }

    // ── 密钥 ────────────────────────────────────────────────────────────────

    pub fn has_secret(&self, key_ref: &str) -> bool {
        self.secrets
            .get(key_ref)
            .map(|v| !v.is_empty())
            .unwrap_or(false)
    }

    pub fn get_secret(&self, key_ref: &str) -> Option<&str> {
        self.secrets.get(key_ref).map(|v| v.as_str())
    }

    /// 设置密钥；传 `None` / 空串表示删除（与雷达 `SetSecret` 语义一致）。
    pub fn set_secret(&mut self, key_ref: &str, value: Option<&str>) {
        match value {
            Some(v) if !v.is_empty() => {
                self.secrets.insert(key_ref.to_string(), v.to_string());
            }
            _ => {
                self.secrets.remove(key_ref);
            }
        }
    }
}

/// 从候选目录读密钥库：第一个存在 `secrets.dat` 的目录生效。
/// 解不开（换用户/换机器）是正常情况：当作"没有密钥"，不把程序弄崩。
fn load_secrets_from(candidates: &[PathBuf]) -> BTreeMap<String, String> {
    for dir in candidates {
        let path = dir.join("secrets.dat");
        if !path.exists() {
            continue;
        }
        let cipher = match fs::read(&path) {
            Ok(bytes) => bytes,
            Err(e) => {
                warn!("secrets.dat 读取失败（{}）", e);
                return BTreeMap::new();
            }
        };
        if cipher.is_empty() {
            return BTreeMap::new();
        }
        #[cfg(windows)]
        let plain = match dpapi::unprotect(&cipher) {
            Ok(bytes) => bytes,
            Err(e) => {
                warn!("secrets.dat DPAPI 解密失败（{}），当作没有密钥", e);
                return BTreeMap::new();
            }
        };
        #[cfg(not(windows))]
        let plain = cipher;
        return match serde_json::from_slice::<BTreeMap<String, String>>(&plain) {
            Ok(map) => map,
            Err(e) => {
                warn!("secrets.dat 内容不是合法的密钥映射（{}），当作没有密钥", e);
                BTreeMap::new()
            }
        };
    }
    BTreeMap::new()
}

/// 临时文件 + rename 替换（Windows 上 `std::fs::rename` 带
/// MOVEFILE_REPLACE_EXISTING，可原子覆盖已存在的目标）。
fn atomic_write(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let temp = path.with_extension("tmp");
    fs::write(&temp, bytes)?;
    fs::rename(&temp, path)
}

/// 坏文件改名留证：`translation.json` → `translation.json.broken-20260101235959`。
fn quarantine(path: &Path) {
    let stamp = chrono::Local::now().format("%Y%m%d%H%M%S");
    let broken = path.with_file_name(format!(
        "{}.broken-{}",
        path.file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default(),
        stamp
    ));
    if let Err(e) = fs::rename(path, &broken) {
        // 留证失败不影响主流程
        warn!("隔离坏配置失败（{}）: {:?}", e, path);
    }
}

/// `%LOCALAPPDATA%`。取不到时退回临时目录（仅在异常环境下发生，
/// 保证"永不 panic"）。
fn local_app_data() -> PathBuf {
    match std::env::var_os("LOCALAPPDATA") {
        Some(v) if !v.is_empty() => PathBuf::from(v),
        _ => {
            warn!("LOCALAPPDATA is not set; falling back to the temp directory");
            std::env::temp_dir()
        }
    }
}

// ── 测试 ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// 每个用例一个独立临时目录，避免相互污染。
    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir()
            .join(format!("chatrans-store-test-{}-{}", std::process::id(), tag));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    fn sample_config() -> TranslationConfig {
        let mut ep1 = TranslationEndpoint {
            id: "endpoint1".to_string(),
            name: "DeepSeek".to_string(),
            base_url: "https://api.deepseek.com/v1".to_string(),
            model: "deepseek-chat".to_string(),
            ..Default::default()
        };
        ep1.extra_headers.insert("X-Foo".to_string(), "bar".to_string());
        let ep2 = TranslationEndpoint {
            id: "endpoint2".to_string(),
            protocol: WireProtocol::Anthropic,
            base_url: "https://api.anthropic.com".to_string(),
            model: "claude-3-haiku".to_string(),
            ..Default::default()
        };
        TranslationConfig {
            enabled: true,
            target_language: "zh".to_string(),
            team_only: false,
            active_endpoint_id: "endpoint2".to_string(),
            endpoints: vec![ep1, ep2],
        }
    }

    /// json 读写 round-trip：字段一个不丢，serde 大小写别名也认。
    #[test]
    fn config_json_round_trip() {
        let dir = temp_dir("round-trip");
        let mut store = TranslationStore::new_in(dir.clone());
        *store.config_mut() = sample_config();
        store.set_secret("ep:endpoint1", Some("sk-test-123"));
        store.save().expect("save must succeed");

        let loaded = TranslationStore::load_with(&[dir.clone()], &[dir.clone()], dir.clone());
        assert_eq!(loaded.config().enabled, true);
        assert_eq!(loaded.config().team_only, false);
        assert_eq!(loaded.config().target_language, "zh");
        assert_eq!(loaded.config().endpoints.len(), 2);
        let ep1 = &loaded.config().endpoints[0];
        assert_eq!(ep1.id, "endpoint1");
        assert_eq!(ep1.model, "deepseek-chat");
        assert_eq!(ep1.extra_headers.get("X-Foo").map(String::as_str), Some("bar"));
        let ep2 = &loaded.config().endpoints[1];
        assert_eq!(ep2.protocol, WireProtocol::Anthropic);
        assert_eq!(loaded.config().active_endpoint_id, "endpoint2");
        assert_eq!(
            loaded.config().active().map(|e| e.id.as_str()),
            Some("endpoint2")
        );
        assert_eq!(loaded.get_secret("ep:endpoint1"), Some("sk-test-123"));
        let _ = fs::remove_dir_all(&dir);
    }

    /// 旧版落盘的真实形态（含 DisplayName / Active 死数据、PascalCase 协议）
    /// 必须能被读进来 —— 配置 schema 的历史兼容承诺。
    #[test]
    fn legacy_json_with_dead_fields_parses() {
        let json = r#"{
  "Enabled": true,
  "TargetLanguage": "zh",
  "TeamOnly": false,
  "ActiveEndpointId": "endpoint1",
  "Endpoints": [
    {
      "Id": "endpoint1",
      "Protocol": "OpenAi",
      "BaseUrl": "https://api.deepseek.com/v1",
      "Model": "deepseek-flash",
      "TimeoutSeconds": 10,
      "Proxy": "",
      "ExtraHeaders": {},
      "DisplayName": "endpoint1"
    }
  ],
  "Active": {
    "Id": "endpoint1",
    "Protocol": "OpenAi",
    "BaseUrl": "https://api.deepseek.com/v1",
    "Model": "deepseek-flash",
    "TimeoutSeconds": 10,
    "Proxy": "",
    "ExtraHeaders": {},
    "DisplayName": "endpoint1"
  }
}"#;
        let config: TranslationConfig = serde_json::from_str(json).expect("must parse legacy json");
        assert_eq!(config.endpoints.len(), 1);
        assert_eq!(config.endpoints[0].model, "deepseek-flash");
        assert_eq!(config.endpoints[0].timeout_seconds, 10);
        assert_eq!(config.active().map(|e| e.id.as_str()), Some("endpoint1"));
    }

    /// DPAPI round-trip + 密文不含明文（R4 自测清单）。
    #[cfg(windows)]
    #[test]
    fn dpapi_protect_unprotect_round_trip() {
        let plain = br#"{"ep:endpoint1":"sk-very-secret-value"}"#;
        let cipher = dpapi::protect(plain).expect("protect");
        assert!(!cipher.is_empty());
        assert_ne!(&cipher[..], &plain[..]);
        // 密文里不允许出现明文片段
        let cipher_str = String::from_utf8_lossy(&cipher);
        assert!(!cipher_str.contains("sk-very-secret-value"));

        let recovered = dpapi::unprotect(&cipher).expect("unprotect");
        assert_eq!(recovered, plain.to_vec());

        // 空输入直通（与 C# 一致）
        assert!(dpapi::protect(b"").unwrap().is_empty());
        assert!(dpapi::unprotect(b"").unwrap().is_empty());
    }

    /// 换了密钥/坏了的密文：解密失败必须优雅降级为"没有密钥"，不 panic。
    #[cfg(windows)]
    #[test]
    fn corrupted_secrets_degrade_to_empty() {
        let dir = temp_dir("bad-secrets");
        fs::write(dir.join("secrets.dat"), b"this is not a dpapi blob").unwrap();
        let store = TranslationStore::load_with(&[dir.clone()], &[dir.clone()], dir.clone());
        assert!(store.secrets.is_empty());
        let _ = fs::remove_dir_all(&dir);
    }

    /// 坏 json：隔离成 .broken-时间戳，再用默认配置继续跑。
    #[test]
    fn broken_config_is_quarantined() {
        let dir = temp_dir("broken-json");
        fs::write(dir.join("translation.json"), "{{{ not json").unwrap();
        let store = TranslationStore::load_with(&[dir.clone()], &[dir.clone()], dir.clone());
        // 坏文件 → 默认配置 + 空端点兜底（恰好一张空白供应商卡片）
        assert_eq!(store.config().endpoints.len(), 1);
        assert_eq!(store.config().endpoints[0].id, "endpoint1");
        assert_eq!(store.config().enabled, false);
        let quarantined: Vec<_> = fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().to_string())
            .filter(|n| n.starts_with("translation.json.broken-"))
            .collect();
        assert_eq!(quarantined.len(), 1, "坏文件必须被隔离留证: {:?}", quarantined);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn secret_key_format() {
        assert_eq!(secret_key("endpoint1"), "ep:endpoint1");
        assert_eq!(secret_key(""), "ep:default");
        assert_eq!(secret_key("  "), "ep:default");
    }

    #[test]
    fn set_secret_none_removes_entry() {
        let mut store = TranslationStore::new_in(PathBuf::from("."));
        store.set_secret("ep:a", Some("v"));
        assert!(store.has_secret("ep:a"));
        store.set_secret("ep:a", None);
        assert!(!store.has_secret("ep:a"));
        store.set_secret("ep:a", Some(""));
        assert!(!store.has_secret("ep:a"));
    }

    /// 默认配置预置一张空白供应商卡片：新用户打开设置直接看到
    /// API Key 表单（而不是只有「添加更多」按钮）。
    #[test]
    fn default_config_has_one_blank_endpoint() {
        let cfg = TranslationConfig::default();
        assert_eq!(cfg.endpoints.len(), 1);
        assert_eq!(cfg.endpoints[0].id, "endpoint1");
        assert_eq!(cfg.endpoints[0].name, "模型供应商 1");
        assert!(cfg.endpoints[0].base_url.is_empty());
        assert!(cfg.endpoints[0].model.is_empty());
        assert!(cfg.endpoints[0].enabled);
        assert_eq!(cfg.active_endpoint_id, "endpoint1");
    }

    /// load 兜底：文件里把供应商删光（endpoints 空数组）→ 补一张空白卡；
    /// 幂等 —— 已有端点时原样保留。
    #[test]
    fn load_empty_endpoints_gets_blank_fallback() {
        let dir = temp_dir("blank-fallback");
        fs::write(
            dir.join("translation.json"),
            r#"{"Enabled":false,"TargetLanguage":"zh","TeamOnly":false,"ActiveEndpointId":"","Endpoints":[]}"#,
        )
        .unwrap();
        let store = TranslationStore::load_with(&[dir.clone()], &[dir.clone()], dir.clone());
        assert_eq!(store.config().endpoints.len(), 1);
        assert_eq!(store.config().endpoints[0].id, "endpoint1");
        assert_eq!(store.config().active_endpoint_id, "endpoint1");
        let _ = fs::remove_dir_all(&dir);
    }

    /// load 兜底幂等：文件里已有 N 个端点时原样（不重复插入空白卡）。
    #[test]
    fn load_preserves_existing_endpoints() {
        let dir = temp_dir("blank-preserves");
        fs::write(
            dir.join("translation.json"),
            r#"{"Enabled":true,"TargetLanguage":"zh","TeamOnly":false,"ActiveEndpointId":"ep-a","Endpoints":[{"Id":"ep-a","Protocol":"Anthropic","BaseUrl":"https://api.anthropic.com","Model":"claude-3-haiku","TimeoutSeconds":10,"Proxy":"","ExtraHeaders":{}}]}"#,
        )
        .unwrap();
        let store = TranslationStore::load_with(&[dir.clone()], &[dir.clone()], dir.clone());
        assert_eq!(store.config().endpoints.len(), 1);
        assert_eq!(store.config().endpoints[0].id, "ep-a");
        assert_eq!(store.config().active_endpoint_id, "ep-a");
        let _ = fs::remove_dir_all(&dir);
    }

    /// S5 Models 字段：round-trip 不丢字段；PascalCase 落盘形态正确。
    #[test]
    fn endpoint_models_round_trip() {
        let mut ep = TranslationEndpoint {
            id: "ep-m".to_string(),
            model: "deepseek-chat".to_string(),
            ..Default::default()
        };
        ep.models = vec![
            EndpointModel { id: "m-a".to_string(), enabled: false },
            EndpointModel { id: "m-b".to_string(), enabled: true },
            EndpointModel { id: "m-c".to_string(), enabled: false },
        ];
        let json = serde_json::to_string(&ep).unwrap();
        assert!(json.contains("\"Models\":"), "落盘字段名是 PascalCase");
        assert!(json.contains("\"Id\":\"m-b\""));

        let parsed: TranslationEndpoint = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.models.len(), 3);
        assert_eq!(parsed.effective_model(), Some("m-b".to_string()));
    }

    /// 旧版文件（无 Models 字段）解析不受影响：models 空 →
    /// effective_model None → 模型区显示"自定义模型：{model}"，行为不变。
    #[test]
    fn legacy_json_without_models_field_parses() {
        let json = r#"{"Id":"ep-old","Protocol":"OpenAi","BaseUrl":"https://x.com","Model":"old-model","TimeoutSeconds":10,"Proxy":"","ExtraHeaders":{}}"#;
        let ep: TranslationEndpoint = serde_json::from_str(json).expect("旧文件必须能解析");
        assert!(ep.models.is_empty());
        assert_eq!(ep.model, "old-model");
        assert_eq!(ep.effective_model(), None);
    }

    /// Enabled→model 写回语义（主理人裁决 2）：
    /// 首个 enabled 模型写回 endpoint.model；全部关闭则 model 原值不动。
    #[test]
    fn apply_enabled_model_writeback_semantics() {
        let mut ep = TranslationEndpoint {
            id: "ep-w".to_string(),
            model: "original-model".to_string(),
            ..Default::default()
        };
        // 全部关闭：model 原值不动
        ep.models = vec![
            EndpointModel { id: "m-a".to_string(), enabled: false },
        ];
        ep.apply_enabled_model();
        assert_eq!(ep.model, "original-model");

        // 空 models：同样原值不动
        ep.models.clear();
        ep.apply_enabled_model();
        assert_eq!(ep.model, "original-model");

        // 有 enabled：写回首个 enabled 的 id（不是第一个模型）
        ep.models = vec![
            EndpointModel { id: "m-a".to_string(), enabled: false },
            EndpointModel { id: "m-b".to_string(), enabled: true },
            EndpointModel { id: "m-c".to_string(), enabled: true },
        ];
        ep.apply_enabled_model();
        assert_eq!(ep.model, "m-b");
    }

    /// 界面用词从「端点」改成「模型供应商」时，**配置 schema 一个字都不许动**：
    /// 这几个 PascalCase 键名是既有 schema，改了老配置就废了。
    /// 谁要是把 Rust 字段跟着改名，这条测试会立刻红。
    #[test]
    fn config_schema_keys_are_frozen() {
        let config = TranslationConfig {
            enabled: true,
            target_language: "zh".to_string(),
            team_only: true,
            active_endpoint_id: "ep1".to_string(),
            endpoints: vec![TranslationEndpoint {
                id: "ep1".to_string(),
                name: "DeepSeek".to_string(),
                base_url: "https://api.deepseek.com/v1".to_string(),
                model: "deepseek-chat".to_string(),
                ..Default::default()
            }],
        };
        let json = serde_json::to_string(&config).expect("serialize");
        for key in [
            "\"Endpoints\"",
            "\"ActiveEndpointId\"",
            "\"TargetLanguage\"",
            "\"TeamOnly\"",
            "\"BaseUrl\"",
            "\"TimeoutSeconds\"",
            "\"ExtraHeaders\"",
            "\"Models\"",
        ] {
            assert!(json.contains(key), "配置缺少兼容键 {key}: {json}");
        }

        // 反向：一份「雷达/旧版」文件（没有 Name / Models）仍能读进来
        let legacy = r#"{
            "Enabled": true,
            "TargetLanguage": "zh",
            "TeamOnly": false,
            "ActiveEndpointId": "old",
            "Endpoints": [{
                "Id": "old",
                "Protocol": "OpenAi",
                "BaseUrl": "https://example.com/v1",
                "Model": "m",
                "DisplayName": "旧版遗留数据"
            }]
        }"#;
        let parsed: TranslationConfig = serde_json::from_str(legacy).expect("旧配置要能读");
        let active = parsed.active().expect("应有生效供应商");
        assert_eq!(active.id, "old");
        assert_eq!(active.name, "", "Name 缺失时为空，界面回退显示 id");
        assert!(active.enabled, "Enabled 缺失时默认开");
    }
}
