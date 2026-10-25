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

## Voice and screen

A Speech note stores a transcript. Record voice only while a Speech note is open. Set `GROQ_API_KEY` before you start the app. The audio clip is sent to Groq. The app does not save the audio file.

A Sight note stores a screen recording. Record the screen only while a Sight note is open. A new Sight note asks for the full display or one window before recording starts. The video button asks for the same choice later. Screen recording needs `ffmpeg`. The app writes an mp4 beside that note and appends a markdown link.

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

A Research rune asks for a question, then for a few clarifying answers. Charlotte writes a plan first. When that plan needs facts that are not in the question, Charlotte gathers those facts and revises the plan. The investigation then searches, reads, and writes. The file tree shows that run above Settings until the findings land in the note.

Set `OPENAI_API_KEY` before you start a Research rune. `OPENAI_BASE_URL` and `OPENAI_MODEL` are optional. The default model is `gpt-5.6-luna`.

Search uses SearXNG at `http://127.0.0.1:18088` unless `CHARLOTTE_SEARCH_BACKEND` or `CHARLOTTE_SEARXNG_URL` says otherwise. The read step opens a headless browser. Install Playwright's Chromium for that step. The run blocks private hosts. There is no switch to turn that block off.

The app skips hidden files and directories. It also skips symlinks. `[[wiki links]]` point at a note id. A bare stem matches one note when that stem is unique.

The user config is `$XDG_CONFIG_HOME/runebook/config.toml`. When `XDG_CONFIG_HOME` is unset, the path is `~/.config/runebook/config.toml`. It stores the last vault, the microphone, the theme, the default note view, and `dock_layout`.

The file tree, the note, and the Archivist column are dock panels. Their names are `files`, `note`, and `archivist`. `dock_layout` stores that layout as JSON.

## Tests

1. Run `just test` in this directory.
2. From the monorepo root, run `just runebook`.

## License

BSD-4-Clause
