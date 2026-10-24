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

Voice appends a transcript to the open note. Set `GROQ_API_KEY` before you start the app. The audio clip is sent to Groq. The app does not save the audio file.

Screen recording needs `ffmpeg`. The video button asks for the full display or one window. The app writes an mp4 beside the note and appends a markdown link.

On X11, `x11-utils` lists the windows. On Wayland, the compositor copies the screen. When `WAYLAND_DISPLAY` is set, that path is used. Sway lists each window with `swaymsg`.

A media link in the preview opens the file with `xdg-open`. The link can name a video (`mp4`, `webm`, `mkv`, `mov`), an audio file (`mp3`, `wav`, `ogg`, `m4a`, `flac`), or an image (`png`, `jpg`, `jpeg`, `gif`, `webp`). The file must stay inside the vault.

Folder delete removes those media files. Any other non-note file still blocks the delete.

## Vault format

A vault is a folder with `runebook.toml`:

```toml
schema_version = 1
name = "Personal"
```

A note is any `*.md` file under that folder. The note id is the path without `.md`. Example: `journal/2026-10-23.md` has id `journal/2026-10-23`.

The app skips hidden files and directories. It also skips symlinks. `[[wiki links]]` point at a note id. A bare stem matches one note when that stem is unique.

The user config is `$XDG_CONFIG_HOME/runebook/config.toml`. When `XDG_CONFIG_HOME` is unset, the path is `~/.config/runebook/config.toml`. It stores the last vault, the microphone, the theme, and the default note view.

## Tests

1. Run `just test` in this directory.
2. From the monorepo root, run `just runebook`.

## License

BSD-4-Clause
