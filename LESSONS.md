# Lessons — Submarine

## Write manual test steps in the app's UI words, not the code's (2026-10-05)

The code says "credential" / "node"; the UI says **Logins** (key icon in the sidebar, "Add Password") and
**servers**, and a server picks one under **How to log in → Use a saved login** in its editor (pencil
**Edit**, only visible on hover, right next to **Clone**). Telling the user to "create a saved credential"
made them create a new *profile* instead. Before giving click-through steps, grep the component for the
exact label (`Sidebar.tsx`, `AddNodePanel.tsx`, `NodeGrid.tsx`).

## `src-tauri/src/lib.rs` is CRLF — never edit it with text-mode scripts (2026-10-05)

`lib.rs` uses CRLF line endings and ends in a bare `\r` with no final newline. A Python `open(p).read()` /
`write()` round-trip silently rewrote all ~11,000 lines to LF (a 22,000-line diff). Edit it with the Edit tool,
or in binary mode (`open(p,'rb')`) preserving `\r\n`. Check with `git diff --stat` after any scripted edit;
`file <path>` shows "with CRLF line terminators". The other edited files (`*.tsx`, `TODO.md`) are LF.
