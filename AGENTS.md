# ccview-rs

A ratatui TUI to keep track of, read and use many coding-agent sessions from one screen. Claude Code is the main harness; Codex and OpenCode are secondary; more harnesses will follow, so anything harness-specific stays behind `sources/*` and the `Harness` trait. The owner wants one UI for listing, reading, resuming, forking and embedding sessions, not a Claude-only viewer. This file is the single source of truth (`CLAUDE.md` imports it). Statements marked PLANNED are not built; do not describe them as existing, and do not build them unless asked. Statements are from the code at the time of writing; if the code disagrees, the code wins and this file gets fixed.

## Architecture

Library crate (`src/lib.rs`) plus a thin binary (`src/main.rs`: no args starts the TUI; `--export <out.html|out.md|out.jsonl> [--file <session.jsonl>]`, `--live`, `--file <path>` are CLI paths without the TUI).

| Module | One job |
|---|---|
| `terminal/` | Embedded terminal host. `host.rs` PTY (portable-pty) plus reader thread feeding a vt100 screen, 5000 lines scrollback, waker callback; `input.rs` key/mouse/paste encoding (kitty keyboard flags); `responder.rs` replies to terminal queries and tracks modes; `state.rs` keyboard flags and event queue; `spec.rs` `CommandSpec` (program, args, cwd, env, env_remove); `widget.rs` ratatui widget |
| `registry.rs` | Reads `<claude config>/sessions/*.json` (live Claude sessions, pid alive, status); `RegistryWatcher` stats the files and parses only the changed ones |
| `live.rs` | The live thread: polls the registry every 250 ms, recomputes activity on change and every 500 ms, asks the `claude agents --json` worker to run when a session's state or a job's state changes (at most every 500 ms, otherwise every 5 s), sends `Update::{Live, Activity}` only when they changed |
| `stamp.rs` | `Stamp` (mtime plus length) for cheap change detection |
| `activity.rs` | Derives per-session state (busy, blocked, background, agents) from registry, `agents_cli.rs` and `subagents.rs`; `attention_rank` orders the Active tab |
| `agents_cli.rs` | Runs `claude agents --json` (3 s timeout) only if the Claude daemon lock shows a live daemon, or `CCVIEW_CLAUDE_BIN` is overridden (how the PTY test injects a fake) |
| `subagents.rs` | Running subagents from `<project>/<session>/subagents/agent-*.meta.json` (10 min staleness window, ended early by `finished`, `stoppedByUser`, a final `end_turn`, or the parent transcript's `queue-operation` task-notification for the agent's `toolUseId`) |
| `library.rs` | `SessionEntry`, `Scanner`, `Update` enum, `Feed`, `spawn()` source scan thread (Claude, Codex, OpenCode every 1 s; OpenCode only when the database or its `-wal` stat changed); merges all harness sources, orders by `last_used` |
| `index_cache.rs` | Persistent per-file index (size, mtime, scanned bytes, title facts) so unchanged files are not re-read |
| `discover.rs`, `parse.rs`, `tail.rs` | Claude project dir, JSONL line parser into `Message`/`Block`, Claude line follower |
| `sources/` | `HarnessId`, `SourceEntry`, `TranscriptHandle`; `codex.rs` (rollouts under `$CODEX_HOME/sessions`), `opencode.rs` (SQLite, read-only), `follow.rs` (`JsonlFollower`: offset, inode, partial line) |
| `harness.rs` | `Harness` trait, `Launch`/`LaunchKind`, argv for Claude/Codex/OpenCode |
| `external.rs` | Opens a session in an external macOS terminal app through a temp `.command` script |
| `favorites.rs` | Favourites file, `config_file()` and `write_atomically()` shared by other persisted files |
| `markdown.rs`, `export.rs` | Markdown to ratatui lines; `--export` writers |
| `tui/mod.rs` | `App`, `Focus`, key routing, draw. Must not grow: new behaviour goes in a submodule |
| `tui/runtime.rs` | Event loop, frame throttle, mouse-capture toggle |
| `tui/list.rs`, `tabs.rs`, `badges.rs` | List pane, tab bar (All, Favorites, Active, one per directory), activity badges |
| `tui/transcript.rs`, `follow.rs` | Transcript selection with 120 ms settle delay and a small cache (6 sessions, 6000 messages); follower threads |
| `tui/conversation.rs`, `verbosity.rs`, `tool_block.rs`, `diff.rs`, `timefmt.rs` | Rendering of messages at a verbosity level, tool blocks, line diffs, local timestamps |
| `tui/embedded.rs`, `sessions.rs`, `prefix.rs`, `launch.rs` | Embedded panes, `SessionManager`, `^Space` prefix state machine, launch requests and confirmations |
| `tui/identity.rs`, `palette.rs`, `colors.rs` | Per-session colour assignment (14 hues), persisted overrides |
| `tui/footer.rs`, `theme.rs` | Key hints per mode; all styles and colour constants |

Data flow:
- `library::spawn` starts a source scan thread and the live thread (`live.rs`), and takes a wake callback that posts `Message::Updated` so the loop drains updates immediately. It sends `Update::{Scanned, Removed, Live, Activity, InitialScanDone}` over an mpsc channel; `App` drains it in `tick()` and rebuilds tabs and rows. Cached entries are sent before the first scan finishes.
- Selecting a session starts a follower thread (`tui/follow.rs`) per `TranscriptHandle`; it sends `Batch::{Appended, Replaced}` that `Transcript` drains.
- Each embedded `PtyHost` has a waker that sends `Message::Output(id)` to the event loop, so output redraws without polling. Input is read on its own thread (`Message::Input`). The loop draws at most every 16 ms and polls every 50 ms when idle.

Persisted files (written atomically):

| File | Location | Override |
|---|---|---|
| `favorites.json`, `hidden.json`, `colors.json`, `view.json`, `ui.json` (`split_ratio`, `mouse`) | `<config>/ccview-rs/` | `XDG_CONFIG_HOME`, else `$HOME/.config` |
| `index-v1.json` | `<cache>/ccview-rs/` | `XDG_CACHE_HOME`, else `$HOME/.cache` |

Sources read (never write): Claude `$CLAUDE_CONFIG_DIR/projects` else `~/.claude/projects`, plus `~/.claude/sessions` and `~/.claude/daemon.lock`; Codex `$CODEX_HOME` else `~/.codex`; OpenCode `$XDG_DATA_HOME/opencode/opencode.db` else `~/.local/share/opencode/opencode.db`.

Other env vars read: `CCVIEW_CLAUDE_BIN`, `CCVIEW_CODEX_BIN`, `CCVIEW_OPENCODE_BIN` (launch binary, default `claude`/`codex`/`opencode`), `CCVIEW_TERMINAL_APP` (default `Terminal`, falls back on `TERM_PROGRAM`), `CCVIEW_OPEN_BACKGROUND` (any non-empty value adds `-g` to `open`). `CCVIEW_TZ_CHILD` is an internal flag of a timezone test.

## Commands

Run `export PATH="/opt/homebrew/opt/rustup/bin:$PATH"` first if `cargo` is not found. Edition 2024, no `rust-version` pinned.

- Build: `cargo build`
- Test everything: `cargo test`. Last run: 125 unit tests plus 5 PTY integration files, all green, about 15 s.
- One file: `cargo test --test terminal_protocol`; unit tests: `cargo test --lib`.
- Install: `cargo install --path . --locked` installs the single binary `ccview-rs` into `~/.cargo/bin`. The commands `ccview-rs` and `ccv` on this machine are symlinks in `/opt/homebrew/bin` to that binary; `ccv` is not defined in this repo. Verified with `--root <temp dir>`; the real install was not run from here.
- Run the real UI: `cargo run`. Run against fakes: `HOME=<tmp> XDG_CONFIG_HOME=<tmp>/cfg XDG_CACHE_HOME=<tmp>/cache CCVIEW_CLAUDE_BIN=/usr/bin/false cargo run`, with transcripts crafted under `<tmp>/.claude/projects/<dir>/<id>.jsonl` (see `tests/view_level_pty.rs` and `tests/activity_pty.rs` for the minimal shapes). Not run interactively here.
- Test caveats: the tests in `tui/follow.rs`, `tui/transcript.rs`, `tui/mod.rs` poll with 10-30 s deadlines, so load shows up as timeouts rather than wrong answers. Nothing here needs `--test-threads=1`; no per-package issue exists (single package).

## Safety rules

These are hard. A test or tool that breaks one is a bug.

1. Never start a real `claude`, `codex` or `opencode` from tests or tools. Point `CCVIEW_CLAUDE_BIN`, `CCVIEW_CODEX_BIN`, `CCVIEW_OPENCODE_BIN` at a fake script or `/usr/bin/false` (as `tests/harness_launch_pty.rs` does).
2. Tests never write to `~/.claude`, `~/.codex`, the OpenCode database, `~/.config/ccview-rs` or `~/.cache/ccview-rs`. Use a temp dir and override `HOME`, `XDG_CONFIG_HOME`, `XDG_CACHE_HOME`, `CLAUDE_CONFIG_DIR`, `CODEX_HOME`, `XDG_DATA_HOME`. Unit tests construct `Favorites`, `Identity`, `ViewSetting` and `IndexCache` with explicit temp paths; never call their `load_default`.
3. Never open a GUI app or terminal window from tests. `external::open_with` takes an opener closure; inject a fake.
4. The only `claude` subcommand ccview runs on its own is the read-only `claude agents --json`. Never run `claude agents` interactively, `claude attach` or `--bg` from tests or tools. `attach` exists only as a user-triggered launch (key `a`).
5. OpenCode's database is opened with `SQLITE_OPEN_READ_ONLY` and `PRAGMA query_only = ON` (`sources/opencode.rs::open_read_only`). Keep it that way; use no other open path.
6. Session transcripts are private data. No transcript content in reports, logs, commit messages or fixtures. Fixtures are synthetic (`/work/app`, short invented prompts).
7. No customer or personal names in code, fixtures or docs.

## Code conventions

- Simplest valid solution. No abstraction nobody asked for. Fix root causes, not symptoms.
- No code comments. Make the code clearer with names, constants and small functions. The only exception is a constraint imposed from outside the file. Rationale goes in the commit message.
- One job per module. `tui/mod.rs` (about 830 lines, much of it tests) must not grow; new UI behaviour gets its own submodule.
- `parse::Message` and `parse::Block` are the shared shape every harness converts into (`parse.rs`, `sources/codex.rs`, `sources/opencode.rs`). Do not change them casually; a change touches all converters, the renderer and the exporter.
- Tests only where they protect behaviour: a test must fail when the change is reverted, and is derived from intent, not from the implementation. A bug gets a regression test; a feature gets a few tests on its core behaviour. No per-function test sprawl, no tests for code that merely forwards.
- No new dependency without a stated reason in the commit. Current set is in `Cargo.toml`.
- Performance budget: no file or process IO on the UI thread (scanning, following, `claude agents --json` run on their own threads); per-frame work is cached (conversation lines per width and level, tab and row lists on change); keep frame time in the hundreds of microseconds; the transcript shows at most the latest 1500 messages (`MAX_INITIAL_MESSAGES`, with a `hidden_earlier` count).

## Parallel work

- One git worktree per task under the session scratchpad. Each agent owns specific files. Edits to shared files stay small and are listed in the agent's report.
- Conflicts concentrate in `tui/mod.rs`, `tui/footer.rs` and `tui/theme.rs`. History merges task branches with merge commits ("Merge <feature>") onto `master`; fast-forward is fine when the branch is current. After a merge with hand-resolved conflicts run `cargo build` and the full `cargo test`.
- Remove the worktree and branch when merged; never leave worktrees behind. Worktrees under scratch or tmp are lost on reboot, so keep unmerged work on a branch.
- Identity: this repo lives under `~/Documents/Personal`, where git applies the personal GitHub identity through `includeIf` (config in `~/.gitconfig-vriken`). Worktrees elsewhere do not get it. There, commit with `git -c user.name="$(git config -f ~/.gitconfig-vriken user.name)" -c user.email="$(git config -f ~/.gitconfig-vriken user.email)" commit ...`. Do not hard-code them. Existing history has a single author.
- Commit trailer: `Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>`. Branch from the current `master`. Do not commit or push unless asked.

## Product decisions already made

- Focus model: three zones, `Focus::{List, Conversation, Embedded}`, one focused at a time (focused border and title marker; others dimmed). The right pane shows the embedded session if one is bound to the selected entry, otherwise the transcript.
- Keys, list and transcript: the list has two sub-focuses. In the rows `Right`/`l`/`Enter`/`Tab` focuses the right pane, `Up` from the first row moves to the tab bar, `x` hides the selected session (persisted in `hidden.json`, never touches transcripts), `H` shows hidden sessions dimmed. On the tab bar `Left/Right`, `h/l` or `[ ]` switch tabs and `Down`/`Enter`/`Esc` return to the rows; `Left`, `h`, `Esc` or `Tab` in the transcript returns to the list; `Ctrl+C` or `q` quits, with a confirm only while embedded sessions are running. Actions: `r` resume, `f` fork, `n` new, `c` continue, `a` attach (Claude only), `o`/`O` external terminal, `s` or space star, `C` cycle colour, `v` cycle verbosity.
- Keys, embedded: everything is forwarded to the child, including `Ctrl+C`. The only interception is the `^Space` prefix: `^Space l`/`Esc` back to list, `x` close (confirm), `[`/`]` or arrows previous/next session, `PgUp/PgDn` or `Up/Down` scroll the child, `C` colour, `^Space ^Space` sends a literal `^Space`. An unknown key after the prefix is dropped. A key press on an exited session dismisses it.
- Verbosity levels `chat`, `tools`, `everything`, cycled by `v`, persisted in `view.json`.
- Identity colours: 14 hues (`tui/palette.rs`), a stable default from a hash of the session id, no two tracked sessions share a hue while free ones remain, overrides persisted in `colors.json`. Colours are tracked for running embedded sessions and sessions with a live pid, and the list bar is drawn only on those active rows.
- Mouse capture is off by default so the terminal's own selection and scrolling work. It is turned on only while a running embedded session is focused and that session enabled a mouse protocol or the alternate screen; the wheel is then forwarded (or turned into arrow keys on the alternate screen, 3 lines per notch).
- Claude launches (new, resume, fork, continue) set `CLAUDE_CODE_DISABLE_AGENT_VIEW=1` and remove a named list of inherited session-identity variables (`CLAUDECODE`, `CLAUDE_CODE_CHILD_SESSION`, `_ENTRYPOINT`, `_EXECPATH`, `_MESSAGING_SOCKET`, `_MESSAGING_TOKEN`, `_SESSION_ATTENDED`, `_SESSION_ID`, `CLAUDE_PID`) so a nested session does not believe it is inside another; user-set variables such as `CLAUDE_CODE_USE_BEDROCK` are kept, and `CLAUDE_BG_*` variables are not stripped. `attach` is the exception: it keeps the environment.
- Refresh cadences: registry stat every 250 ms (`live.rs`), `claude agents --json` on change (min gap 500 ms) or every 5 s, sub-agents every 500 ms, source rescan every 1 s (`library.rs`), transcript follow poll 500 ms, transcript settle delay 120 ms, frames at most every 16 ms.
- Ordering: the All list is sorted by `last_used` descending (path as tiebreak); the Active tab is sorted by attention rank.
- Launching a session already open in an embedded pane asks for confirmation (`Confirm::Duplicate`).
- Split: the list/detail ratio (default 0.35, list at least 24 columns, detail at least 30) is persisted in `ui.json`; resize with `<` `>` `=` (prefix first inside an embedded session) or drag the divider; mouse capture is on unless `mouse` is false in `ui.json`.
- In an embedded session `Left` returns to the list only when the cursor is visible and nothing but prompt glyphs, box drawing or whitespace is left of it (`tui/back_to_list.rs`); Ctrl+C is always forwarded; a child that exits 0 returns focus to the list automatically.
- The running-sessions strip, the transcript and session pane titles and the footer chip name the harness (`claude`, `codex`, `opencode`); list rows do not. In a row's detail line the status (`idle`, `busy`, badges) comes first so it is not clipped.
- PLANNED (the owner's open requests, not in the code): settings and help overlays, collapsing continuation copies of one conversation into one row.

## Known limitations

Verified in code:
- OSC 8 hyperlinks are not rendered in embedded panes; `responder.rs` ignores unknown OSC sequences (only OSC 9;4 progress is recognised and dropped).
- Synchronized output (mode 2026) is tracked and reported to the child but output is not buffered by it; `PtyHost::synchronized_output()` is only exercised by `tests/terminal_protocol.rs`, not by drawing.
- External terminal launch is macOS only (`external.rs`).
- Unix only (`libc`, `os::unix`).
- The Claude daemon check and `claude agents --json` depend on a Claude version that has them; absence yields no agent badges rather than an error.

Not verified:
- Claude embedded behaviour past the trust dialog: no test goes beyond a fake binary.
- `tests/terminal_protocol.rs` flaking under load: one run under CPU load did not complete in the time available, so it is neither confirmed nor ruled out; it passed in the normal runs.
- Real Codex and OpenCode launch behaviour beyond argv: tested with fake binaries only.
