# Runebook

[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![Rust edition 2024](https://img.shields.io/badge/rust-edition%202024-orange.svg)](Cargo.toml)
[![Rust 1.96+](https://img.shields.io/badge/rust-1.96%2B-orange.svg)](Cargo.toml)

**Markdown notes in a folder.** The window uses [GPUI Kit](https://gpui-kit.com). Notes stay on disk. Voice, screen recordings, research, and an Archivist sit beside the file tree.

The site is [runebook.sh](https://runebook.sh). A published binary installs with:

```bash
curl -fsSL https://runebook.sh | sh
```

That command covers Linux x86_64, macOS arm64, and macOS x86_64. It puts `runebook` on `~/.local/bin`. The vault stays on disk when you uninstall:

```bash
curl -fsSL https://runebook.sh/uninstall | sh
```

[Install from source](#install-from-source) · [Vault](#vault-format) · [Voice and screen](#voice-and-screen) · [Research](#research)

Release `1.0.0`. Every crate uses `[workspace.package].version` in [Cargo.toml](Cargo.toml).

## Contents

- [Requirements](#requirements)
- [Install from source](#install-from-source)
- [Quick start](#quick-start)
- [Crates](#crates)
- [Linux packages](#linux-packages)
- [macOS packages](#macos-packages)
- [Voice and screen](#voice-and-screen)
- [Vault format](#vault-format)
- [Research](#research)
- [Settings](#settings)
- [Troubleshooting](#troubleshooting)
- [Development](#development)
- [License](#license)

## Requirements

- Rust 1.96 or newer. GPUI Kit calls `std::hint::cold_path`. Rust 1.94 rejects that call.
- [just](https://github.com/casey/just), or use the `cargo` commands below.
- Sibling checkouts of [superglue](https://github.com/pinhole-io/superglue), [charlotte](https://github.com/pinhole-io/charlotte), and [notepad](https://github.com/pinhole-io/notepad). The path dependencies resolve next to this repository.
- `ffmpeg` for playback and screen recording.
- `GROQ_API_KEY` for voice transcription.
- An OpenAI-compatible key for the Archivist and for Research.

There is no crates.io package. `publish` is false.

## Install from source

Keep this directory name. The path dependencies use it.

```bash
mkdir -p ~/src && cd ~/src
git clone https://github.com/pinhole-io/runebook.sh.git
git clone https://github.com/pinhole-io/superglue.git
git clone https://github.com/pinhole-io/charlotte.git
git clone https://github.com/pinhole-io/notepad.git
```

You need this layout:

```text
~/src/
  runebook.sh/
  superglue/
  charlotte/
  notepad/
```

Research uses Charlotte's Playwright driver. Install Chromium for that driver:

```bash
npx playwright@1.58.2 install chromium
```

## Quick start

From this repository:

```bash
just app
```

The same binary:

```bash
cargo run -p runebook-app
```

One launch argument is a folder path.

```bash
cargo run -p runebook-app -- /path/to/folder
```

No argument opens the last vault. Two paths exit with status 2. A second launch is a second process.

## Crates

| Crate | Role |
| --- | --- |
| `runebook` | Reads and writes the vault |
| `runebook-preview` | Prepares preview text |
| `runebook-capture` | Records voice and the screen |
| `runebook-agent` | Runs the Archivist tools |
| `runebook-research` | Runs a research note |
| `runebook-app` | Desktop window and the `runebook` binary |

## Linux packages

GPUI draws with the GPU. On Debian or Ubuntu, install:

```bash
sudo apt install libxkbcommon-dev libxkbcommon-x11-dev libwayland-dev libvulkan1 libasound2-dev ffmpeg x11-utils
```

Use a Wayland or X11 session. This version does not target Windows.

The first start writes a desktop file. The shell uses that file for the book icon.

## macOS packages

Install the Xcode command line tools and Homebrew. Then install:

```bash
brew install ffmpeg pkgconf
```

Screen recording captures the full display. Playback needs `ffmpeg`. A click on a vault file uses `open`.

The first start writes a Finder Quick Action. Right-click a folder in Finder. Choose **Quick Actions**, then **Open in Runebook**. That folder opens as the vault. A folder with no `runebook.toml` becomes a new vault. Finder may need one restart before the item shows.

In the file tree, right-click a folder. Choose **Open in Finder**. Finder shows that folder.

## Voice and screen

**Warning:** Voice audio is sent to Groq. The app does not save that audio file. A screen recording writes an mp4 beside the note.

A Speech note stores a transcript. Record voice while that note is open. Another note can open while the capture runs. The transcript still lands in the Speech note. Set `GROQ_API_KEY` before you start the app.

A Sight note stores a screen recording. Record the screen while that note is open. Another note can open while the capture runs. The video link still lands in the Sight note. A new Sight note asks for the full display or one window before recording starts. A new Speech or Sight note waits until the capture ends. The file tree shows a mark on the recording note while voice or screen capture runs. A click on the mark stops the capture.

After that recording stops, the app transcribes the microphone and appends the text. It then reads one frame each second and appends a timeline and a summary. The summary uses the Archivist model. A clip longer than 3 minutes reviews the first 3 minutes. The app does not save the microphone audio.

On X11, `x11-utils` lists the windows. On Wayland, the compositor copies the screen. When `WAYLAND_DISPLAY` is set, that path is used. Sway lists each window with `swaymsg`.

A preview row that is only a video or audio link plays in the note. A video row shows the first frame before you press Play. The picture follows the file frame rate. Pause holds the place and leaves the picture. Play continues from that place. The bar seeks. Mute silences the sound. A preview image shows in the row. A click on a vault image opens the file with `xdg-open`. A remote image loads from its URL. The alt text shows when the image does not load. The link can name a video (`mp4`, `webm`, `mkv`, `mov`), an audio file (`mp3`, `wav`, `ogg`, `m4a`, `flac`), or an image (`png`, `jpg`, `jpeg`, `gif`, `webp`). The file must stay inside the vault. Playback needs `ffmpeg`.

Folder delete removes those media files. Any other non-note file still blocks the delete.

## Vault format

A vault is a folder with `runebook.toml`:

```toml
schema_version = 1
name = "Personal"
```

A note is any `*.md` file under that folder. The note id is the path without `.md`. Example: `journal/2026-10-23.md` has id `journal/2026-10-23`.

Each note has one kind.

| Kind | Name in the window | File |
| --- | --- | --- |
| Book | Book Runes (Bókrunar) | Markdown. The file has no kind header. |
| Speech | Speech Runes (Málrunar) | A transcript. The header says `kind: speech`. |
| Sight | Sight Runes (Sjónrunar) | A screen recording. The header says `kind: sight`. |
| Research | Research Runes (Rannsóknarrúnar) | An investigation. The header says `kind: research`. |

A file with no header is a Book note. Speech, Sight, and Research use this header:

```markdown
---
kind: speech
---
```

Use `speech`, `sight`, or `research`. The editor shows the body. Save writes the header back. An old `kind: deed` header opens as a Book note. The next save drops that header.

A task is a checklist line in any note. `- [ ]` is open. `- [x]` is done. `📅 YYYY-MM-DD` is the due date. `✅ YYYY-MM-DD` is the done date. A click on a preview checkbox toggles that line and writes the done date. Control+Shift+T inserts a task into the open note. The due date uses a calendar. A cleared date leaves the task undated. The Tasks list in the sidebar shows tasks from the vault.

The app skips hidden files and directories. It also skips symlinks. `[[wiki links]]` point at a note id. A bare stem matches one note when that stem is unique.

## Research

**Warning:** A research run blocks private hosts. There is no switch to turn that block off.

A Research rune asks for a question, then for a few clarifying answers. [Charlotte](https://github.com/pinhole-io/charlotte) writes a plan first. When that plan needs facts that are not in the question, Charlotte gathers those facts and revises the plan. The note shows that plan. The investigation then searches, reads, and writes. The file tree shows that run above Settings until the findings land in the note.

Set the OpenAI key, hostname, and model in Settings. An empty OpenAI key uses `OPENAI_API_KEY`, then `OPENROUTER_API_KEY`, `OPENROUTER_GATEWAY_KEY`, `XAI_API_KEY`, `RUNINFRA_GATEWAY_KEY`, or `VERCEL_GATEWAY_KEY`. An empty host uses `OPENAI_BASE_URL`. An empty model uses `OPENAI_MODEL`. The default model is `gpt-5.6-luna`. Tool rounds default to 250. Search and read use that cap. Plan stays at 1 round. Write stays at 2 rounds. Voice uses the Groq key in Settings, then `GROQ_API_KEY`. A `groq:` Archivist model uses that same Groq key. Any other Archivist model uses the OpenAI-compatible key and host. The key fields open with the environment value.

Search uses SearXNG at `http://127.0.0.1:18088` unless `CHARLOTTE_SEARCH_BACKEND` or `CHARLOTTE_SEARXNG_URL` says otherwise. The read step opens a headless browser.

Keep provider keys in the environment or in Settings. Do not commit them.

## Settings

The user config is `$XDG_CONFIG_HOME/runebook/config.toml`. When `XDG_CONFIG_HOME` is unset, the path is `~/.config/runebook/config.toml`. It stores the last vault, the last note, the microphone, the theme, the window language, the text size, the default note view, the editor choices, the autosave pace, and `dock_layout`.

A theme file uses the same keys as a VS Code color theme. Copy `name`, `type`, `colors`, and `tokenColors`. `type` is `dark` or `light`. Put the file in `$XDG_CONFIG_HOME/runebook/themes`. The app lists that file in Settings. `theme = "dark"` and `theme = "light"` still open Runebook Dark and Runebook Light. An unknown name uses Runebook Dark. The app ignores `semanticTokenColors`. An `include` must be one `.json` file in the same folder. Font, radius, and note spacing sit in an optional `runebook` object.

The app also ships Obsidian Dark, Obsidian Light, Blue Topaz Dark, and Blue Topaz Light. It also ships 15 color themes from the VS Code marketplace. [themes/NOTICE](themes/NOTICE) lists each file, the source, and the license. Monokai Pro and Material Theme are not included. Their licenses forbid redistribution.

Settings groups those choices. Appearance holds the theme, the window language, the text size, and the note view. Editor holds line numbers, line wrap, and autosave. Capture holds the microphone, the speech model, the spoken language, and mute. Models holds the keys, the host, and the model names. Vault reopens the last note when that note is still in the vault.

The window language is `locale`. `en` is English. A blank or unknown code uses English. The sentences live in `locales/en.json`. A new language is another file with the same keys.

Autosave `normal` waits 400 ms. `fast` waits 200 ms. `slow` waits 1 second. `manual` writes the note when you leave it or close the window. Speech `fast` uses `whisper-large-v3-turbo`. Speech `accurate` uses `whisper-large-v3`. An empty language lets Groq detect it.

The file tree reads the vault every 500 ms. A change made outside the app shows up on that read. The refresh button reads immediately. A clean open note reloads when its file changed. Unsaved text stays in the editor.

The file tree, the note, and the Archivist column sit side by side. The two side columns stop at the sidebar minimum. `dock_layout` stores the files width and the Archivist width. An older dock JSON still supplies those widths.

## Troubleshooting

| What you see | What to do |
| --- | --- |
| `can't find crate superglue` or `charlotte` | Clone the sibling repositories in [Install from source](#install-from-source). Keep the directory name `runebook.sh`. |
| The window does not open on Linux | Install the packages in [Linux packages](#linux-packages). Use a Wayland or X11 session. |
| Playback or screen recording fails | Install `ffmpeg`. |
| Voice does not transcribe | Export `GROQ_API_KEY`, or set the Groq key in Settings. |
| Research cannot reach a model | Set a provider key in Settings, or export `OPENAI_API_KEY`. |
| Research search returns no rows | Start SearXNG at `http://127.0.0.1:18088`, or set `CHARLOTTE_SEARXNG_URL`. |
| Rust rejects `std::hint::cold_path` | Install Rust 1.96 or newer. |

## Development

```bash
just test
cargo fmt
cargo clippy --workspace --all-targets -- -D warnings
```

`just test` runs `cargo test`. `just app` runs `cargo run -p runebook-app`.

Bump `[workspace.package].version` in `Cargo.toml`. The member crates inherit it.

## License

The license is [MIT](LICENSE).

Keep the copyright notice and the permission notice in every copy.

Color themes shipped in `themes/` keep the licenses listed in [themes/NOTICE](themes/NOTICE).
