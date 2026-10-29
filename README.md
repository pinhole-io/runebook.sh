# Runebook

Runebook is a local markdown note app. The window uses [GPUI Kit](https://gpui-kit.com).

## Crates

- `runebook` reads and writes the vault.
- `runebook-app` is the desktop window.

## Run the app

1. Install Rust stable 1.96 or newer.
2. Install the Linux packages in the next section.
3. Run `just app` in this directory.

From the monorepo root, run `just runebook-app`.

GPUI Kit calls `std::hint::cold_path`. Rust 1.94 rejects that call.

## Linux packages

GPUI draws with the GPU. On Debian or Ubuntu, install:

```bash
sudo apt install libxkbcommon-dev libxkbcommon-x11-dev libwayland-dev libvulkan1 libasound2-dev ffmpeg x11-utils
```

Use a Wayland or X11 session. The app does not target Windows in this version.

## macOS packages

Install the Xcode command line tools and Homebrew. Then install:

```bash
brew install ffmpeg pkgconf
```

Screen recording captures the full display. Playback needs `ffmpeg`. A click on a vault file uses `open`.

## Voice and screen

A Speech note stores a transcript. Record voice while that note is open. Another note can open while the capture runs. The transcript still lands in the Speech note. Set `GROQ_API_KEY` before you start the app. The audio clip is sent to Groq. The app does not save the audio file.

A Sight note stores a screen recording. Record the screen while that note is open. Another note can open while the capture runs. The video link still lands in the Sight note. A new Sight note asks for the full display or one window before recording starts. A new Speech or Sight note waits until the capture ends. The file tree shows a mark on the recording note while voice or screen capture runs. A click on the mark stops the capture. Screen recording needs `ffmpeg`. The app writes an mp4 beside that note and appends a markdown link.

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
| Deed | Deed Runes (Verkrunar) | A task list. The header says `kind: deed`. |
| Research | Research Runes (Rannsóknarrúnar) | An investigation. The header says `kind: research`. |

A file with no header is a Book note. Speech, Sight, Deed, and Research use this header:

```markdown
---
kind: speech
---
```

Use `speech`, `sight`, `deed`, or `research`. The editor shows the body. Save writes the header back. A click on a preview checkbox toggles that task line in the note.

## Research

A Research rune asks for a question, then for a few clarifying answers. Charlotte writes a plan first. When that plan needs facts that are not in the question, Charlotte gathers those facts and revises the plan. The note shows that plan. The investigation then searches, reads, and writes. The file tree shows that run above Settings until the findings land in the note.

Set the OpenAI key, hostname, and model in Settings. An empty OpenAI key uses `OPENAI_API_KEY`, then `OPENROUTER_API_KEY`, `OPENROUTER_GATEWAY_KEY`, `XAI_API_KEY`, `RUNINFRA_GATEWAY_KEY`, or `VERCEL_GATEWAY_KEY`. An empty host uses `OPENAI_BASE_URL`. An empty model uses `OPENAI_MODEL`. The default model is `gpt-5.6-luna`. Tool rounds default to 250. Search and read use that cap. Plan stays at 1 round. Write stays at 2 rounds. Voice uses the Groq key in Settings, then `GROQ_API_KEY`. A `groq:` Archivist model uses that same Groq key. Any other Archivist model uses the OpenAI-compatible key and host. The key fields open with the environment value.

Search uses SearXNG at `http://127.0.0.1:18088` unless `CHARLOTTE_SEARCH_BACKEND` or `CHARLOTTE_SEARXNG_URL` says otherwise. The read step opens a headless browser. Install Playwright's Chromium for that step. The run blocks private hosts. There is no switch to turn that block off.

The app skips hidden files and directories. It also skips symlinks. `[[wiki links]]` point at a note id. A bare stem matches one note when that stem is unique.

The user config is `$XDG_CONFIG_HOME/runebook/config.toml`. When `XDG_CONFIG_HOME` is unset, the path is `~/.config/runebook/config.toml`. It stores the last vault, the last note, the microphone, the theme, the window language, the text size, the default note view, the editor choices, the autosave pace, and `dock_layout`.

A theme file uses the same keys as a VS Code color theme. Copy `name`, `type`, `colors`, and `tokenColors`. `type` is `dark` or `light`. Put the file in `$XDG_CONFIG_HOME/runebook/themes`. The app lists that file in Settings. `theme = "dark"` and `theme = "light"` still open Runebook Dark and Runebook Light. An unknown name uses Runebook Dark. The app ignores `semanticTokenColors`. An `include` must be one `.json` file in the same folder. Font, radius, and note spacing sit in an optional `runebook` object.

The app also ships Obsidian Dark, Obsidian Light, Blue Topaz Dark, and Blue Topaz Light. It also ships 15 color themes from the VS Code marketplace. `themes/NOTICE` lists each file, the source, and the license. Monokai Pro and Material Theme are not included. Their licenses forbid redistribution.

Settings groups those choices. Appearance holds the theme, the window language, the text size, and the note view. Editor holds line numbers, line wrap, and autosave. Capture holds the microphone, the speech model, the spoken language, and mute. Models holds the keys, the host, and the model names. Vault reopens the last note when that note is still in the vault.

The window language is `locale`. `en` is English. A blank or unknown code uses English. The sentences live in `locales/en.json`. A new language is another file with the same keys.

Autosave `normal` waits 400 ms. `fast` waits 200 ms. `slow` waits 1 second. `manual` writes the note when you leave it or close the window. Speech `fast` uses `whisper-large-v3-turbo`. Speech `accurate` uses `whisper-large-v3`. An empty language lets Groq detect it.

The file tree reads the vault every 500 ms. A change made outside the app shows up on that read. The refresh button reads immediately. A clean open note reloads when its file changed. Unsaved text stays in the editor.

The file tree, the note, and the Archivist column are dock panels. Their names are `files`, `note`, and `archivist`. `dock_layout` stores that layout as JSON.

## Tests

1. Run `just test` in this directory.
2. From the monorepo root, run `just runebook`.

## License

BSD-4-Clause
