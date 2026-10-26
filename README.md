<div align="center">
    <h2 align="center">Chatrans</h2>
    <p align="center">
        A real-time chat interpreter for Korabli.
        <br />
        <br />
        <a href="https://github.com/Yusux/Korabli-chatrans/blob/main/README.md">English Document</a>
        ·
        <a href="https://github.com/Yusux/Korabli-chatrans/blob/main/docs/README_CN.md">中文文档</a>
        <br />
        <a href="https://github.com/Yusux/Korabli-chatrans/issues">Report Bug</a>
        ·
        <a href="https://github.com/Yusux/Korabli-chatrans/issues">Request Feature</a>
    </p>
</div>

## About The Project

This is a real-time chat interpreter for Korabli developed from [wows-replays](https://github.com/lkolbly/wows-replays). It watches the `replays` folder under the game root directory, reads the chat messages from the `temp.korablireplay` / `tempArenaInfo.json` files while you are in a battle, and shows them on an **always-on-top overlay panel** with optional LLM-powered translation (OpenAI / Anthropic compatible endpoints).

Two ways to use it:

- **Double-click `chatrans.exe` (GUI mode, recommended)** — a translation panel floats over the game. First run opens a settings view where you pick the game directory and fill in your API endpoint. Since v0.2 the built-in WebSocket/SSE web client has been removed; the panel is the client.
- **Console mode (`--headless` or a build without the `gui` feature)** — prints the original chat lines to the terminal. No translation is done in this mode; translation runs in the GUI panel.

### Todo List

- [ ] Add in-gaming chat message interpreter


## Getting Started

The compiled executable can be found in the [releases](https://github.com/Yusux/Korabli-chatrans/releases) page. Choose the proper version and download the executable.

You can also compile the project yourself. The following instructions will help you to compile the project.

### *Prerequisites

- rustc
  - Installation: [Rustup](https://rustup.rs/)

### *Compilation

1. Clone the repository.
    ``` sh
    git clone git@github.com:Yusux/Korabli-chatrans.git
    cd Korabli-chatrans
    ```
2. Pull submodules.
    ``` sh
    git submodule update --init --recursive
    ```
    > P.S. This part is for getting the `scripts` folder unpacked from the game. We provide the latest scripts from the official server in [Korabli-scripts](https://github.com/Yusux/Korabli-scripts), but we cannot guarantee that it will be updated in time for every server update. If the previous version of Chatrans cannot work properly after the game version is updated, please try to update this submodule first or unpack the scripts yourself and put them in the `addons/scripts` folder.
3. Compile the project.
    ``` sh
    cargo build --release --features gui
    ```
    (Without `--features gui` you get a console-only build.)
4. The compiled executable `chatrans.exe` will be found in `target/release/` directory.

## Usage

### GUI mode (double-click)

Double-click `chatrans.exe` — no arguments needed:

1. **First run**: the settings view opens automatically. Chatrans tries to find your game directory for you (Windows registry → common paths such as `D:\Game\Korabli`); if it fails, use **浏览…** (browse) to pick the game root directory (the folder that contains the `replays` folder, e.g. `D:\Game\Korabli`).
2. **Translation endpoints** (optional): fill in an OpenAI- or Anthropic-compatible endpoint (name / base URL / API key / model). You can fetch the model list from the endpoint, or type model ids manually. Multiple endpoints are supported; the radio button picks the active one. Keys are stored encrypted with Windows DPAPI (`secrets.dat`) and never written in plain text.
3. Click **保存配置** (save). If the game path is valid, the panel immediately starts listening for battles.
4. Enter a battle — chat lines appear on the panel with `[友]`/`[敌]` team badges; team channel lines are translated by the active endpoint.

The panel starts **locked** (click-through, so it never steals clicks from the game). Click the padlock icon to unlock and drag/resize it; hover the top bar for 150 ms in locked mode to reach the icons. The gear icon opens/closes the settings view.

Configuration lives in `%LOCALAPPDATA%\KorabliOne\`:

| File | Content |
| --- | --- |
| `app.json` | Game directory (doubles as the "configured" flag) |
| `translation.json` | Translation endpoints (superset of the TomatoRadar addon schema — old addon configs are migrated automatically) |
| `secrets.dat` | API keys, DPAPI-encrypted |
| `placement.json` | Panel position/size |

### Console mode

``` powershell
.\chatrans.exe -r 'path\to\replays'
```

Prints original chat lines as they happen:

``` text
── 新对局开始：北方海峡 ──
[友] Alice·Minotaur: gg wp
[敌] Bob: нужна поддержка
```

No translation is performed in console mode — use the GUI panel for translation. `--headless` on a GUI build behaves the same way.

``` text
Usage: chatrans.exe [OPTIONS] --replay-dir <REPLAY_DIR>

Options:
  -r, --replay-dir <REPLAY_DIR>
          The replay dir to use
  -t, --target-language <TARGET_LANGUAGE>
          The target language, where `zh` stands for Chinese, `en` stands for English. Default is `zh` [default: zh]
      --access-key-id <ACCESS_KEY_ID>
          The Aliyun access key id
      --access-key-secret <ACCESS_KEY_SECRET>
          The Aliyun access key secret
      --headless
          Run without the translation overlay panel (console mode)
  -h, --help
          Print help
  -V, --version
          Print version
```

## Contributing

Any contributions you make are **greatly appreciated**. You can fork the repository and make a pull request. Also, you can open an issue if you find a bug or want to request a feature.

## License

Distributed under the MIT License. See [LICENSE](https://github.com/Yusux/Korabli-chatrans/blob/main/LICENSE) for more information.
