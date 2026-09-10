# Handoff — Eve widget work, 2026-09-10

Stopping point for the `feat/eve-widget` branch. Disposable: delete it once
the branch lands. Everything durable is already in ARCHITECTURE.md, README.md
and TODO.md.

**PR:** https://github.com/ethantalos/Switchboard/pull/4 (open, 12 commits)

## Do this first

Open Eve → gear → **Connect Claude Code**, then start a *new* Claude Code
session.

`PostToolUse` and `SubagentStop` are not registered in
`~/.claude/settings.json`. Hooks are read when a session starts, so already
running sessions will not pick them up. Until then, Eve falls back to reading
transcripts and a session mid-turn can look quieter than it is. The widget
shows a banner saying so.

## What changed

The reported symptom was "I have an agent running in Themis B and you haven't
noticed it". Three causes, all the same mistake - treating hooks as the only
source of truth:

1. The session store was in memory, so restarting forgot every session and
   each only reappeared when it next fired a hook.
2. A long turn fires nothing at all between `UserPromptSubmit` and `Stop`.
3. `PostToolUse` was never registered, so tool activity was silent too.

Sessions are now also read from what the assistants write to disk. On a cold
start with an empty hook store, Eve found 9 sessions with no hooks at all.

Separately, `Stop` was mapped to "done" and painted green. `Stop` means the
assistant finished and is waiting for *you*, which is the most useful thing
this widget can say, and it was being reported as nothing to see here. It is
"your turn" now, in its own colour.

Full list in the PR description and the commit messages, which are written to
be read.

## Things worth not rediscovering

Each of these cost real time to establish.

- **There is no `message` field on any Claude Code hook event.** Detail text
  never rendered because of it. Captured from live payloads: `Stop` carries
  `last_assistant_message`, `UserPromptSubmit` carries `prompt`, tool events
  carry `tool_name`, `Notification` carries `notification_type`. Every event
  carries `transcript_path`, `cwd`, `session_id`, `permission_mode`.
- **A `Notification` is not always blocking.** `idle_prompt` fires about a
  minute after a turn ends. Treating it as blocking turns the badge red for
  sessions with nothing to do.
- **`SessionEnd` is not reliable** - a closed window fires nothing - but when
  it does fire it must be remembered *past* the session record, or the disk
  scan resurrects the session as a permanent ghost. Found by running a real
  `claude -p` end to end, not by reading code.
- **Hook `cwd` is the process's current directory**, not the session root. A
  session that `cd`s into a subfolder appears as a second worktree.
- **`~/.claude/session-env/` is not a liveness signal** - 191 stale entries.
  Neither is transcript existence. Only recent transcript writes are.
- **Codex 0.153 does have hooks**, nearly Claude Code's contract, but they
  need trusting via `/hooks` and the trust is keyed to a hash of the hook
  definition, so every update re-arms the prompt. Reading its rollouts needs
  no setup. Codex has no `Notification` event at all.
- **Codex liveness could be exact**: try-lock
  `~/.codex/thread-writer-locks/<id>.lock`. File existence is not enough,
  locks leak. Not implemented.
- **Windows: `SetWindowPlacement` will not move an already-maximised window
  to another monitor**, and setting a rect directly lands 1.5x too large on a
  150% display. Maximising is what keeps sizing correct. Restoring across a
  DPI boundary needs the size restated twice.
- **`rcNormalPosition` is workspace coordinates**, offset by the *primary*
  monitor's work area. It happens to equal screen coordinates on this setup
  because that origin is (0,0). Nothing reads it; do not start.
- **Fluent motion**: enter decelerates over 250ms
  `cubic-bezier(0.1,0.9,0.2,1)`, exit accelerates over 200ms
  `cubic-bezier(0.9,0.1,1,0.2)`. Never the same curve or length both ways.

## Environment gotchas

- Writing a whole file at once can race Vite's watcher and cache an **empty**
  transform - the window goes black with "does not provide an export named
  default". `touch` the file to invalidate.
- Escape sequences get collapsed one level when passed through the shell.
  Anything containing a backslash or `\n` should be built with an explicit
  character rather than typed as an escape, then verified with `repr()`.
- `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=9222` gives
  the webview a CDP endpoint, which is the only sane way to inspect or drive
  the UI headlessly. Not set in a normal run.

## State

- Working tree clean, everything pushed.
- `tsc` clean, oxlint 0 warnings, 27 Rust tests, release build clean. Two
  clippy warnings remain and predate this work (`lib.rs:275`, `lib.rs:441`
  equivalents in `main.rs`).
- `aether.html` sits untracked at the repo root. It is not Switchboard's and
  was deliberately left out of every commit.
- Test artifacts removed: scratch worktrees, the Claude project directories
  they created, and five throwaway Codex rollouts. Real history untouched.

## Next

TODO.md section 6.5 has the live list. The three worth doing first:

1. Codex "blocked on you", via its hooks or the writer-lock probe.
2. Persist the ended-session tombstones, so one restart cannot resurrect a
   finished session.
3. Decide `list_worktrees`' fate - it works against a real repository and is
   tested, but nothing calls it.
