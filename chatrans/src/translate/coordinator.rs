//! 翻译调度器（移植自雷达 addon 的 `TranslateCoordinator.cs`）。
//!
//! 「收到原文 → 翻译 → 回填译文」，三条硬规则照抄雷达已验收实现：
//!   1. 同一原文（hash 相同）**只翻一次**，缓存命中直接返回；
//!   2. 并发上限 [`MAX_CONCURRENT`]（2）—— 对局中网络抖一下不能把队列打爆；
//!   3. 失败**静默降级**：返回 `None` 并 log::warn 留痕（静默的是 UI 不是日志），
//!      不 panic、不重试风暴。调用方拿到 None 继续显示原文。
//!
//! 日志纪律：只记供应商 ID / 模型名 / 耗时 / HTTP 状态 / 错误文本，
//! 绝不记录 apiKey，也不记录待翻译正文。
//!
//! 缓存键用 SHA1+Base64（本项目已有依赖，键长恒定，不把聊天原文长期
//! 留在字典键里）。

use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::Instant;

use base64::prelude::*;
use sha1::{Digest, Sha1};
use tokio::sync::Semaphore;
use tracing::{info, warn};


/// 并发上限（与雷达 addon 一致）。
pub const MAX_CONCURRENT: usize = 2;

/// 原文 → 缓存键。SHA1 后 Base64，键长恒定。
pub fn cache_key(text: &str) -> String {
    let mut hasher = Sha1::new();
    hasher.update(text.as_bytes());
    BASE64_STANDARD.encode(hasher.finalize())
}

/// 拿锁的统一姿势：锁中毒时取回内部数据继续跑 —— 缓存/去重集合里的
/// 数据本身没有不变量会被破坏，没必要因为一次 panic 连坐整个翻译链路。
fn lock_or_recover<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

pub struct TranslateCoordinator {
    concurrency: Semaphore,
    cache: Mutex<HashMap<String, String>>,
    in_flight: Mutex<HashSet<String>>,
    translated_count: AtomicU64,
}

impl Default for TranslateCoordinator {
    fn default() -> Self {
        TranslateCoordinator::new()
    }
}

impl TranslateCoordinator {
    pub fn new() -> TranslateCoordinator {
        TranslateCoordinator {
            concurrency: Semaphore::new(MAX_CONCURRENT),
            cache: Mutex::new(HashMap::new()),
            in_flight: Mutex::new(HashSet::new()),
            translated_count: AtomicU64::new(0),
        }
    }

    /// 渲染前调用：命中缓存就返回译文，否则 None（先显示原文）。
    /// 纯内存查找、每帧都会调用，这里绝不能写日志（否则一次重绘刷十条）。
    pub fn cache_get(&self, text: &str) -> Option<String> {
        lock_or_recover(&self.cache)
            .get(&cache_key(text))
            .cloned()
    }

    /// 翻译成功的累计条数（底栏「已译 N 条」用）。
    pub fn translated_count(&self) -> u64 {
        self.translated_count.load(Ordering::Relaxed)
    }

    /// 该原文是否正在翻译中（GUI 显示"翻译中…"占位用）。
    pub fn is_in_flight(&self, text: &str) -> bool {
        lock_or_recover(&self.in_flight).contains(&cache_key(text))
    }

    /// 核心入口：带缓存 + 去重 + 并发上限地执行一次翻译。
    ///
    /// `translate` 是真正的翻译动作（由调用方注入：生产环境是
    /// [`client::translate`]，测试里是假闭包）；`log_context` 是日志前缀
    /// （如 `"ep=ep1 model=m "`），让 ok/fail 行带上供应商信息但不带 key 和正文。
    /// 返回：
    ///   - `Some(译文)`：本次翻译成功或缓存命中；
    ///   - `None`：翻译失败，或同一原文已在翻译中（调用方继续显示原文）。
    pub async fn translate_with<F, Fut>(
        &self,
        text: &str,
        log_context: &str,
        translate: F,
    ) -> Option<String>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Option<String>>,
    {
        let key = cache_key(text);

        // 缓存命中：直接返回，不发网络
        if let Some(hit) = self.cache_get(text) {
            return Some(hit);
        }

        // 去重：同一原文在途时不重复排队（消息更新会把同一行反复推过来）
        {
            let mut in_flight = lock_or_recover(&self.in_flight);
            if in_flight.contains(&key) {
                return None;
            }
            in_flight.insert(key.clone());
        }

        // 并发上限：第 3 个以后的请求在这里排队等信号量
        let _permit = match self.concurrency.acquire().await {
            Ok(permit) => permit,
            Err(_) => {
                // 信号量被 close 才会走到这（当前代码从不 close），防御性返回
                lock_or_recover(&self.in_flight).remove(&key);
                return None;
            }
        };

        let started = Instant::now();
        let result = translate().await;

        // 无论如何都要放人：in_flight 移除 + 信号量随 permit drop 释放
        lock_or_recover(&self.in_flight).remove(&key);

        match result {
            Some(translated) => {
                lock_or_recover(&self.cache).insert(key, translated.clone());
                self.translated_count.fetch_add(1, Ordering::Relaxed);
                // 日志格式与雷达 addon 一致：[翻译] ok ep=… model=… {ms}ms
                info!("[翻译] ok {}{}ms", log_context, started.elapsed().as_millis());
                Some(translated)
            }
            None => {
                // UI 静默降级，但日志必须留痕 —— 不缓存失败，下次原文再
                // 出现时给翻译一次重新成功的机会（雷达同语义）
                warn!("[翻译] fail {}{}ms", log_context, started.elapsed().as_millis());
                None
            }
        }
    }

    // 高层组合（供应商选择 + 失败原因回传状态行）在 gui/app.rs 的 worker 里；
    // 本类型只保留经过完整测试的核心机制 translate_with。
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicUsize};
    use std::sync::Arc;

    /// 缓存命中：同一原文只真正翻译一次。
    #[tokio::test]
    async fn cache_hit_returns_without_second_call() {
        let coordinator = TranslateCoordinator::new();
        let calls = Arc::new(AtomicUsize::new(0));

        let first = {
            let calls = calls.clone();
            coordinator
                .translate_with("gg wp", "ep=t model=m ", move || {
                    calls.fetch_add(1, Ordering::SeqCst);
                    async { Some("打得好".to_string()) }
                })
                .await
        };
        assert_eq!(first.as_deref(), Some("打得好"));

        // 第二次同原文：直接命中缓存，闭包不再被调用
        let second = coordinator.cache_get("gg wp");
        assert_eq!(second.as_deref(), Some("打得好"));
        let via_translate = {
            let calls = calls.clone();
            coordinator
                .translate_with("gg wp", "", move || {
                    calls.fetch_add(1, Ordering::SeqCst);
                    async { Some("不该被调用".to_string()) }
                })
                .await
        };
        assert_eq!(via_translate.as_deref(), Some("打得好"));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(coordinator.translated_count(), 1);
    }

    /// 不同原文各翻各的（缓存键按原文区分）。
    #[tokio::test]
    async fn different_texts_are_translated_separately() {
        let coordinator = TranslateCoordinator::new();
        let a = coordinator
            .translate_with("hello", "", || async { Some("你好".to_string()) })
            .await;
        let b = coordinator
            .translate_with("world", "", || async { Some("世界".to_string()) })
            .await;
        assert_eq!(a.as_deref(), Some("你好"));
        assert_eq!(b.as_deref(), Some("世界"));
    }

    /// 失败静默降级：返回 None、不缓存、下次还能再试。
    #[tokio::test]
    async fn failure_returns_none_and_is_not_cached() {
        let coordinator = TranslateCoordinator::new();
        let calls = Arc::new(AtomicUsize::new(0));

        let make = |calls: Arc<AtomicUsize>, fail: bool| {
            move || {
                let calls = calls.clone();
                async move {
                    calls.fetch_add(1, Ordering::SeqCst);
                    if fail {
                        None
                    } else {
                        Some("好了".to_string())
                    }
                }
            }
        };

        let first = coordinator
            .translate_with("ouch", "", make(calls.clone(), true))
            .await;
        assert_eq!(first, None);
        assert_eq!(coordinator.cache_get("ouch"), None, "失败不能进缓存");
        assert_eq!(coordinator.translated_count(), 0);

        // 重试（同一 coordinator）：失败不缓存，所以会再次尝试
        let second = coordinator
            .translate_with("ouch", "", make(calls.clone(), false))
            .await;
        assert_eq!(second.as_deref(), Some("好了"));
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    /// 在途去重：同一原文并发提交只翻一次。
    #[tokio::test]
    async fn duplicate_in_flight_requests_are_deduplicated() {
        let coordinator = Arc::new(TranslateCoordinator::new());
        let calls = Arc::new(AtomicUsize::new(0));
        let claimed = Arc::new(AtomicBool::new(false));

        let mut handles = Vec::new();
        for _ in 0..5 {
            let coordinator = coordinator.clone();
            let calls = calls.clone();
            let claimed = claimed.clone();
            handles.push(tokio::spawn(async move {
                coordinator
                    .translate_with("dup", "", move || {
                        calls.fetch_add(1, Ordering::SeqCst);
                        let claimed = claimed.clone();
                        async move {
                            // 恰好一个请求能进到闭包：它睡 100ms 制造"在途"窗口，
                            // 其余四个应因在途去重根本不进闭包、直接返回 None
                            if claimed.swap(true, Ordering::SeqCst) {
                                Some("去重失效".to_string())
                            } else {
                                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                                Some("去重后".to_string())
                            }
                        }
                    })
                    .await
            }));
        }

        let results: Vec<Option<String>> = futures_util::future::join_all(handles)
            .await
            .into_iter()
            .map(|r| r.expect("task panicked"))
            .collect();

        assert_eq!(calls.load(Ordering::SeqCst), 1, "只应有 1 次真实翻译调用");
        let good = results
            .iter()
            .filter(|r| r.as_deref() == Some("去重后"))
            .count();
        assert_eq!(good, 1, "只有拿到信号量的那 1 个请求得到译文");
        assert!(
            results.iter().all(|r| r.as_deref() != Some("去重失效")),
            "去重失效：多个请求同时进了翻译闭包"
        );
    }

    /// 并发上限：6 个请求同时来，真实翻译最多同时进行 2 个。
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrency_is_capped_at_two() {
        let coordinator = Arc::new(TranslateCoordinator::new());
        let active = Arc::new(AtomicUsize::new(0));
        let max_active = Arc::new(AtomicUsize::new(0));

        let mut handles = Vec::new();
        for i in 0..6 {
            let coordinator = coordinator.clone();
            let active = active.clone();
            let max_active = max_active.clone();
            handles.push(tokio::spawn(async move {
                coordinator
                    .translate_with(&format!("text-{}", i), "", move || {
                        let active = active.clone();
                        let max_active = max_active.clone();
                        async move {
                            let now = active.fetch_add(1, Ordering::SeqCst) + 1;
                            max_active.fetch_max(now, Ordering::SeqCst);
                            tokio::time::sleep(std::time::Duration::from_millis(80)).await;
                            active.fetch_sub(1, Ordering::SeqCst);
                            Some(format!("译文-{}", i))
                        }
                    })
                    .await
            }));
        }

        let results: Vec<Option<String>> = futures_util::future::join_all(handles)
            .await
            .into_iter()
            .map(|r| r.expect("task panicked"))
            .collect();

        assert!(results.iter().all(|r| r.is_some()), "6 个请求都应成功");
        assert_eq!(
            max_active.load(Ordering::SeqCst),
            MAX_CONCURRENT,
            "真实并发的峰值必须恰好等于上限 2"
        );
        assert_eq!(coordinator.translated_count(), 6);
    }

    #[test]
    fn cache_key_is_stable_and_distinct() {
        let a = cache_key("gg wp");
        let b = cache_key("gg wp");
        let c = cache_key("гг");
        assert_eq!(a, b, "同原文同键");
        assert_ne!(a, c, "不同原文不同键");
        assert!(BASE64_STANDARD.decode(&a).is_ok(), "键应是合法 base64");
    }
}
