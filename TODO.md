# TODO

Current state: Eve, the widget, works and is used daily. Claude Code sessions
are tracked and grouped by worktree, editors are discovered, and clicking a
worktree throws its VS Code window onto the monitor the widget is on.
Next: Codex sessions, then durable state beyond the worktree order.

Checked items mean completed work. The order below is a working plan, not a
commitment to build every feature before using the app. Add dependencies only
when needed, and use disposable repositories for initial integration tests.

## 0. Repository foundation
- [x] Save project motivation, scope, and open questions in README.md.
- [x] Create the source skeleton and root architecture map.
- [x] Keep AGENTS.md and CLAUDE.md short and identical.
- [x] Add Git ignores, line-ending rules, and editor defaults.
- [x] Add TECHSTACK.md for proposed technologies and tradeoffs.
- [x] Remove contributor/PR templates and duplicate documentation.
- [x] Choose an open-source license before releasing for reuse.

## 1. Bootable desktop shell
- [x] Discuss TECHSTACK.md and select the initial stack.
- [x] Verify prerequisites and record toolchain versions.
- [x] Generate the smallest conventional application scaffold.
- [x] Add dependency lockfiles and actual setup, run, and build commands.
- [x] Configure relevant formatting, linting, type checks, and test commands.
- [x] Launch an empty shell on Windows.
- [ ] Validate startup on macOS when a Mac is available.
- [x] Replace .gitkeep files as real files enter each folder.

## 2. First useful workspace view
- [ ] Register an existing local Git repository.
- [x] Discover its worktrees with checkout paths and branch information.
- [x] Show an overview and select a worktree.
- [x] Open the selected checkout in VS Code.
- [ ] Keep worktree environment references separate.
- [ ] Handle invalid repositories, missing paths, detached worktrees, and paths with spaces.
- [x] Show launch failures clearly.
- [ ] Validate with a disposable repository, then try the owner's daily workflow.

## 3. Agent interaction loop
- [ ] Decide whether the first version needs existing conversations or may start new sessions.
- [x] Validate one supported standalone Claude Code interface.
- [ ] Validate one supported standalone Codex interface.
- [ ] Find a Codex equivalent of Claude Code's hooks, or fall back to reading
      its session files the way Claude Code transcripts are read.
- [x] Associate each session with the correct worktree and environment.
- [ ] Send a specifically targeted prompt to each and identify both responses.
- [ ] Distinguish submission, confirmed delivery, completion, failure, and unknown outcomes.
- [x] Show working, waiting for user, completed, disconnected, and unknown states from evidence.
- [x] Surface questions, blockers, and finished work in an attention feed.
- [ ] Validate disconnect/reconnect behavior without silently duplicating prompts.

## 4. Everyday desktop access
- [ ] Validate discovery and activation of existing Chrome tabs.
- [ ] Handle multiple browser windows/profiles and closed tabs.
- [ ] Provide global tab access and optional worktree associations.
- [ ] Validate a basic Discord entry point.
- [ ] Validate a basic Spotify entry point.
- [ ] Explore supported Spotify playback controls.
- [ ] Evaluate richer in-app access separately from external app launching.
- [ ] Keep browser, chat, and music usable independently of a repository.

## 5. Durable local state
- [ ] Choose storage for workspace references, settings, and activity.
- [ ] Choose credential handling if integrations require authentication.
- [ ] Restore registered workspaces without assuming old sessions are connected.
- [ ] Handle moved or deleted worktrees and unavailable applications.
- [ ] Define activity retention and deletion behavior.

## 6. Voice and handoffs
- [ ] Compare supported voice integration paths and any usage costs.
- [ ] Route voice through the same explicit actions used by the UI.
- [ ] Always show the targeted worktree/session and action outcome.
- [ ] Navigate workspaces and request session status by voice.
- [ ] Relay a finding between sessions with traceable delivery.
- [ ] Test interruptions, ambiguous targets, and uncertain delivery.

## 6.5 Gaps found in daily use
- [x] Keyboard navigation: the widget is hover-only, so there is no way to
      reach a worktree without the mouse.
- [x] Clicking a session row does nothing.
- [ ] `list_worktrees` works against a real repository but nothing calls it;
      either surface branch names per worktree or drop the command.
- [ ] Ended-session tombstones live in memory, so restarting Switchboard
      lets a finished session be rediscovered from disk once more.
- [ ] A prompt that is machine-generated renders as raw XML in the detail
      line. Worth detecting and summarising.
- [ ] Codex sessions show presence and activity but never "blocked on you".
      Its hooks would fix that, at the cost of a trust prompt on every
      Switchboard update that edits them.
- [ ] Codex liveness could be exact rather than inferred: a non-blocking
      lock attempt on `~/.codex/thread-writer-locks/<id>.lock` distinguishes
      a live session from a leaked lock.
- [ ] Window placement is Windows-only. macOS needs an Accessibility-API
      equivalent of the Win32 move-and-maximise.
- [ ] The transcript scan re-reads every project directory every five
      seconds. Fine at this size, wasteful later.
- [ ] Nothing survives a restart except the worktree order, so session
      history is lost each time.

## 7. Daily use and releases
- [ ] Refine layout, keyboard navigation, and accessibility from real use.
- [ ] Check responsiveness and resource usage with several active workspaces.
- [ ] Validate the supported integration behavior on both Windows and macOS.
- [ ] Add CI once there are useful automated checks to run.
- [ ] Add packaging and signing when the app is ready to distribute.
- [ ] Expand integrations based on actual desktop use.
