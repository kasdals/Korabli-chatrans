<div align="center">
    <h2 align="center">Chatrans</h2>
    <p align="center">
        Real-time battle chat translation overlay for Korabli (World of Warships / Lesta client)
        <br />
        <a href="README.md">简体中文</a> · <b>English</b> · <a href="https://github.com/Yusux/Korabli-chatrans">Upstream</a>
    </p>
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

**Chinese ship names** come from [DDFantasyV/Korabli_localization_chs](https://github.com/DDFantasyV/Korabli_localization_chs), a Simplified Chinese localization mod for the Lesta client (Apache-2.0). Names are extracted from its `global.mo` for game version 26.11 (`IDS_<index>_FULL`) into `chatrans/ship_names.tsv` and `chatrans/ship_ids.tsv`. Only the ship-name entries are used; nothing else from the mod (fonts, UI strings) is bundled here.
