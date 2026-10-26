//! 游戏目录自动发现（S5）：注册表探测 → 常见路径扫描 → 校验。
//!
//! 注册表键名/键值各家客户端（Lesta 国服 / Wargaming.net Game Center）
//! 没有公开契约，探不到是正常情况 —— 静默降级到常见路径扫描，都不行就
//! 返回 None 交给 UI 引导用户手动选（rfd 目录对话框，gui 侧接线）。
//!
//! 可测性设计：注册表读取与候选根目录全部注入，核心
//! [`discover_with`] 是纯函数，单测不碰真实注册表/文件系统
//! （真实注册表路径的真实实现只走 `#[cfg(all(windows, feature = "gui"))]`）。

use std::path::PathBuf;

use crate::app_config::{is_valid_game_dir, AppConfig, SOURCE_REGISTRY, SOURCE_SCAN};

/// 注册表探测候选：`(键路径, 值名)`，按优先级排序。探不到静默降级。
const REGISTRY_CANDIDATES: &[(&str, &str)] = &[
    // Lesta Game Center（国服）
    (r"Software\Lesta Games\Game Center", "install_path"),
    (r"Software\Lesta Games\GameCenter", "install_path"),
    // Wargaming.net Game Center（国际服）
    (r"Software\Wargaming.net\Game Center", "install_path"),
    (r"Software\Wargaming.net\GameCenter", "install_path"),
];

/// 常见路径扫描候选（{D..G} 盘 × 三种习惯命名），按优先级排序。
fn scan_candidates() -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    for letter in ['D', 'E', 'F', 'G'] {
        let drive = format!("{}:", letter);
        for suffix in [r"\Game\Korabli", r"\Games\Korabli", r"\Korabli"] {
            candidates.push(PathBuf::from(format!("{}{}", drive, suffix)));
        }
    }
    candidates
}

/// 一次发现的结论。`source` 直接落进 [`AppConfig::source`]（诊断用）。
#[derive(Debug, Clone, PartialEq)]
pub struct DiscoveryResult {
    pub game_dir: PathBuf,
    pub source: String,
}

impl DiscoveryResult {
    fn new(game_dir: PathBuf, source: &str) -> DiscoveryResult {
        DiscoveryResult {
            game_dir,
            source: source.to_string(),
        }
    }
}

/// 注册表读取抽象（注入点）：读 `(键路径, 值名)` 对应的字符串值，
/// 读不到（键不存在/类型不对/权限）返回 None。
pub type RegistryReader<'a> = &'a dyn Fn(&str, &str) -> Option<String>;

/// 真实发现入口：注册表 → 扫描。什么都不行 → None（UI 引导手动选）。
pub fn discover() -> Option<DiscoveryResult> {
    #[cfg(all(windows, feature = "gui"))]
    {
        let reader = |key: &str, value: &str| -> Option<String> {
            use winreg::enums::HKEY_CURRENT_USER;
            use winreg::RegKey;
            let hkcu = RegKey::predef(HKEY_CURRENT_USER);
            let subkey = hkcu.open_subkey(key).ok()?;
            subkey.get_value(value).ok()
        };
        if let Some(found) = discover_with(&reader, &scan_candidates()) {
            return Some(found);
        }
    }
    // 非 gui 构建（没有 winreg）只做扫描 —— console 模式不依赖自动发现。
    #[cfg(not(all(windows, feature = "gui")))]
    {
        let no_registry: RegistryReader = &|_, _| None;
        if let Some(found) = discover_with(no_registry, &scan_candidates()) {
            return Some(found);
        }
    }
    None
}

/// 纯函数核心（测试入口）：先按候选清单问注册表，再扫根目录。
/// 每个候选都过 [`is_valid_game_dir`] 校验，首个通过者胜出。
pub fn discover_with(registry: RegistryReader<'_>, roots: &[PathBuf]) -> Option<DiscoveryResult> {
    // 1) 注册表：注册表给的是"安装根"，可能就是游戏根目录，也可能多一层
    for (key, value) in REGISTRY_CANDIDATES {
        if let Some(path) = registry(key, value) {
            let trimmed = path.trim().trim_matches('"').trim();
            if trimmed.is_empty() {
                continue;
            }
            let dir = PathBuf::from(trimmed);
            if is_valid_game_dir(&dir) {
                return Some(DiscoveryResult::new(dir, SOURCE_REGISTRY));
            }
        }
    }

    // 2) 常见路径扫描
    for dir in roots {
        if is_valid_game_dir(dir) {
            return Some(DiscoveryResult::new(dir.clone(), SOURCE_SCAN));
        }
    }

    None
}

/// 把发现结果固化进配置（source 一并记录；GameDir 为空才接受，
/// 用户已手动配置过的路径不被自动发现覆盖）。
pub fn apply_to_config(config: &mut AppConfig, found: &DiscoveryResult) {
    if !config.game_dir.trim().is_empty() {
        return;
    }
    config.game_dir = found.game_dir.to_string_lossy().to_string();
    config.replays_dir = crate::app_config::replays_dir_of(&found.game_dir)
        .to_string_lossy()
        .to_string();
    config.source = found.source.clone();
}

// ── 后台任务（S5 T04：UI 线程零阻塞）────────────────────────────────────

/// 原生目录选择对话框（rfd）。**必须后台线程跑**：同步 IFileDialog 会
/// 冻结消息循环，UI 卡到用户选完为止（设计文档第七节风险 4）。
/// 结果（None = 用户取消）经 channel 回传，调用方每帧 poll。
#[cfg(feature = "gui")]
pub fn spawn_dir_picker(
    tx: std::sync::mpsc::Sender<Option<String>>,
) -> std::thread::JoinHandle<()> {
    std::thread::Builder::new()
        .name("dir-picker".into())
        .spawn(move || {
            let picked = rfd::FileDialog::new()
                .set_title("选择游戏根目录（含 replays 子目录）")
                .pick_folder()
                .map(|p| p.to_string_lossy().to_string());
            let _ = tx.send(picked);
        })
        .expect("spawn dir picker")
}

/// 自动发现后台任务（注册表 + 盘符扫描会碰注册表与文件系统，
/// 同样不占 UI 线程）。结果 None = 没找到，UI 引导手动选。
#[cfg(feature = "gui")]
pub fn spawn_discovery(
    tx: std::sync::mpsc::Sender<Option<String>>,
) -> std::thread::JoinHandle<()> {
    std::thread::Builder::new()
        .name("game-discovery".into())
        .spawn(move || {
            let found = discover().map(|f| f.game_dir.to_string_lossy().to_string());
            let _ = tx.send(found);
        })
        .expect("spawn game discovery")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app_config::SOURCE_MANUAL;
    use std::fs;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir()
            .join(format!("chatrans-discovery-test-{}-{}", std::process::id(), tag));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    /// 永远返回 None 的假注册表（非捕获函数项 → 可以安全转 &dyn Fn）。
    fn none_registry(_key: &str, _value: &str) -> Option<String> {
        None
    }

    /// 注册表命中且校验通过 → registry 来源。
    #[test]
    fn registry_hit_wins() {
        let game = temp_dir("reg-hit");
        fs::create_dir_all(game.join("replays")).unwrap();

        let reader = |_key: &str, _value: &str| -> Option<String> {
            Some(game.to_string_lossy().to_string())
        };
        let roots = vec![PathBuf::from(r"E:\Game\Korabli")];
        let found = discover_with(&reader, &roots).expect("注册表命中应被发现");
        assert_eq!(found.game_dir, game);
        assert_eq!(found.source, SOURCE_REGISTRY);
        let _ = fs::remove_dir_all(&game);
    }

    /// 注册表给的路径校验不过（空目录）→ 降级扫描；扫描命中 → scan 来源。
    #[test]
    fn registry_invalid_falls_back_to_scan() {
        let bogus = temp_dir("reg-invalid"); // 没有 exe 也没有 replays/ → 无效
        let game = temp_dir("scan-hit");
        fs::write(game.join("WorldOfWarships.exe"), b"MZ").unwrap();

        let bogus_str = bogus.to_string_lossy().to_string();
        let reader = move |_key: &str, _value: &str| -> Option<String> {
            Some(bogus_str.clone())
        };
        let roots = vec![game.clone()];
        let found = discover_with(&reader, &roots).expect("扫描命中应被发现");
        assert_eq!(found.game_dir, game);
        assert_eq!(found.source, SOURCE_SCAN);
        let _ = fs::remove_dir_all(&bogus);
        let _ = fs::remove_dir_all(&game);
    }

    /// 注册表路径带引号/空白（注册表里常见的手滑写法）要能清洗。
    #[test]
    fn registry_value_with_quotes_is_trimmed() {
        let game = temp_dir("reg-quotes");
        fs::create_dir_all(game.join("replays")).unwrap();
        let game_str = format!("\"{}\" ", game.display());

        let reader = move |_key: &str, _value: &str| -> Option<String> { Some(game_str.clone()) };
        let found = discover_with(&reader, &[]).expect("清洗后应命中");
        assert_eq!(found.game_dir, game);
        let _ = fs::remove_dir_all(&game);
    }

    /// 全部候选都失败 → None（不 panic），交 UI 引导手动选。
    #[test]
    fn nothing_found_returns_none() {
        let bogus = temp_dir("all-invalid");
        let roots = vec![bogus.clone(), PathBuf::from(r"Q:\no\such\drive")];
        let found = discover_with(&none_registry, &roots);
        assert!(found.is_none());
        let _ = fs::remove_dir_all(&bogus);
    }

    /// 扫描候选按顺序取第一个有效的。
    #[test]
    fn scan_takes_first_valid_root() {
        let first = temp_dir("scan-first");
        let second = temp_dir("scan-second");
        fs::create_dir_all(first.join("replays")).unwrap();
        fs::create_dir_all(second.join("replays")).unwrap();

        let roots = vec![first.clone(), second.clone()];
        let found = discover_with(&none_registry, &roots).expect("应有命中");
        assert_eq!(found.game_dir, first);
        let _ = fs::remove_dir_all(&first);
        let _ = fs::remove_dir_all(&second);
    }

    /// apply_to_config：空配置被填充 + source 记录；已手动配置的不被覆盖。
    #[test]
    fn apply_respects_manual_config() {
        let mut config = AppConfig::default();
        let found = DiscoveryResult::new(PathBuf::from(r"D:\Game\Korabli"), SOURCE_SCAN);
        apply_to_config(&mut config, &found);
        assert_eq!(config.game_dir, r"D:\Game\Korabli");
        assert_eq!(config.replays_dir, r"D:\Game\Korabli\replays");
        assert_eq!(config.source, SOURCE_SCAN);

        // 已有手动配置 → 不覆盖
        config.game_dir = r"E:\My\Korabli".to_string();
        config.source = SOURCE_MANUAL.to_string();
        apply_to_config(&mut config, &found);
        assert_eq!(config.game_dir, r"E:\My\Korabli");
        assert_eq!(config.source, SOURCE_MANUAL);
    }

    /// 真实发现冒烟（不注入）：只断言"不 panic"，结果随环境而定。
    #[test]
    fn real_discover_never_panics() {
        let _ = discover();
    }
}
