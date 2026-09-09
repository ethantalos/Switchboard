import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import "./App.css";

type SessionState = "needs_you" | "working" | "done" | "idle";

type Session = {
  sessionId: string;
  cwd: string;
  state: SessionState;
  detail: string | null;
  updatedAt: number;
};

const LABEL: Record<SessionState, string> = {
  needs_you: "needs you",
  working: "working",
  done: "done",
  idle: "idle",
};

const RANK: Record<SessionState, number> = {
  needs_you: 0,
  working: 1,
  done: 2,
  idle: 3,
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
};

/// One row per worktree, with its sessions underneath. A worktree takes the
/// state of its most urgent session, so the group header is the thing to scan.
function groupByWorktree(sessions: Session[]): Group[] {
  const byCwd = new Map<string, Session[]>();
  for (const session of sessions) {
    const list = byCwd.get(session.cwd);
    if (list) list.push(session);
    else byCwd.set(session.cwd, [session]);
  }

  const groups: Group[] = [];
  for (const [cwd, list] of byCwd) {
    const ordered = [...list].sort(
      (a, b) => RANK[a.state] - RANK[b.state] || b.updatedAt - a.updatedAt,
    );
    groups.push({
      cwd,
      sessions: ordered,
      state: ordered[0].state,
      updatedAt: Math.max(...ordered.map((s) => s.updatedAt)),
    });
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
  const [expanded, setExpanded] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [now, setNow] = useState(Date.now());

  useEffect(() => {
    void invoke<Session[]>("list_sessions").then(setSessions).catch(() => {});

    // Rust owns hover: the window never resizes, so it watches the pointer and
    // tells us when to grow. Nothing here changes window geometry.
    const offHover = listen<boolean>("hover-changed", (event) => {
      setExpanded(event.payload);
    });
    const offSessions = listen<Session[]>("sessions-changed", (event) =>
      setSessions(event.payload),
    );

    const ticker = setInterval(() => setNow(Date.now()), 10_000);

    return () => {
      void offHover.then((off) => off());
      void offSessions.then((off) => off());
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

  const groups = groupByWorktree(sessions);
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
                  No sessions yet. Start a Claude Code session and it will appear
                  here.
                </p>
              ) : (
                <ul className="worktrees">
                  {groups.map((group) => (
                    <li key={group.cwd} className="worktree">
                      <button
                        className={`worktree-head ${group.state}`}
                        onClick={() => open(group.cwd)}
                        title={group.cwd}
                      >
                        <span className="dot" aria-hidden="true" />
                        <span className="worktree-name">
                          {basename(group.cwd) || "unknown"}
                        </span>
                        <span className="worktree-count">
                          {group.sessions.length}
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
              title="Close Switchboard"
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
