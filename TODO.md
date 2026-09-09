# TODO

Current state: repository skeleton only. Nothing runs yet.
Next: choose the desktop stack, then launch the smallest application shell.

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
- [ ] Choose an open-source license before releasing for reuse.

## 1. Bootable desktop shell
- [ ] Discuss TECHSTACK.md and select the initial stack.
- [ ] Verify prerequisites and record toolchain versions.
- [ ] Generate the smallest conventional application scaffold.
- [ ] Add dependency lockfiles and actual setup, run, and build commands.
- [ ] Configure relevant formatting, linting, type checks, and test commands.
- [ ] Launch an empty shell on Windows.
- [ ] Validate startup on macOS when a Mac is available.
- [ ] Replace .gitkeep files as real files enter each folder.

## 2. First useful workspace view
- [ ] Register an existing local Git repository.
- [ ] Discover its worktrees with checkout paths and branch information.
- [ ] Show an overview and select a worktree.
- [ ] Open the selected checkout in VS Code.
- [ ] Keep worktree environment references separate.
- [ ] Handle invalid repositories, missing paths, detached worktrees, and paths with spaces.
- [ ] Show launch failures clearly.
- [ ] Validate with a disposable repository, then try the owner's daily workflow.

## 3. Agent interaction loop
- [ ] Decide whether the first version needs existing conversations or may start new sessions.
- [ ] Validate one supported standalone Claude Code interface.
- [ ] Validate one supported standalone Codex interface.
- [ ] Associate each session with the correct worktree and environment.
- [ ] Send a specifically targeted prompt to each and identify both responses.
- [ ] Distinguish submission, confirmed delivery, completion, failure, and unknown outcomes.
- [ ] Show working, waiting for user, completed, disconnected, and unknown states from evidence.
- [ ] Surface questions, blockers, and finished work in an attention feed.
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

## 7. Daily use and releases
- [ ] Refine layout, keyboard navigation, and accessibility from real use.
- [ ] Check responsiveness and resource usage with several active workspaces.
- [ ] Validate the supported integration behavior on both Windows and macOS.
- [ ] Add CI once there are useful automated checks to run.
- [ ] Add packaging and signing when the app is ready to distribute.
- [ ] Expand integrations based on actual desktop use.
