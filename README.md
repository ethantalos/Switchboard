# Switchboard

A personal open-source side project: one place to access my coding workspaces,
Chrome tabs, Discord, Spotify, and eventually more of my desktop.

I currently juggle four VS Code windows, with separate virtual environments
outside the main checkout. Three monitors help, but I want this to work on my
Mac too. I'm building it for fun, learning, and my own daily workflow alongside
my main coding work. Similar tools exist; I still want to make my own.

## What I want
- Organize coding work as repository → worktrees → agent sessions.
- See workspaces together and open the right VS Code checkout.
- Access Claude Code and Codex sessions, send targeted prompts, and see who needs attention.
- Find and return to existing Chrome tabs.
- Access Discord and Spotify independently of coding projects.
- Eventually coordinate everything through one live voice conversation.

An overview, worktree tabs, a selected session, and an attention feed are initial
UI ideas, not an approved design. Voice could switch workspaces, request status,
or relay a finding between agents. Windows and macOS are the target platforms.

## Decisions and open questions
- Keep the repository lean; add structure when implementation needs it.
- Stack chosen and installed: Tauri 2 + React/TypeScript + Rust. SQLite is
  deferred until persistence is actually needed.
- The first agent integration question is unanswered: control existing VS Code
  conversations, or start new sessions inside Switchboard?
- A hybrid of managed sessions and external app shortcuts is a possibility.
  Embedding every external window is not required.
- Chrome, Discord, Spotify, and voice interfaces still need validation. Tools
  available inside an assistant are not automatically available to a public app.
- Voice may require a separately billed API; no provider has been selected.
- Existing tools considered: Emdash, Superset, Rambox, and Raycast. No need to
  revisit building versus buying unless requested.

## Integration principles
Source projects stay in their existing folders, with separate worktree environments.
Settings and activity stay in local application data; private projects, credentials,
and conversations stay out of this public repo.

Always show an action's target and a response's source. Submission, confirmed
delivery, and completion are different. Silence is not completion; window focus
is not proof of prompt delivery. Session states may be working, waiting for user,
completed, disconnected, or unknown.

## Where things stand
Eve, the widget, works and is in daily use. It is a 320x320 always-on-top
square that sits as a 60px badge until you hover it, then opens into the
worktrees it knows about and the Claude Code sessions in each.

Working today:

- Sessions are tracked from Claude Code hooks, and discovered from session
  transcripts on disk so ones that started before Switchboard - or survived
  its restart - still appear.
- Each session says what it wants: blocked on you, your turn, working, idle,
  or quiet. The badge shows the most urgent of those and how many.
- Editors with Claude Code attached are found by reading `~/.claude/ide`,
  with dead locks pruned by checking the process is alive.
- Clicking a worktree fills the monitor the widget is on with its VS Code
  window; clicking again puts it back exactly where it was.
- Worktrees can be dragged into a fixed order, which is saved.
- The settings window says whether Claude Code is actually wired up, and
  shows the raw hook traffic.

Not built yet: Codex sessions, Chrome tabs, Discord, Spotify, voice, and any
persistence beyond the worktree order. macOS is untested - the window
placement is Windows-only.

Run `npm install` once, then `npm run tauri dev`.
Claude and Codex use identical project instructions. There is no agent-specific
setup, mandatory document-reading checklist, or contributor workflow.

[ARCHITECTURE.md](ARCHITECTURE.md) maps what exists.
[docs/STACK.md](docs/STACK.md) lists the major technologies.
[TODO.md](TODO.md) holds the next steps and open implementation work.
Licensed under the [MIT License](LICENSE).
