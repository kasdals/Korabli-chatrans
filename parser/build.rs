//! 让 cargo 感知 `addons/` 的内容变化。
//!
//! `src/version.rs` 用 rust-embed 的 `#[folder = "../addons/"]` 把这个目录在编译期
//! 整个嵌进二进制。rust-embed 是靠 proc-macro 直接读文件生成字节字面量，cargo 只能
//! 靠 mtime 判断这些文件变没变。
//!
//! 问题在于：`addons/` 里的数据通常是从游戏客户端复制/解压过来的，Windows 复制会
//! 保留源文件的 LastWriteTime，7-Zip 解压同样保留。于是内容变了、时间戳没变，
//! cargo 判定 up-to-date，直接跳过编译，exe 里仍然是旧数据。
//!
//! 这里改成按**内容哈希**判断：本脚本每次构建都跑一次（`rerun-if-changed` 指向一个
//! 永远不存在的路径），把 `addons/` 全量指纹写进 `rustc-env`；指纹变了，cargo 才会
//! 认为这个 crate 需要重编。开销是遍历 + 读一遍 addons（百来个文件），可以忽略。

use std::collections::hash_map::DefaultHasher;
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

fn collect(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect(&path, out);
        } else {
            out.push(path);
        }
    }
}

fn main() {
    // 监听整个 addons 目录：新增 / 删除 / 修改（mtime 变化）都会触发本脚本重跑，
    // 重跑后算出的内容指纹一变，cargo 就会重编 parser。
    // 注意：cargo 对 build script 的触发完全基于 mtime，所以"内容变了但时间戳没变"
    // 这种情形它看不见——从游戏客户端复制文件时就会出现。遇到那种情况用
    // 仓库根目录的 build.ps1，它会先把 addons 的时间戳统一刷新。
    println!("cargo:rerun-if-changed=../addons");

    let addons = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("addons");

    let mut files = Vec::new();
    collect(&addons, &mut files);
    files.sort();

    let mut hasher = DefaultHasher::new();
    for file in &files {
        // 路径也计入指纹：新增/删除/改名都能被感知。
        file.to_string_lossy().hash(&mut hasher);
        match fs::read(file) {
            Ok(bytes) => bytes.hash(&mut hasher),
            // 读不到就留个标记，别让整个指纹因为一个坏文件崩掉。
            Err(_) => 0u8.hash(&mut hasher),
        }
    }

    println!("cargo:rustc-env=ADDONS_FINGERPRINT={:016x}", hasher.finish());
}
