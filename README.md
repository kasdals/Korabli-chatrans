<div align="center">
    <h2 align="center">Chatrans</h2>
    <p align="center">
        Korabli（战舰世界 / Lesta 客户端）战局聊天实时翻译悬浮面板
        <br />
        <b>简体中文</b> · <a href="#english">English</a> · <a href="https://github.com/Yusux/Korabli-chatrans">上游项目 / Upstream</a>
    </p>
</div>

## 这是什么

在**对局进行中**实时读取游戏 `replays` 目录的 `temp.korablireplay` / `tempArenaInfo.json`，把战局聊天实时显示在一个**置顶悬浮面板**上，并可选接入 OpenAI / Anthropic 兼容的模型供应商（DeepSeek、通义等）做 LLM 翻译。

- 双击 `chatrans.exe` 即可启动，**不会弹出黑色控制台窗口**
- 首次运行自动发现游戏目录
- 面板默认锁定、点击穿透——**绝不抢游戏的鼠标点击与键盘焦点**
- API key 经 Windows DPAPI 加密存储，不落明文、不进日志

## 快速开始

本仓库暂无预编译版本，需自行编译：

``` sh
git clone https://github.com/kasdals/Korabli-chatrans.git
cd Korabli-chatrans
cargo build --release --features gui
```

产物在 `target\release\chatrans.exe`。不带 `--features gui` 会得到纯控制台构建。

> **游戏大版本更新后解析失败？** `addons/scripts` 是从游戏内解包的实体定义，大版本更新后可能过期。从 [Korabli-scripts](https://github.com/Yusux/Korabli-scripts) 获取最新版覆盖 `addons/scripts` 后重新编译即可。

## 首次设置（三步）

双击 `chatrans.exe`，首次运行会自动打开设置视图：

1. **游戏路径** —— 程序按「注册表 → 常见安装路径」自动发现（如 `E:\Game\Korabli`）；未命中时点**浏览…**手动选择游戏根目录（即包含 `replays` 文件夹的那一级）。路径有效会显示绿色 ✓。`replays` 子目录会自动推导并创建，无需手动指定。
2. **模型供应商**（用翻译就必填）—— 填写 ID、名称、API Base URL、API Key（输入框默认掩码，点眼睛切换明文核对），模型可点**获取模型列表**从供应商拉取（失败会红字提示，可手动填模型 id）。点**添加更多**可配多个供应商，单选按钮决定当前生效的那个。高级配置里可调超时秒数、代理地址（如 `http://127.0.0.1:7890`）、自定义请求头。
3. **保存配置** —— 路径有效时面板立即开始监听，进对局聊天即上屏。

底部还可调：目标语言（默认中文）、频道范围（只翻译队伍频道 / 全部）、仅译文（隐藏原文行）、面板透明度（40%–100%，实时预览）。

## 面板使用

顶栏图标（右 → 左）：**关闭 · 置顶 · 清空 · 设置（齿轮）· 锁定（锁）**。

| 操作 | 说明 |
| --- | --- |
| 锁定（默认） | 整窗点击穿透，游戏操作零干扰。把鼠标悬停在顶栏 **150ms** 即可临时接管，点图标；锁定状态会被记住，重启自动恢复 |
| 解锁 | 拖动标题栏移动；拖窗口边缘缩放（对边钉住不跑偏）；位置尺寸持久化，重启恢复 |
| 打开设置 | 锁定态也可点齿轮；设置打开期间整窗可交互（可以正常打字），关闭后立即恢复锁定语义 |
| 置顶 | 切换 always-on-top |
| 清空 | 清空当前消息列表 |

**状态行**（面板底部）：

| 显示 | 含义 |
| --- | --- |
| `已连接 · 本局 N 条 · 已译 M 条` | 正常工作中 |
| `未配置游戏路径 · 点这里打开设置` | 还没配游戏路径（锁定态下点击穿透，走齿轮进设置） |
| `后台管线未运行` | 监听线程未启动或已退出，重启程序可恢复 |
| `连不上：<原因>` | 模型供应商请求失败（查 API key、Base URL、网络/代理）；恢复后自动回到正常态 |

## 配置文件

全部位于 `%LOCALAPPDATA%\KorabliOne\`：

| 文件 | 内容 |
| --- | --- |
| `app.json` | 游戏目录等本机配置 |
| `translation.json` | 模型供应商配置（字段名沿用 `Endpoint*`） |
| `secrets.dat` | API 密钥，DPAPI 加密，永不落明文 |
| `placement.json` | 面板位置 / 尺寸 / 锁定状态 / 透明度（自动修正越界与脏值） |
| `logs\chatrans.log` | 运行日志（单文件 2MB 上限，超出滚动为 `.log.1`） |

> **用词说明**：界面上叫「模型供应商」，配置字段叫 `Endpoint` —— 两者是同一个东西。

## 控制台模式（可选）

``` powershell
.\chatrans.exe -r 'D:\Game\Korabli\replays'
```

实时把聊天**原文**打印到终端（`── 新对局开始：×× ──` / `[友] …` / `[敌] …`）。控制台模式**不做翻译**——翻译能力在 GUI 面板里。GUI 构建加 `--headless` 行为相同。

启动规则：**双击（无参数）** = 纯 GUI，不占用任何控制台，日志写 `logs\chatrans.log`；**带参数** = 附着当前终端运行，输出直接可见。

``` text
Usage: chatrans.exe [OPTIONS] --replay-dir <REPLAY_DIR>

Options:
  -r, --replay-dir <REPLAY_DIR>        回放目录
  -t, --target-language <LANG>         目标语言，zh/en，默认 zh
      --access-key-id <ID>             Aliyun access key id（内置解释器用）
      --access-key-secret <SECRET>     Aliyun access key secret
      --headless                       不启动面板（控制台模式）
  -h, --help                           打印帮助
  -V, --version                        打印版本
```

## 已知限制

- 自动发现仅覆盖部分安装方式（注册表 + 常见盘符路径）；未命中时**手动浏览**永远可用
- 部分系统环境下面板底边可能滑进任务栏区域（Windows 工作区 API 失败的环境特异问题）
- 多显示器场景未做适配
- 翻译延迟取决于供应商响应速度（DeepSeek 实测约 1–3 秒/条）
- 关闭面板 = 退出程序（本程序无宿主主窗口，面板即程序）

## 二次开发

``` sh
cargo test                        # 默认构建测试
cargo test --features gui         # 含 GUI 全量测试
cargo build --release             # 控制台版
cargo build --release --features gui   # GUI 版
```

GUI 相关代码在 `chatrans/src/gui/`（面板/交互/设置视图），翻译链路在 `chatrans/src/translate/`（client / store / coordinator），配置与自动发现在 `chatrans/src/app_config.rs`、`game_discovery.rs`。

## 许可与来源

MIT License（见 [LICENSE](LICENSE)）。本仓库 fork 自 [Yusux/Korabli-chatrans](https://github.com/Yusux/Korabli-chatrans)，解析能力基于 [wows-replays](https://github.com/lkolbly/wows-replays)，本项目在其上完成了 GUI 悬浮面板化改造（网页客户端移除、设置内嵌、双击启动、自动发现）。

---

<a id="english"></a>
<div align="center">
    <h2 align="center">English</h2>
</div>

## What is this

Reads `temp.korablireplay` / `tempArenaInfo.json` from the game's `replays` folder **during a live battle** and shows chat messages on an **always-on-top overlay panel**, with optional LLM translation via OpenAI / Anthropic compatible providers (DeepSeek, Qwen, etc.).

- Double-click `chatrans.exe` to launch — **no console window pops up**
- Auto-discovers the game directory on first run
- The panel starts locked and click-through — **never steals mouse clicks or keyboard focus from the game**
- API keys are stored encrypted with Windows DPAPI; never written in plain text or logs

## Quick start

No prebuilt binaries in this repository yet — build it yourself:

``` sh
git clone https://github.com/kasdals/Korabli-chatrans.git
cd Korabli-chatrans
cargo build --release --features gui
```

The executable lands in `target\release\chatrans.exe`. Building without `--features gui` yields a console-only binary.

> **Parsing broken after a game update?** `addons/scripts` contains entity definitions unpacked from the game and may go stale after major patches. Grab the latest from [Korabli-scripts](https://github.com/Yusux/Korabli-scripts), overwrite `addons/scripts`, and rebuild.

## First-time setup (3 steps)

Double-click `chatrans.exe`; the settings view opens automatically on first run:

1. **Game directory** — auto-discovered via "Windows registry → common install paths" (e.g. `E:\Game\Korabli`); if that fails, click **浏览…** (Browse) and pick the game root folder (the one containing the `replays` folder). A green ✓ appears when valid. The `replays` subfolder is derived and created automatically.
2. **Model provider** (required for translation) — fill in ID, name, API Base URL, API Key (masked by default; click the eye icon to verify). Click **获取模型列表** (Fetch models) to pull the model list from the provider; on failure you can type model ids manually. **添加更多** (Add more) supports multiple providers; the radio button picks the active one. Advanced settings: timeout seconds, proxy address (e.g. `http://127.0.0.1:7890`), custom headers.
3. **保存配置** (Save) — when the path is valid the panel starts listening immediately; chat appears in battle.

Bottom of the settings view: target language (Chinese by default), channel scope (team channel only / all), translation-only mode (hide original lines), panel opacity (40%–100%, live preview).

## Panel usage

Top bar icons (right → left): **Close · Pin on top · Clear · Settings (gear) · Lock**.

| Action | Behavior |
| --- | --- |
| Locked (default) | Whole window is click-through; the game is never disturbed. Hover the top bar for **150 ms** to temporarily take over and click icons. The locked state is remembered across restarts |
| Unlocked | Drag the title bar to move; drag edges to resize (opposite edge stays pinned); position/size persist across restarts |
| Settings open | Clickable even when locked; while open the whole window is interactive (typing works), locking semantics resume the moment you close it |
| Pin on top | Toggle always-on-top |
| Clear | Clear the message list |

**Status line** (bottom of the panel):

| Display | Meaning |
| --- | --- |
| `已连接 · 本局 N 条 · 已译 M 条` | Working normally |
| `未配置游戏路径 · 点这里打开设置` | Game path not configured (click-through when locked; use the gear to open settings) |
| `后台管线未运行` | Watcher thread not started or dead — restart the app |
| `连不上：<原因>` | Provider request failed (check API key, Base URL, network/proxy); returns to normal automatically |

## Configuration files

Everything lives in `%LOCALAPPDATA%\KorabliOne\`:

| File | Content |
| --- | --- |
| `app.json` | Local machine config (game directory, etc.) |
| `translation.json` | Model provider config (fields still named `Endpoint*`) |
| `secrets.dat` | API keys, DPAPI-encrypted, never in plain text |
| `placement.json` | Panel position / size / locked state / opacity (out-of-bounds and corrupt values auto-repaired) |
| `logs\chatrans.log` | Runtime log (2 MB per file, rolls over to `.log.1`) |

> **Naming note**: the UI says "模型供应商" (model provider) while config fields say `Endpoint` — they are the same thing.

## Console mode (optional)

``` powershell
.\chatrans.exe -r 'D:\Game\Korabli\replays'
```

Prints **original** chat lines to the terminal (`── 新对局开始：×× ──` / `[友] …` / `[敌] …`). Console mode does **no translation** — translation lives in the GUI panel. `--headless` on a GUI build behaves the same.

Launch rules: **double-click (no args)** = pure GUI, no console attached, logs to `logs\chatrans.log`; **with args** = attaches to the current terminal so output is visible.

``` text
Usage: chatrans.exe [OPTIONS] --replay-dir <REPLAY_DIR>

Options:
  -r, --replay-dir <REPLAY_DIR>        The replay dir to use
  -t, --target-language <LANG>         Target language, zh/en. Default: zh
      --access-key-id <ID>             Aliyun access key id (built-in interpreter)
      --access-key-secret <SECRET>     Aliyun access key secret
      --headless                       Run without the overlay panel (console mode)
  -h, --help                           Print help
  -V, --version                        Print version
```

## Known limitations

- Auto-discovery covers some install methods only (registry + common drive paths); **manual browse** always works as a fallback
- On some systems the panel's bottom edge may slide into the taskbar area (environment-specific Windows work-area API failure)
- Multi-monitor layouts are not adapted
- Translation latency depends on the provider (measured ~1–3 s per line on DeepSeek)
- Closing the panel = exiting the program (there is no host window; the panel is the app)

## Development

``` sh
cargo test                        # console-build tests
cargo test --features gui         # full test suite incl. GUI
cargo build --release             # console build
cargo build --release --features gui   # GUI build
```

GUI code lives in `chatrans/src/gui/` (panel / interaction / settings view), the translation pipeline in `chatrans/src/translate/` (client / store / coordinator), config and auto-discovery in `chatrans/src/app_config.rs` and `game_discovery.rs`.

## License & credits

MIT License (see [LICENSE](LICENSE)). This repository is a fork of [Yusux/Korabli-chatrans](https://github.com/Yusux/Korabli-chatrans); parsing is based on [wows-replays](https://github.com/lkolbly/wows-replays). This fork replaces the web client with a native GUI overlay panel (in-panel settings, double-click launch, game directory auto-discovery).
