# ccview-rs

A ratatui TUI to keep track of, read and use many coding-agent sessions from one screen. Claude Code is the main harness; Codex and OpenCode are secondary; more harnesses will follow, so anything harness-specific stays behind `sources/*` and the `Harness` trait. The owner wants one UI for listing, reading, resuming, forking and embedding sessions, not a Claude-only viewer. This file is the single source of truth (`CLAUDE.md` imports it). Statements marked PLANNED are not built; do not describe them as existing, and do not build them unless asked. Statements are from the code at the time of writing; if the code disagrees, the code wins and this file gets fixed.

## Architecture

Library crate (`src/lib.rs`) plus a thin binary (`src/main.rs`: no args starts the TUI; `--export <out.html|out.md|out.jsonl> [--file <session.jsonl>]`, `--live`, `--file <path>` are CLI paths without the TUI; `host` runs the background session host, `host --list` prints its sessions).

| Module | One job |
|---|---|
| `terminal/` | Embedded terminal and the session host. `host.rs` `PtyHost`: PTY (portable-pty) plus reader thread feeding a vt100 screen, 5000 lines scrollback, waker callback, `attach` (atomic screen snapshot plus bounded output subscription); `session.rs` the `TerminalSession` trait the UI draws and types into (implemented by `PtyHost` and `RemoteHost`); `remote.rs` `RemoteHost`: a client-side vt100 mirror fed by the attach snapshot and relayed bytes, never replies to the child; `daemon.rs` the session host process (`ccview-rs host`): socket, lock, one `PtyHost` per session, idle exit; `client.rs` `HostClient` (list, spawn, dismiss, attach; starts the daemon on demand); `protocol.rs` length-prefixed frames and the `Request`/`Response` messages; `runtime_dir.rs` socket, lock, pid and log paths; `input.rs` key/mouse/paste encoding (kitty keyboard flags); `responder.rs` replies to terminal queries and tracks modes; `state.rs` keyboard flags, event queue, `ModeSnapshot`; `spec.rs` `CommandSpec` (program, args, cwd, env, env_remove); `widget.rs` ratatui widget |
| `registry.rs` | Reads `<claude config>/sessions/*.json` (live Claude sessions, pid alive, status); `RegistryWatcher` stats the files and parses only the changed ones |
| `live.rs` | The live thread: polls the registry every 250 ms, recomputes activity on change and every 500 ms, asks the `claude agents --json` worker to run when a session's state or a job's state changes (at most every 500 ms, otherwise every 5 s), sends `Update::{Live, Activity}` only when they changed |
| `stamp.rs` | `Stamp` (mtime plus length) for cheap change detection |
| `activity.rs` | Derives per-session state (busy, blocked, background, agents) from registry, `agents_cli.rs` and `subagents.rs`; `attention_rank` orders the Active tab |
| `agents_cli.rs` | Runs `claude agents --json` (3 s timeout) only if the Claude daemon lock shows a live daemon, or `CCVIEW_CLAUDE_BIN` is overridden (how the PTY test injects a fake) |
| `subagents.rs` | Running subagents from `<project>/<session>/subagents/agent-*.meta.json` (10 min staleness window, ended early by `finished`, `stoppedByUser`, a final `end_turn`, or the parent transcript's `queue-operation` task-notification for the agent's `toolUseId`) |
| `library.rs` | `SessionEntry`, `Scanner`, `Update` enum, `Feed`, `spawn()` source scan thread (Claude, Codex, OpenCode every 1 s; OpenCode only when the database or its `-wal` stat changed); merges all harness sources, orders by `last_used`; after every merge/remove groups lineages (`lineage::group`) and sets `SessionEntry::{copy_of, copies}`; `Library::activity()` and the visible entry's `live` are lineage-effective while collapsed (the most active member's state); `set_expanded` is the in-memory `E` toggle |
| `lineage.rs` | `Fingerprint` (uuid count plus a bottom-128 sketch of 32-bit uuid hashes, kept in the index record), `relation`, `group`: conversation lineage detection, see Product decisions |
| `dismissed.rs` | `Dismissed`: sessions the owner removed from the active set (`dismissed.json`, session id to the pid dismissed for) and `reconcile`, the clearing rule. Imports nothing that can signal a process; a test asserts it |
| `favorite_dirs.rs` | Starred directories (`favorite_dirs.json`), same pattern as `favorites.rs` |
| `index_cache.rs` | Persistent per-file index (size, mtime, scanned bytes, title facts, lineage fingerprint) so unchanged files are not re-read; `FORMAT_VERSION` 2 |
| `discover.rs`, `parse.rs`, `tail.rs` | Claude project dir, JSONL line parser into `Message`/`Block`, Claude line follower |
| `sources/` | `HarnessId`, `SourceEntry`, `TranscriptHandle`; `codex.rs` (rollouts under `$CODEX_HOME/sessions`), `opencode.rs` (SQLite, read-only), `follow.rs` (`JsonlFollower`: offset, inode, partial line) |
| `harness.rs` | `Harness` trait, `Launch`/`LaunchKind`, argv for Claude/Codex/OpenCode |
| `external.rs` | Opens a session in an external macOS terminal app through a temp `.command` script |
| `favorites.rs` | Favourites file, `config_file()` and `write_atomically()` shared by other persisted files |
| `markdown.rs`, `export.rs` | Markdown to ratatui lines; `--export` writers |
| `tui/mod.rs` | `App`, `Focus`, key routing, draw. Must not grow: new behaviour goes in a submodule |
| `tui/runtime.rs` | Event loop, frame throttle, mouse-capture toggle |
| `tui/dismiss.rs` | `d`/`D` key handlers and `with_dismissed` |
| `tui/list.rs`, `tabs.rs`, `badges.rs` | List pane, tab bar (All, Favorites, Active, one per directory), activity badges |
| `tui/transcript.rs`, `follow.rs` | Transcript selection with 120 ms settle delay and a small cache (6 sessions, 6000 messages); follower threads |
| `tui/conversation.rs`, `verbosity.rs`, `tool_block.rs`, `diff.rs`, `timefmt.rs` | Rendering of messages at a verbosity level, tool blocks, line diffs, local timestamps |
| `tui/embedded.rs`, `sessions.rs`, `session_host.rs`, `prefix.rs`, `launch.rs` | Embedded panes, `SessionManager` (a client of the session host; in-process `PtyHost` only when no host client is installed, which is how unit tests run), `with_session_host` and the `1`-`9` strip keys, `^Space` prefix state machine, launch requests and the duplicate-launch confirm |
| `tui/identity.rs`, `palette.rs`, `colors.rs` | Per-session colour assignment (14 hues), persisted overrides |
| `tui/footer.rs`, `theme.rs` | Footer drawing (essential hints per mode, from `keys.rs`); all styles and colour constants |
| `tui/keys.rs` | The single key table (`KEYS`: zone, keys, label, note, essential). The footer draws the `essential` entries of the focused zone, the help overlay draws every entry grouped by zone. A new key binding gets one line here |
| `tui/overlay.rs`, `settings.rs`, `help.rs` | Modal overlay (`App.overlay`) drawn centered over the whole UI; it owns keys, paste and mouse while open, nothing reaches the list, transcript or an embedded child. `settings.rs` holds the settings rows and their actions, `help.rs` renders `KEYS` |

Data flow:
- `library::spawn` starts a source scan thread and the live thread (`live.rs`), and takes a wake callback that posts `Message::Updated` so the loop drains updates immediately. It sends `Update::{Scanned, Removed, Live, Activity, InitialScanDone}` over an mpsc channel; `App` drains it in `tick()` and rebuilds tabs and rows. Cached entries are sent before the first scan finishes.
- Selecting a session starts a follower thread (`tui/follow.rs`) per `TranscriptHandle`; it sends `Batch::{Appended, Replaced}` that `Transcript` drains.
- Embedded sessions live in the session host daemon (`terminal/daemon.rs`), not in ccview. `SessionManager::launch` sends `Spawn` (the client's whole environment, minus `TERM`/`COLORTERM`, which the host sets) and then opens one connection per session with `Attach`: the daemon replies with the screen as `state_formatted()` bytes plus a `ModeSnapshot` (kitty keyboard stacks, focus reporting, synchronized output, title) and then streams the raw output chunks that arrive afterwards, snapshot and subscription taken under the parser lock. The `RemoteHost` keeps its own vt100 parser and `Responder` (events and modes, replies go to a sink). Input and `Resize` flow back on the same connection. On startup `SessionManager::attach_existing` attaches every session the daemon lists.
- Each embedded session (`PtyHost` or `RemoteHost`) has a waker that sends `Message::Output(id)` to the event loop, so output redraws without polling. Input is read on its own thread (`Message::Input`). The loop draws at most every 16 ms and polls every 50 ms when idle.

Persisted files (written atomically):

| File | Location | Override |
|---|---|---|
| `favorites.json`, `favorite_dirs.json` (starred directory cwd paths), `hidden.json`, `dismissed.json` (session id to pid, object), `colors.json`, `view.json`, `ui.json` (`split_ratio`, `mouse`, `identity_colors`, `terminal_app`, `disable_agent_view`; every field optional with a default) | `<config>/ccview-rs/` | `XDG_CONFIG_HOME`, else `$HOME/.config` |
| `index-v1.json` (internal `format_version` 2; older versions load empty and rescan) | `<cache>/ccview-rs/` | `XDG_CACHE_HOME`, else `$HOME/.cache` |

Runtime files of the session host (all under one per-user directory, mode 0700: `$CCVIEW_RUNTIME_DIR`, else `$XDG_RUNTIME_DIR/ccview-rs`, else `<temp dir>/ccview-<uid>`):

| File | Purpose |
|---|---|
| `host.sock` | Unix socket, mode 0600; the daemon refuses peers with another uid |
| `host.lock` | `flock` held by the one running daemon (single instance per directory) |
| `host.pid` | daemon pid, removed on idle exit |
| `host.log` | daemon stdout and stderr, truncated at each daemon start |

Sources read (never write): Claude `$CLAUDE_CONFIG_DIR/projects` else `~/.claude/projects`, plus `~/.claude/sessions` and `~/.claude/daemon.lock`; Codex `$CODEX_HOME` else `~/.codex`; OpenCode `$XDG_DATA_HOME/opencode/opencode.db` else `~/.local/share/opencode/opencode.db`.

Other env vars read: `CCVIEW_CLAUDE_BIN`, `CCVIEW_CODEX_BIN`, `CCVIEW_OPENCODE_BIN` (launch binary, default `claude`/`codex`/`opencode`), `CCVIEW_TERMINAL_APP` (wins over the `terminal_app` setting, which the settings overlay then shows as locked; otherwise the setting, else `TERM_PROGRAM`, else `Terminal`), `CCVIEW_OPEN_BACKGROUND` (any non-empty value adds `-g` to `open`). `CCVIEW_TZ_CHILD` is an internal flag of a timezone test. `CCVIEW_RUNTIME_DIR` (directory of the session host socket, lock, pid and log; tests set it to a temp dir) and `CCVIEW_HOST_IDLE_MS` (shortens the host idle exit, tests only).

## Commands

Run `export PATH="/opt/homebrew/opt/rustup/bin:$PATH"` first if `cargo` is not found. Edition 2024, no `rust-version` pinned.

- Build: `cargo build`
- Test everything: `cargo test`. Last run: 250 unit tests plus 9 integration files (`session_host.rs` runs the daemon in a thread, `session_host_pty.rs` runs the real binary and daemon against a fake harness), all green, about 25 s.
- One file: `cargo test --test terminal_protocol`; unit tests: `cargo test --lib`.
- Install: `cargo install --path . --locked` installs the single binary `ccview-rs` into `~/.cargo/bin`. The commands `ccview-rs` and `ccv` on this machine are symlinks in `/opt/homebrew/bin` to that binary; `ccv` is not defined in this repo. Verified with `--root <temp dir>`; the real install was not run from here.
- Run the real UI: `cargo run`. Run against fakes: `HOME=<tmp> XDG_CONFIG_HOME=<tmp>/cfg XDG_CACHE_HOME=<tmp>/cache CCVIEW_RUNTIME_DIR=<tmp>/run CCVIEW_CLAUDE_BIN=/usr/bin/false cargo run`, with transcripts crafted under `<tmp>/.claude/projects/<dir>/<id>.jsonl` (see `tests/view_level_pty.rs` and `tests/activity_pty.rs` for the minimal shapes). Not run interactively here.
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
8. A test that can start a session host must set `CCVIEW_RUNTIME_DIR` to a temp directory (every binary-running test does, so none ever attaches to the owner's real sessions), must stop the daemon and its sessions when it ends, including on failure (`tests/common/mod.rs` `RuntimeGuard`), and must leave no process behind (check `ps`). Daemon unit tests run `daemon::run` in a thread with an explicit `RuntimeDir`. There is no kill command in the product; only `RuntimeGuard` signals processes.

## Keeping this file current

If a change alters an existing decision, pattern, key binding, persisted file, env var, module boundary or convention, update this file in the same change. Remove or reword statements that are no longer true, move items from PLANNED to built (or the reverse), and update the architecture table when a module is added, renamed or removed. A change that makes this file wrong is not finished.

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
- Keys, list and transcript: the list has two sub-focuses. In the rows `Right`/`l`/`Enter`/`Tab` focuses the right pane, `Up` from the first row moves to the tab bar, `x` hides the selected session (persisted in `hidden.json`, never touches transcripts), `H` shows hidden sessions dimmed, `S` stars the selected row's directory, `E` toggles `expand copies` (in memory only, not persisted). On the tab bar `Left/Right`, `h/l` or `[ ]` switch tabs, `s` stars or unstars the active directory tab and `Down`/`Enter`/`Esc` return to the rows; `Left`, `h`, `Esc` or `Tab` in the transcript returns to the list; `Ctrl+C` or `q` quits, with a confirm only while embedded sessions are running. `Shift+Left/Right` switch tabs from the rows, the tab bar and the transcript (no wrap change; inside an embedded session the child gets them). Actions: `r` resume, `f` fork, `n` new, `c` continue, `a` attach (Claude only), `o`/`O` external terminal, `s` or space star, `C` cycle colour, `v` cycle verbosity.
- Keys, embedded: everything is forwarded to the child, including `Ctrl+C`. The only interception is the `^Space` prefix: `^Space l`/`Esc` back to list, `x` or `d` detach (no confirm, the session keeps running), `[`/`]` or arrows previous/next session, `PgUp/PgDn` or `Up/Down` scroll the child, `C` colour, `^Space ^Space` sends a literal `^Space`. An unknown key after the prefix is dropped. A key press on an exited session dismisses it.
- Verbosity levels `chat`, `tools`, `everything`, cycled by `v`, persisted in `view.json`.
- Identity colours: 14 hues (`tui/palette.rs`), a stable default from a hash of the session id, no two tracked sessions share a hue while free ones remain, overrides persisted in `colors.json`. Colours are tracked for running embedded sessions and sessions with a live pid, and the list bar is drawn only on those active rows.
- Mouse capture is on by default (`mouse` in `ui.json`, default true) so the divider can be dragged and the wheel scrolls the pane under the pointer; with `"mouse": false` capture is on only while a running embedded session is focused and that session enabled a mouse protocol or the alternate screen. The wheel is forwarded to the child, or turned into arrow keys on the alternate screen (3 lines per notch). With capture on, the terminal's own text selection needs the Shift (or Option in iTerm) modifier.
- Claude launches (new, resume, fork, continue) set `CLAUDE_CODE_DISABLE_AGENT_VIEW=1` unless the `disable_agent_view` setting is off, and remove a named list of inherited session-identity variables (`CLAUDECODE`, `CLAUDE_CODE_CHILD_SESSION`, `_ENTRYPOINT`, `_EXECPATH`, `_MESSAGING_SOCKET`, `_MESSAGING_TOKEN`, `_SESSION_ATTENDED`, `_SESSION_ID`, `CLAUDE_PID`) so a nested session does not believe it is inside another; user-set variables such as `CLAUDE_CODE_USE_BEDROCK` are kept, and `CLAUDE_BG_*` variables are not stripped. `attach` is the exception: it keeps the environment.
- Refresh cadences: registry stat every 250 ms (`live.rs`), `claude agents --json` on change (min gap 500 ms) or every 5 s, sub-agents every 500 ms, source rescan every 1 s (`library.rs`), transcript follow poll 500 ms, transcript settle delay 120 ms, frames at most every 16 ms.
- Ordering: the All list is sorted by `last_used` descending (path as tiebreak); the Active tab lists every active session (the same test as the colour bar, `tabs::entry_is_active`: a live process or an activity entry, idle ones included, and not dismissed) ordered blocked, then busy or running agents, then idle, each group by `last_used`.
- Dismiss: `d` (list rows and transcript, not the tab bar) removes the selected active session from the active set, `D` undoes it. ccview only: it never signals or touches the process. A dismissed session looks inactive everywhere (colour bar, Active tab and its count, rank, status text, badges): `Library::derive_activity` drops it from the effective activity map and `tabs::entry_is_active` and the list row consult `Library::dismissed()`. The dismissal is stored with the pid (0 for a session with only an activity entry) and `Dismissed::reconcile` clears it on every library update, once both a live and an activity update have arrived, when the session is busy, blocked or has running sub-agents, when its current pid differs from the stored one (new run or process gone), or when nothing active is left; an idle session stays dismissed, across restarts while the pid lives. A session id with no entry in the library is left alone. `d` on a row bound to a running embedded pane shows `running in a pane` and does nothing. Start time is not part of the identity, only the pid. In a collapsed lineage it acts on the visible member and its lineage-effective live and activity.
- Launching a session already open in an embedded pane asks for confirmation (`Confirm::Duplicate`).
- Split: the list/detail ratio (default 0.35, list at least 24 columns, detail at least 30) is persisted in `ui.json`; resize with `<` `>` `=` (prefix first inside an embedded session) or drag the divider; mouse capture is on unless `mouse` is false in `ui.json`.
- In an embedded session `Left` returns to the list only when the cursor is visible and nothing but prompt glyphs, box drawing or whitespace is left of it (`tui/back_to_list.rs`); Ctrl+C is always forwarded; a child that exits 0 returns focus to the list automatically.
- The running-sessions strip, the transcript and session pane titles and the footer chip name the harness (`claude`, `codex`, `opencode`); list rows do not. In a row's detail line the status (`idle`, `busy`, badges) comes first so it is not clipped.
- Overlays: `,` opens settings and `?` opens help in the list, tab bar and transcript; inside an embedded session they are `ctrl+space ,` and `ctrl+space ?`. While one is open it owns every key (Esc closes, or leaves the hidden-sessions sub-list); a click outside closes it, the wheel scrolls it. Settings, all applied at once and persisted: identity colours on/off plus a colour picker for the selected session, view level, mouse capture, split ratio and reset, terminal app for `o`/`O` (auto, Terminal, iTerm, Ghostty, WezTerm), hidden sessions (Enter un-hides), disable Claude agent view.
- Footer: shows only each zone's essential hints plus `, settings`, `? help`, `q quit`; `help` and `quit` are pinned so they survive narrow widths. `C`, `v`, `< > =`, `f`, `c`, `a`, `o/O`, `x`, `H`, `g/G`, PgUp/PgDn work as before but are listed only in help.
- Starred directories (`s` on the tab bar, `S` on a row) sit first among the directory tabs with a `★ ` label prefix, each group most-recent-first; `s` on the tab bar acts on the directory, not the session.
- Lineage: resuming, continuing or backgrounding a Claude session writes a new JSONL that starts with the whole earlier history while the old file stays. Only records starting with `{"parentUuid"` carry a top-level `uuid`; each file keeps a bottom-128 sketch of 32-bit hashes of those uuids (exact for files with fewer than 128). Two files are compared over the sketch values at or below the smaller of their two cut-offs. If fewer than 8 values fall there, or none are shared, they are unrelated. A side is contained when at least 95% of its values are shared: one side contained means it is the ancestor of the other, both contained means identical copies (older is the ancestor), neither means a fork and both stay separate rows. An older file that kept growing with more than 5% of its uuids missing from the newer one is therefore a fork. Ancestors attach to their newest descendant leaf; the visible row of a group is its most recently used member, shown with `+N` after the time; `E` shows every member, older ones dimmed with `copy of <short id>`. A lineage counts as starred if any member is starred and `s` toggles all members; hide (`x`) and `Enter`/`r`/`f`/`n`/`c`/`o` act on the visible member only. Activity and live state of any member show on the visible row. Codex and OpenCode rows are never grouped. Limit: an ancestor with fewer than about 8 sketch values inside the descendant's cut-off (tiny ancestor, huge descendant) is not detected directly, only through an intermediate copy.
- Sessions outlive ccview. Every embedded session is a child of the session host daemon (`ccview-rs host`), started detached (setsid, log in the runtime dir) the first time a session is launched and not running. ccview has no kill action and never asks about killing: quitting, a crash or a panic leaves sessions running (the panic hook only restores the terminal); a session ends only when the harness itself exits. There is deliberately no kill message in the protocol or the debug CLI.
- Detach (`^Space x`/`d`) removes the pane from the strip for this run and says so in a notice; the session keeps running and comes back on `r` of its list row (matched by session id or pid) or at the next start. Dismissing an exited session (any key) is the only thing that removes a session from the daemon, and the daemon refuses to dismiss a running one.
- On startup ccview attaches every session the daemon lists (including exited ones, which show their final screen with the exited state) instead of attaching lazily on selection: attaching is what feeds the strip state, bells and titles, and `resize_all` resizes every session anyway.
- The daemon exits by itself after `IDLE_EXIT` (60 s) with no live session and no connected client; it never kills anything when it exits. The same exit rule is the only lifecycle rule.
- The PTY has one size: the last attach or resize wins for every client. Reattach restores the screen as vt100 `state_formatted()` (visible grid, cursor, colours, input modes) plus the kitty keyboard stacks, focus reporting, synchronized output and title from the `ModeSnapshot`.
- PLANNED (the owner's open requests, not in the code): restore of interrupted sessions after a reboot (the host keeps no durable list yet), the Rust conventions restructure, a dismiss-from-active key.

## Known limitations

Verified in code:
- Reattach restores the visible screen, not the scrollback (5000 lines stay in the daemon and in clients that were attached while it was produced). When the child is on the alternate screen the main screen behind it is not restored (vt100 exposes only the current grid), so leaving the alternate screen after a reattach shows an empty main screen until the child redraws.
- All clients share one PTY size; a second ccview attached at another size keeps its own parser size until it resizes, so its drawing can differ from the child's real grid.
- A session whose host dies (daemon killed or crashed) is shown as exited with `host lost`; the children die with it because the PTY master closes. There is no protocol version: replacing the binary while a daemon runs leaves the old daemon serving the new client.
- A detached session without a list row (no registry match, for example a new Codex session) is reachable again only at the next start.
- Daemon-side unix sockets limit the runtime dir path to the platform `sun_path` size (104 bytes on macOS).
- OSC 8 hyperlinks are not rendered in embedded panes; `responder.rs` ignores unknown OSC sequences (only OSC 9;4 progress is recognised and dropped).
- Synchronized output (mode 2026) is tracked and reported to the child but output is not buffered by it; `PtyHost::synchronized_output()` is only exercised by `tests/terminal_protocol.rs`, not by drawing.
- External terminal launch is macOS only (`external.rs`).
- Unix only (`libc`, `os::unix`).
- The Claude daemon check and `claude agents --json` depend on a Claude version that has them; absence yields no agent badges rather than an error.

Not verified:
- The peer-uid rejection path (it needs a second user), and the Linux `SO_PEERCRED` branch (only the macOS `getpeereid` branch was compiled and run).
- Real Claude behaviour across a reattach (fake harnesses only).
- Claude embedded behaviour past the trust dialog: no test goes beyond a fake binary.
- `tests/terminal_protocol.rs` flaking under load: one run under CPU load did not complete in the time available, so it is neither confirmed nor ruled out; it passed in the normal runs.
- Real Codex and OpenCode launch behaviour beyond argv: tested with fake binaries only.
