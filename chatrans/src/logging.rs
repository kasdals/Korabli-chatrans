//! 日志与控制台（S9）：GUI 双击不再弹黑窗，排障信息落到文件。
//!
//! 背景：Rust 在 Windows 上默认是 **console 子系统**，双击 `chatrans.exe`
//! 会先弹一个黑色控制台窗口（用户截图里那个）。改成 `windows_subsystem =
//! "windows"` 后黑窗没了，但 stdout/stderr 也随之失效 —— 所以要做两件事：
//!
//! 1. **从终端带参数启动时**（`-r` / `--headless` / `--help` …）附着到父
//!    控制台（[`attach_parent_console`]），让日志和 clap 的帮助信息照常
//!    显示在用户自己的终端里；
//! 2. **无论如何都写日志文件**（[`TeeWriter`]），双击启动时出问题还能查。
//!
//! 日志同时写文件和 stderr：有控制台就两边都有，没有控制台就只落文件，
//! 单条写失败一律静默（日志不该拖垮宿主）。

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use tracing::level_filters::LevelFilter;
use tracing_subscriber::fmt::MakeWriter;

/// 单个日志文件上限：超过就滚动一次（只留当前 + 上一份）。
pub const MAX_LOG_BYTES: u64 = 2 * 1024 * 1024;

/// 日志文件位置：`%LOCALAPPDATA%\KorabliOne\logs\chatrans.log`。
pub fn log_path() -> PathBuf {
    let base = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    base.join("KorabliOne").join("logs").join("chatrans.log")
}

/// 超过上限就该滚动（纯函数，单测覆盖）。
pub fn should_rotate(current_len: u64) -> bool {
    current_len >= MAX_LOG_BYTES
}

/// 启动时准备日志文件：目录不存在就建，超限就滚成 `.1`。
fn prepare_log_file(path: &Path) -> io::Result<File> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    if let Ok(meta) = fs::metadata(path) {
        if should_rotate(meta.len()) {
            let rotated = path.with_extension("log.1");
            let _ = fs::remove_file(&rotated);
            let _ = fs::rename(path, &rotated);
        }
    }
    OpenOptions::new().create(true).append(true).open(path)
}

/// 同时写文件与 stderr 的 writer（tracing 的 `MakeWriter`）。
pub struct TeeWriter {
    file: Mutex<Option<File>>,
    echo_stderr: bool,
}

impl TeeWriter {
    /// `file=None` 表示文件打不开 —— 只保留 stderr，绝不 panic。
    fn new(file: Option<File>, echo_stderr: bool) -> TeeWriter {
        TeeWriter {
            file: Mutex::new(file),
            echo_stderr,
        }
    }
}

impl<'a> MakeWriter<'a> for TeeWriter {
    type Writer = TeeGuard<'a>;

    fn make_writer(&'a self) -> Self::Writer {
        TeeGuard { tee: self }
    }
}

pub struct TeeGuard<'a> {
    tee: &'a TeeWriter,
}

impl Write for TeeGuard<'_> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if let Ok(mut guard) = self.tee.file.lock() {
            if let Some(file) = guard.as_mut() {
                let _ = file.write_all(buf);
            }
        }
        if self.tee.echo_stderr {
            let _ = io::stderr().write_all(buf);
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        if let Ok(mut guard) = self.tee.file.lock() {
            if let Some(file) = guard.as_mut() {
                let _ = file.flush();
            }
        }
        if self.tee.echo_stderr {
            let _ = io::stderr().flush();
        }
        Ok(())
    }
}

/// 装好全局 tracing：文件 + stderr（无控制台时 stderr 写入自动无效）。
///
/// `ANSI` 关闭：同一份字节要进文件，带转义序列的日志没法读。
pub fn init() {
    let path = log_path();
    let file = prepare_log_file(&path).ok();
    let writer = TeeWriter::new(file, true);
    let _ = tracing_subscriber::fmt()
        .with_max_level(LevelFilter::INFO)
        .with_ansi(false)
        .with_writer(writer)
        .try_init();
    tracing::info!(target: "chatrans::logging", "log file: {}", path.display());
}

// ── Windows 控制台 ─────────────────────────────────────────────────────

/// 附着到父进程的控制台（从 cmd/PowerShell 启动时沿用用户那个窗口）。
///
/// 父进程没有控制台（双击）→ 新建一个，至少能看到内容。返回是否拿到了
/// 一个可写的控制台。非 Windows 平台恒 false。
#[cfg(windows)]
pub fn attach_parent_console() -> bool {
    const ATTACH_PARENT_PROCESS: u32 = 0xFFFF_FFFF;
    const STD_INPUT_HANDLE: u32 = -10i32 as u32;
    const STD_OUTPUT_HANDLE: u32 = -11i32 as u32;
    const STD_ERROR_HANDLE: u32 = -12i32 as u32;
    const GENERIC_READ: u32 = 0x8000_0000;
    const GENERIC_WRITE: u32 = 0x4000_0000;
    const FILE_SHARE_READ: u32 = 0x0000_0001;
    const FILE_SHARE_WRITE: u32 = 0x0000_0002;
    const OPEN_EXISTING: u32 = 3;

    #[link(name = "kernel32")]
    extern "system" {
        fn AttachConsole(dw_process_id: u32) -> i32;
        fn AllocConsole() -> i32;
        fn GetStdHandle(which: u32) -> isize;
        fn CreateFileW(
            name: *const u16,
            access: u32,
            share: u32,
            security: *const core::ffi::c_void,
            creation: u32,
            flags: u32,
            template: isize,
        ) -> isize;
        fn SetStdHandle(which: u32, handle: isize) -> i32;
    }

    // UTF-16 字面量（结尾的 0 手写，避免依赖 c"" 字面量的 Rust 版本）
    let conout: Vec<u16> = "CONOUT$\0".encode_utf16().collect();
    let conin: Vec<u16> = "CONIN$\0".encode_utf16().collect();

    // 句柄有效 = 调用方已经给了输出目标（终端控制台、管道、文件重定向）。
    // 这种情况下**绝不改写**：`chatrans.exe --help | more` 必须照常进管道。
    fn valid(handle: isize) -> bool {
        handle != 0 && handle != -1
    }

    unsafe {
        let out_valid = valid(GetStdHandle(STD_OUTPUT_HANDLE));
        let err_valid = valid(GetStdHandle(STD_ERROR_HANDLE));
        if out_valid && err_valid {
            return true;
        }

        let attached = AttachConsole(ATTACH_PARENT_PROCESS) != 0;
        if !attached && AllocConsole() == 0 {
            return false;
        }
        let out = CreateFileW(
            conout.as_ptr(),
            GENERIC_READ | GENERIC_WRITE,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            core::ptr::null(),
            OPEN_EXISTING,
            0,
            0,
        );
        let err = CreateFileW(
            conout.as_ptr(),
            GENERIC_READ | GENERIC_WRITE,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            core::ptr::null(),
            OPEN_EXISTING,
            0,
            0,
        );
        let inp = CreateFileW(
            conin.as_ptr(),
            GENERIC_READ | GENERIC_WRITE,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            core::ptr::null(),
            OPEN_EXISTING,
            0,
            0,
        );
        let mut ok = false;
        if !out_valid && valid(out) {
            SetStdHandle(STD_OUTPUT_HANDLE, out);
            ok = true;
        }
        if !err_valid && valid(err) {
            SetStdHandle(STD_ERROR_HANDLE, err);
            ok = true;
        }
        if !valid(GetStdHandle(STD_INPUT_HANDLE)) && valid(inp) {
            SetStdHandle(STD_INPUT_HANDLE, inp);
        }
        ok
    }
}

#[cfg(not(windows))]
pub fn attach_parent_console() -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rotation_threshold() {
        assert!(!should_rotate(0));
        assert!(!should_rotate(MAX_LOG_BYTES - 1));
        assert!(should_rotate(MAX_LOG_BYTES));
        assert!(should_rotate(MAX_LOG_BYTES * 3));
    }

    #[test]
    fn prepare_creates_dir_and_appends() {
        let dir = std::env::temp_dir().join(format!("chatrans-log-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let path = dir.join("logs").join("chatrans.log");
        {
            let mut f = prepare_log_file(&path).expect("create");
            f.write_all(b"hello\n").unwrap();
        }
        {
            let mut f = prepare_log_file(&path).expect("append");
            f.write_all(b"world\n").unwrap();
        }
        let text = fs::read_to_string(&path).unwrap();
        assert_eq!(text, "hello\nworld\n", "同一次会话/多次启动都追加，不覆盖");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn tee_writes_without_file() {
        // 文件打不开时也不能 panic，写入照常返回成功
        let tee = TeeWriter::new(None, false);
        let mut w = tee.make_writer();
        assert_eq!(w.write(b"x").unwrap(), 1);
        w.flush().unwrap();
    }
}
