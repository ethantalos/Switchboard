import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import "./App.css";

type SessionState = "needs_you" | "working" | "done" | "idle" | "unknown";

type Session = {
  sessionId: string;
  cwd: string;
  state: SessionState;
  detail: string | null;
  updatedAt: number;
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
  working: "working",
  done: "done",
  idle: "idle",
  unknown: "unknown",
};

const RANK: Record<SessionState, number> = {
  needs_you: 0,
  working: 1,
  done: 2,
  idle: 3,
  unknown: 4,
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
};

/// The same folder arrives spelled differently depending on the source -
/// hooks report `C:\Switchboard`, IDE locks report `c:\Switchboard` - so
/// group on a normalised key and keep the first spelling seen for display.
function pathKey(path: string): string {
  return path.replace(/[\\/]+$/, "").replace(/\\/g, "/").toLowerCase();
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
function groupByWorktree(sessions: Session[], workspaces: Workspace[]): Group[] {
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
      });
  }

  const groups = [...byKey.values()];
  for (const group of groups) {
    group.sessions.sort(
      (a, b) => RANK[a.state] - RANK[b.state] || b.updatedAt - a.updatedAt,
    );
    group.state = group.sessions[0]?.state ?? "unknown";
    group.updatedAt = group.sessions.reduce(
      (latest, s) => Math.max(latest, s.updatedAt),
      0,
    );
  }

  return groups.sort(
    (a, b) => RANK[a.state] - RANK[b.state] || b.updatedAt - a.updatedAt,
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
  const [expanded, setExpanded] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [now, setNow] = useState(Date.now());

  useEffect(() => {
    void invoke<Session[]>("list_sessions").then(setSessions).catch(() => {});
    void invoke<Workspace[]>("list_workspaces").then(setWorkspaces).catch(() => {});
    void invoke<string[]>("list_parked").then(setParked).catch(() => {});

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

    const ticker = setInterval(() => setNow(Date.now()), 10_000);

    return () => {
      void offHover.then((off) => off());
      void offSessions.then((off) => off());
      void offWorkspaces.then((off) => off());
      void offParked.then((off) => off());
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

  const groups = groupByWorktree(sessions, workspaces);
  const waiting = sessions.filter((s) => s.state === "needs_you").length;
  const working = sessions.filter((s) => s.state === "working").length;

  const pulse: SessionState =
    waiting > 0 ? "needs_you" : working > 0 ? "working" : sessions.length ? "done" : "idle";
  const count = waiting > 0 ? waiting : sessions.length;

  return (
    <div className="stage">
      <div
        className={`box ${expanded ? "expanded" : "collapsed"} ${pulse}`}
        data-tauri-drag-region
      >
        {!expanded && (
          <>
            <Mark />
            {count > 0 && <span className="count">{count}</span>}
          </>
        )}

        {expanded && (
          <>
            <div className="panel" data-tauri-drag-region>
              {error && (
                <p className="error" role="alert">
                  {error}
                </p>
              )}

              {groups.length === 0 ? (
                <p className="empty">
                  No sessions or editors found. Connect Claude Code in settings,
                  then start a session.
                </p>
              ) : (
                <ul className="worktrees">
                  {groups.map((group) => (
                    <li key={group.cwd} className="worktree">
                      <button
                        className={`worktree-head ${group.state}`}
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
                            className={`session ${session.state}`}
                          >
                            <span className="dot" aria-hidden="true" />
                            <span className="state">{LABEL[session.state]}</span>
                            <span className="sid">{shortId(session.sessionId)}</span>
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
              className="close"
              title="Close Eve"
              onClick={() => void invoke("close_widget").catch(() => {})}
            >
              ×
            </button>
            <button
              className="gear"
              title="Settings"
              onClick={() => void invoke("open_settings").catch(() => {})}
            >
              <Cog />
            </button>
          </>
        )}

      </div>
    </div>
  );
}
