import { useEffect, useState } from "react";
import type React from "react";
import type { CSSProperties } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import "./App.css";

type SessionState =
  | "needs_you"
  | "failed"
  | "waiting"
  | "working"
  | "idle"
  | "unknown";

type Session = {
  sessionId: string;
  cwd: string;
  state: SessionState;
  detail: string | null;
  updatedAt: number;
};

/// Whether Claude Code is actually configured to report to this widget.
type HookHealth = {
  listening: boolean;
  missing: string[];
  misdirected: string[];
  registered: number;
};

/// An editor with Claude Code attached, found by reading `~/.claude/ide`
/// rather than by waiting for a hook.
type Workspace = {
  path: string;
  ideName: string;
  pid: number;
  port: string;
};

const LABEL: Record<SessionState, string> = {
  needs_you: "needs you",
  failed: "failed",
  waiting: "your turn",
  working: "working",
  idle: "idle",
  unknown: "unknown",
};

/// Sorted by how much of your attention it wants. Blocked first, then things
/// that finished and are waiting on you; anything still working is last,
/// because there is nothing for you to do about it.
const RANK: Record<SessionState, number> = {
  needs_you: 0,
  failed: 1,
  waiting: 2,
  working: 3,
  idle: 4,
  unknown: 5,
};

/// A patch bay: one hub with cords curving out to jacks. Drawn with
/// currentColor so the mark and the border carry the status colour together.
function Mark() {
  return (
    <svg className="mark" viewBox="0 0 32 32" aria-hidden="true">
      <g stroke="currentColor" strokeWidth="2" strokeLinecap="round" fill="none" opacity="0.55">
        <path d="M16 16 C 12.5 14, 11 12.5, 9 10.5" />
        <path d="M16 16 C 19.5 14, 21 12.5, 23 10.5" />
        <path d="M16 16 C 16 19.5, 16 21, 16 23" />
      </g>
      <g fill="currentColor">
        <circle cx="9" cy="10.5" r="2.3" opacity="0.8" />
        <circle cx="23" cy="10.5" r="2.3" opacity="0.8" />
        <circle cx="16" cy="23" r="2.3" opacity="0.8" />
        <circle cx="16" cy="16" r="3.5" />
      </g>
    </svg>
  );
}

/// A real cog: eight teeth around a hub, with the centre punched out.
function Cog() {
  const teeth = [0, 45, 90, 135, 180, 225, 270, 315];
  return (
    <svg viewBox="0 0 24 24" width="15" height="15" aria-hidden="true">
      <g fill="currentColor">
        {teeth.map((angle) => (
          <rect
            key={angle}
            x="10.7"
            y="1.5"
            width="2.6"
            height="4.6"
            rx="0.9"
            transform={`rotate(${angle} 12 12)`}
          />
        ))}
        <circle cx="12" cy="12" r="6.6" />
      </g>
      {/* Sits on the widget surface, so the hub hole matches that colour. */}
      <circle cx="12" cy="12" r="2.6" fill="var(--surface)" />
    </svg>
  );
}

function basename(path: string): string {
  const trimmed = path.replace(/[\\/]+$/, "");
  const parts = trimmed.split(/[\\/]/);
  return parts[parts.length - 1] || path;
}

/// Enough of the session id to tell two sessions in one worktree apart.
function shortId(sessionId: string): string {
  return sessionId.replace(/-/g, "").slice(-4);
}

type Group = {
  cwd: string;
  sessions: Session[];
  state: SessionState;
  updatedAt: number;
  ide: string | null;
  quiet: boolean;
};

/// The same folder arrives spelled differently depending on the source -
/// hooks report `C:\Switchboard`, IDE locks report `c:\Switchboard` - so
/// group on a normalised key and keep the first spelling seen for display.
function pathKey(path: string): string {
  return path.replace(/[\\/]+$/, "").replace(/\\/g, "/").toLowerCase();
}

/// A session nothing has been heard from in a while. Still shown, because it
/// may simply be a session you left open, but dimmed and kept out of the
/// badge count so it cannot make the widget cry wolf.
function isQuiet(session: Session, now: number, quietAfterMs: number): boolean {
  return quietAfterMs > 0 && now - session.updatedAt > quietAfterMs;
}

/// Has Switchboard filled a monitor with this worktree's editor? If so a
/// second click sends it back where it was.
function isParked(cwd: string, parked: string[]): boolean {
  const key = pathKey(cwd);
  return parked.some((p) => pathKey(p) === key);
}

/// One row per worktree, with its sessions underneath. A worktree takes the
/// state of its most urgent session, so the group header is the thing to scan.
///
/// Worktrees come from two places. Hooks report sessions as they act, but only
/// from the moment Switchboard starts. Editors already running are discovered
/// separately, so they appear with no sessions and an `unknown` state until a
/// hook says otherwise - silence is not completion.
function groupByWorktree(
  sessions: Session[],
  workspaces: Workspace[],
  now: number,
  quietAfterMs: number,
  order: string[],
): Group[] {
  // A quiet session must never outrank a live one, however urgent it looked
  // when it went silent.
  const weight = (s: Session) =>
    RANK[s.state] + (isQuiet(s, now, quietAfterMs) ? 100 : 0);
  const byKey = new Map<string, Group>();

  for (const session of sessions) {
    const key = pathKey(session.cwd);
    const group = byKey.get(key);
    if (group) group.sessions.push(session);
    else
      byKey.set(key, {
        cwd: session.cwd,
        sessions: [session],
        state: "unknown",
        updatedAt: 0,
        ide: null,
        quiet: false,
      });
  }

  for (const workspace of workspaces) {
    const key = pathKey(workspace.path);
    const group = byKey.get(key);
    if (group) group.ide = workspace.ideName;
    else
      byKey.set(key, {
        cwd: workspace.path,
        sessions: [],
        state: "unknown",
        updatedAt: 0,
        ide: workspace.ideName,
        quiet: false,
      });
  }

  const groups = [...byKey.values()];
  for (const group of groups) {
    group.sessions.sort(
      (a, b) => weight(a) - weight(b) || b.updatedAt - a.updatedAt,
    );
    const lead = group.sessions[0];
    group.state = lead?.state ?? "unknown";
    group.quiet = !lead || isQuiet(lead, now, quietAfterMs);
    group.updatedAt = group.sessions.reduce(
      (latest, s) => Math.max(latest, s.updatedAt),
      0,
    );
  }

  // A worktree the user placed by hand stays where they put it. Everything
  // else falls back to urgency, so a new one still surfaces on its own.
  const placed = (g: Group) => {
    const at = order.indexOf(pathKey(g.cwd));
    return at === -1 ? Number.MAX_SAFE_INTEGER : at;
  };

  return groups.sort(
    (a, b) =>
      placed(a) - placed(b) ||
      Number(a.quiet) - Number(b.quiet) ||
      RANK[a.state] - RANK[b.state] ||
      b.updatedAt - a.updatedAt,
  );
}

function ago(timestamp: number, now: number): string {
  const seconds = Math.max(0, Math.round((now - timestamp) / 1000));
  if (seconds < 60) return `${seconds}s`;
  const minutes = Math.round(seconds / 60);
  if (minutes < 60) return `${minutes}m`;
  return `${Math.round(minutes / 60)}h`;
}

export default function App() {
  const [sessions, setSessions] = useState<Session[]>([]);
  const [workspaces, setWorkspaces] = useState<Workspace[]>([]);
  // Worktrees currently filling a monitor. Keyed the same way Rust keys them.
  const [parked, setParked] = useState<string[]>([]);
  // Rust owns the threshold; the frontend re-derives quietness every tick.
  const [quietAfterMs, setQuietAfterMs] = useState(0);
  // The order the user dragged worktrees into, as normalised path keys.
  const [order, setOrder] = useState<string[]>([]);
  const [dragging, setDragging] = useState<string | null>(null);
  const [dropTarget, setDropTarget] = useState<string | null>(null);
  const [pinned, setPinned] = useState(false);
  const [health, setHealth] = useState<HookHealth | null>(null);
  // Session whose full message is open. Rows are one line by default.
  const [opened, setOpened] = useState<string | null>(null);
  const [expanded, setExpanded] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [now, setNow] = useState(() => Date.now());

  useEffect(() => {
    void invoke<Session[]>("list_sessions").then(setSessions).catch(() => {});
    void invoke<Workspace[]>("list_workspaces").then(setWorkspaces).catch(() => {});
    void invoke<string[]>("list_parked").then(setParked).catch(() => {});
    void invoke<number>("quiet_after_ms").then(setQuietAfterMs).catch(() => {});
    void invoke<string[]>("list_order").then(setOrder).catch(() => {});
    void invoke<boolean>("is_pinned").then(setPinned).catch(() => {});
    void invoke<HookHealth>("hook_health").then(setHealth).catch(() => {});

    // Rust owns hover: the window never resizes, so it watches the pointer and
    // tells us when to grow. Nothing here changes window geometry.
    const offHover = listen<boolean>("hover-changed", (event) => {
      setExpanded(event.payload);
    });
    const offSessions = listen<Session[]>("sessions-changed", (event) =>
      setSessions(event.payload),
    );
    const offWorkspaces = listen<Workspace[]>("workspaces-changed", (event) =>
      setWorkspaces(event.payload),
    );
    const offParked = listen<string[]>("parked-changed", (event) =>
      setParked(event.payload),
    );
    const offOrder = listen<string[]>("order-changed", (event) =>
      setOrder(event.payload),
    );
    const offPinned = listen<boolean>("pinned-changed", (event) =>
      setPinned(event.payload),
    );
    // Reconnecting fixes hooks from the settings window, so re-check when
    // traffic starts arriving rather than only at startup.
    const offHooks = listen("hooks-changed", () => {
      void invoke<HookHealth>("hook_health").then(setHealth).catch(() => {});
    });

    const ticker = setInterval(() => setNow(Date.now()), 10_000);

    return () => {
      void offHover.then((off) => off());
      void offSessions.then((off) => off());
      void offWorkspaces.then((off) => off());
      void offParked.then((off) => off());
      void offOrder.then((off) => off());
      void offPinned.then((off) => off());
      void offHooks.then((off) => off());
      clearInterval(ticker);
    };
  }, []);

  async function open(path: string) {
    setError(null);
    try {
      await invoke("open_in_vscode", { path });
    } catch (err) {
      setError(String(err));
    }
  }

  const groups = groupByWorktree(sessions, workspaces, now, quietAfterMs, order);

  /// Arrow keys walk the worktree list, Enter opens one, Escape lets go.
  ///
  /// The panel is otherwise reachable only by hovering it, so without this
  /// there is no way to use the widget from the keyboard at all.
  function onKeys(event: React.KeyboardEvent<HTMLDivElement>) {
    if (event.key === "Escape") {
      event.preventDefault();
      void invoke("set_pinned", { pinned: false }).catch(() => {});
      return;
    }

    const step = event.key === "ArrowDown" ? 1 : event.key === "ArrowUp" ? -1 : 0;
    if (step === 0) return;
    event.preventDefault();

    const heads = [
      ...document.querySelectorAll<HTMLButtonElement>(".worktree-head"),
    ];
    if (heads.length === 0) return;
    const at = heads.indexOf(document.activeElement as HTMLButtonElement);
    // Nothing focused yet starts at the top, which is the most urgent row.
    const next = at === -1 ? 0 : (at + step + heads.length) % heads.length;
    heads[next].focus();
  }

  /// Move the dragged worktree in front of the one it was dropped on, and
  /// persist the whole visible order so later sessions keep the arrangement.
  function reorder(targetKey: string) {
    setDropTarget(null);
    const from = dragging;
    setDragging(null);
    if (!from || from === targetKey) return;

    const keys = groups.map((g) => pathKey(g.cwd));
    const at = keys.indexOf(from);
    const to = keys.indexOf(targetKey);
    if (at === -1 || to === -1) return;

    const next = [...keys];
    next.splice(to, 0, ...next.splice(at, 1));
    setOrder(next);
    void invoke("set_order", { order: next }).catch((err) =>
      setError(String(err)),
    );
  }

  // The badge answers one question - what is the most urgent thing, and how
  // many of them - so it reports the worst live state rather than a total.
  // Quiet sessions are excluded: a count you cannot act on is noise.
  const live = sessions.filter((s) => !isQuiet(s, now, quietAfterMs));
  const tally = (state: SessionState) =>
    live.filter((s) => s.state === state).length;

  const pulse: SessionState =
    tally("needs_you") > 0
      ? "needs_you"
      : tally("failed") > 0
        ? "failed"
        : tally("waiting") > 0
          ? "waiting"
          : tally("working") > 0
            ? "working"
            : live.length > 0
              ? "idle"
              : "unknown";
  const count = pulse === "unknown" ? 0 : tally(pulse);

  return (
    <div
      className={`stage ${pulse}${expanded ? " is-expanded" : ""}`}
      data-tauri-drag-region
    >
      <div className="badge" data-tauri-drag-region aria-hidden={expanded}>
        <Mark />
        {count > 0 && <span className="count">{count}</span>}
      </div>

      <div
        className="surface"
        aria-hidden={!expanded}
        data-tauri-drag-region
        tabIndex={-1}
        onKeyDown={onKeys}
      >
        <div className="panel" data-tauri-drag-region>
          {error && (
            <p className="error" role="alert">
              {error}
            </p>
          )}

          {health && !health.listening && (
            <button
              className="nudge bad"
              tabIndex={expanded ? 0 : -1}
              onClick={() => void invoke("open_settings").catch(() => {})}
            >
              Not listening - another Switchboard has the port. Nothing can
              reach this one.
            </button>
          )}

          {health &&
            health.listening &&
            health.missing.length + health.misdirected.length > 0 && (
              <button
                className="nudge"
                tabIndex={expanded ? 0 : -1}
                onClick={() => void invoke("open_settings").catch(() => {})}
                title={`Not reporting: ${[...health.missing, ...health.misdirected].join(", ")}`}
              >
                {health.missing.length + health.misdirected.length} hook
                {health.missing.length + health.misdirected.length === 1
                  ? ""
                  : "s"}{" "}
                missing - sessions may look idle. Fix
              </button>
            )}

          {groups.length === 0 ? (
            <p className="empty">
              No sessions or editors found. Connect Claude Code in settings,
              then start a session.
            </p>
          ) : (
            <ul className="worktrees">
              {groups.map((group, index) => (
                <li
                  key={group.cwd}
                  className={`worktree${
                    dragging === pathKey(group.cwd) ? " dragging" : ""
                  }${dropTarget === pathKey(group.cwd) ? " drop-target" : ""}`}
                  /* Capped so a long list does not trail on forever. */
                  style={{ "--i": Math.min(index, 5) } as CSSProperties}
                  draggable
                  onDragStart={(event) => {
                    setDragging(pathKey(group.cwd));
                    event.dataTransfer.effectAllowed = "move";
                    // Firefox refuses to start a drag without payload.
                    event.dataTransfer.setData("text/plain", group.cwd);
                  }}
                  onDragEnd={() => {
                    setDragging(null);
                    setDropTarget(null);
                  }}
                  onDragOver={(event) => {
                    if (!dragging) return;
                    event.preventDefault();
                    event.dataTransfer.dropEffect = "move";
                    setDropTarget(pathKey(group.cwd));
                  }}
                  onDragLeave={() => {
                    setDropTarget((current) =>
                      current === pathKey(group.cwd) ? null : current,
                    );
                  }}
                  onDrop={(event) => {
                    event.preventDefault();
                    reorder(pathKey(group.cwd));
                  }}
                >
                  <button
                    className={`worktree-head ${group.state}${group.quiet ? " quiet" : ""}`}
                    onClick={() => open(group.cwd)}
                    title={
                      isParked(group.cwd, parked)
                        ? `${group.cwd}\nFilling this monitor. Click again to send it back.`
                        : `${group.cwd}\nClick to fill this monitor.`
                    }
                  >
                    <span className="dot" aria-hidden="true" />
                    <span className="worktree-name">
                      {basename(group.cwd) || "unknown"}
                    </span>
                    {isParked(group.cwd, parked) && (
                      <span className="parked" aria-label="filling this monitor">
                        &#8617;
                      </span>
                    )}
                    <span className="worktree-count">
                      {group.sessions.length > 0
                        ? group.sessions.length
                        : group.ide
                          ? "idle editor"
                          : "0"}
                    </span>
                  </button>

                  <ul className="sessions">
                    {group.sessions.map((session) => (
                      <li
                        key={session.sessionId}
                        className={`session ${session.state}${
                          isQuiet(session, now, quietAfterMs) ? " quiet" : ""
                        }${opened === session.sessionId ? " opened" : ""}`}
                        title={session.detail ?? undefined}
                        onClick={() =>
                          setOpened((current) =>
                            current === session.sessionId
                              ? null
                              : session.sessionId,
                          )
                        }
                      >
                        <span className="dot" aria-hidden="true" />
                        <span className="state">{LABEL[session.state]}</span>
                        {session.detail ? (
                          <span className="detail">{session.detail}</span>
                        ) : (
                          <span className="sid">
                            {shortId(session.sessionId)}
                          </span>
                        )}
                        <span className="time">
                          {ago(session.updatedAt, now)}
                        </span>
                      </li>
                    ))}
                  </ul>
                </li>
              ))}
            </ul>
          )}
        </div>

        <button
          className={`pin${pinned ? " on" : ""}`}
          title={
            pinned
              ? "Pinned open. Click to let it collapse on hover again."
              : "Keep open, so it does not collapse when you look away"
          }
          aria-pressed={pinned}
          tabIndex={expanded ? 0 : -1}
          onClick={() =>
            void invoke("set_pinned", { pinned: !pinned }).catch(() => {})
          }
        >
          {pinned ? "◉" : "○"}
        </button>
        <button
          className="close"
          title="Close Eve"
          tabIndex={expanded ? 0 : -1}
          onClick={() => void invoke("close_widget").catch(() => {})}
        >
          ×
        </button>
        <button
          className="gear"
          title="Settings"
          tabIndex={expanded ? 0 : -1}
          onClick={() => void invoke("open_settings").catch(() => {})}
        >
          <Cog />
        </button>
      </div>
    </div>
  );
}
