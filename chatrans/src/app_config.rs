//! 游戏路径配置（S5）：独立于 translation.json 的 `app.json`。
//!
//! translation.json 是雷达 addon 迁移兼容面（schema 语义 = 翻译配置），
//! 本机环境字段塞进去会让"雷达迁移"与"本机配置"互相污染 —— 游戏路径
//! 独立成 `%LOCALAPPDATA%\KorabliOne\app.json`，天然向后兼容。
//!
//! 落盘纪律与 store 一致：临时文件 + rename 原子替换，坏文件
//! `.broken-时间戳` 隔离留证，读失败一律降级默认值，绝不 panic。

use std::path::{Path, PathBuf};
use std::{fs, io};

use serde::{Deserialize, Serialize};
use tracing::warn;

/// Source 字段取值：手动选择。
pub const SOURCE_MANUAL: &str = "manual";
/// Source 字段取值：注册表探测。
pub const SOURCE_REGISTRY: &str = "registry";
/// Source 字段取值：常见路径扫描。
pub const SOURCE_SCAN: &str = "scan";

/// 游戏根目录有效性校验：目录存在，且（含 `WorldOfWarships.exe`
/// 或已有 `replays/` 子目录）。两者都满足才算"像游戏根目录"。
pub fn is_valid_game_dir(dir: &Path) -> bool {
    if !dir.is_dir() {
        return false;
    }
    dir.join("WorldOfWarships.exe").is_file()
        || dir.join("replays").is_dir()
}

/// 由游戏根目录推导 replays 目录（永远 `{GameDir}\replays`）。
pub fn replays_dir_of(game_dir: &Path) -> PathBuf {
    game_dir.join("replays")
}

/// app.json 的内容。字段 PascalCase，与 translation.json 风格一致。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct AppConfig {
    /// 游戏根目录（不是 replays）。
    pub game_dir: String,
    /// 落盘时固化，读取时以 GameDir 推导为准（历史字段，向后兼容）。
    pub replays_dir: String,
    /// manual | registry | scan（诊断用）。
    pub source: String,
}

impl Default for AppConfig {
    fn default() -> Self {
        AppConfig {
            game_dir: String::new(),
            replays_dir: String::new(),
            source: SOURCE_MANUAL.to_string(),
        }
    }
}

impl AppConfig {
    /// 默认落盘位置：`%LOCALAPPDATA%\KorabliOne\app.json`。
    pub fn default_path() -> PathBuf {
        local_app_data().join("KorabliOne").join("app.json")
    }

    /// 从默认位置加载。读不到 / 解析失败 → 默认值（坏文件隔离留证），
    /// 首次运行（文件不存在）不算错误，静默用默认值。
    pub fn load() -> AppConfig {
        Self::load_from(&Self::default_path())
    }

    /// 可注入版本的加载（测试用）。
    pub fn load_from(path: &Path) -> AppConfig {
        if !path.exists() {
            return AppConfig::default();
        }
        match fs::read_to_string(path) {
            Ok(json) => match serde_json::from_str::<AppConfig>(&json) {
                Ok(parsed) => parsed,
                Err(e) => {
                    warn!("app.json 解析失败（{}），已隔离并用默认配置", e);
                    quarantine(path);
                    AppConfig::default()
                }
            },
            Err(e) => {
                warn!("app.json 读取失败（{}），使用默认配置", e);
                AppConfig::default()
            }
        }
    }

    /// 落盘到默认位置。
    pub fn save(&self) -> io::Result<()> {
        self.save_to(&Self::default_path())
    }

    /// 落盘到指定位置（临时文件 + rename 原子替换，与 store 同套路）。
    pub fn save_to(&self, path: &Path) -> io::Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_string_pretty(self)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        atomic_write(path, json.as_bytes())
    }

    /// 校验后的游戏根目录（None = 未配置或无效）。
    pub fn valid_game_dir(&self) -> Option<PathBuf> {
        let dir = PathBuf::from(self.game_dir.trim());
        if is_valid_game_dir(&dir) {
            Some(dir)
        } else {
            None
        }
    }

    /// 生效的 replays 目录 = `{GameDir}\replays`（以 GameDir 推导为准）。
    pub fn valid_replays_dir(&self) -> Option<PathBuf> {
        self.valid_game_dir().map(|dir| replays_dir_of(&dir))
    }
}

/// 临时文件 + rename 替换（Windows 上 `std::fs::rename` 带
/// MOVEFILE_REPLACE_EXISTING，可原子覆盖已存在的目标）。
fn atomic_write(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let temp = path.with_extension("tmp");
    fs::write(&temp, bytes)?;
    fs::rename(&temp, path)
}

/// 坏文件改名留证：`app.json` → `app.json.broken-20260101235959`。
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

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir()
            .join(format!("chatrans-appcfg-test-{}-{}", std::process::id(), tag));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    /// 读写 round-trip：字段一个不丢。
    #[test]
    fn app_json_round_trip() {
        let dir = temp_dir("round-trip");
        let path = dir.join("app.json");
        let config = AppConfig {
            game_dir: r"D:\Game\Korabli".to_string(),
            replays_dir: r"D:\Game\Korabli\replays".to_string(),
            source: SOURCE_REGISTRY.to_string(),
        };
        config.save_to(&path).expect("save must succeed");
        let loaded = AppConfig::load_from(&path);
        assert_eq!(loaded, config);
        let _ = fs::remove_dir_all(&dir);
    }

    /// 首次运行（文件不存在）：静默默认值，不算错误。
    #[test]
    fn missing_file_gives_default() {
        let dir = temp_dir("missing");
        let loaded = AppConfig::load_from(&dir.join("app.json"));
        assert_eq!(loaded, AppConfig::default());
        assert!(loaded.valid_game_dir().is_none());
        let _ = fs::remove_dir_all(&dir);
    }

    /// 坏 json：隔离成 .broken-时间戳，再用默认值继续跑。
    #[test]
    fn broken_file_is_quarantined() {
        let dir = temp_dir("broken");
        let path = dir.join("app.json");
        fs::write(&path, "{{{ not json").unwrap();
        let loaded = AppConfig::load_from(&path);
        assert_eq!(loaded, AppConfig::default());
        let quarantined: Vec<_> = fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().to_string())
            .filter(|n| n.starts_with("app.json.broken-"))
            .collect();
        assert_eq!(quarantined.len(), 1, "坏文件必须被隔离留证: {:?}", quarantined);
        let _ = fs::remove_dir_all(&dir);
    }

    /// 有效性校验：有 exe 或有 replays/ 都算；空目录/不存在的路径不算。
    #[test]
    fn game_dir_validation_rules() {
        let dir = temp_dir("valid");
        assert!(!is_valid_game_dir(&dir), "空目录不算游戏根目录");
        assert!(!is_valid_game_dir(&dir.join("no-such-dir")));

        // 只有 replays/ 也算（exe 名字随版本变过，replays 是稳定锚点）
        fs::create_dir_all(dir.join("replays")).unwrap();
        assert!(is_valid_game_dir(&dir));

        // 只有 exe 也算
        let dir2 = temp_dir("valid-exe");
        fs::write(dir2.join("WorldOfWarships.exe"), b"MZ").unwrap();
        assert!(is_valid_game_dir(&dir2));

        // replays 目录推导
        assert_eq!(replays_dir_of(Path::new(r"D:\Game\Korabli")), PathBuf::from(r"D:\Game\Korabli\replays"));
        let _ = fs::remove_dir_all(&dir);
        let _ = fs::remove_dir_all(&dir2);
    }

    /// valid_replays_dir：无效 GameDir → None，有效 → {GameDir}\replays。
    #[test]
    fn valid_replays_dir_derivation() {
        let mut config = AppConfig::default();
        assert!(config.valid_replays_dir().is_none());

        let dir = temp_dir("derive");
        fs::create_dir_all(dir.join("replays")).unwrap();
        config.game_dir = dir.to_string_lossy().to_string();
        let replays = config.valid_replays_dir().expect("有效路径应有 replays");
        assert_eq!(replays, dir.join("replays"));
        let _ = fs::remove_dir_all(&dir);
    }
}
