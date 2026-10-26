//! 翻译三件套（方案 C / S1，整体移植自雷达 addon 的 Core 层）。
//!
//! - [`store`]：配置 schema（`translation.json`）+ DPAPI 密钥库（`secrets.dat`）
//!   + 雷达旧配置的无感迁移；
//! - [`client`]：OpenAI / Anthropic 双协议客户端（移植 `ProtocolClient.cs`）；
//! - [`coordinator`]：调度器 —— 并发上限 2、原文 hash 缓存、失败静默降级。
//!
//! 行为语义与雷达已验收实现对齐：失败返回 `None` 并留日志，绝不 panic、
//! 绝不把 apiKey 写进日志。

pub mod client;
pub mod coordinator;
pub mod store;
