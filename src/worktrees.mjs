// Worktree discovery. Parses `git worktree list --porcelain`, which is
// line-based, so checkout paths containing spaces survive intact.
// Plain .mjs for now; convert to TypeScript when the stack is chosen.

import { execFile } from "node:child_process";
import { promisify } from "node:util";

const run = promisify(execFile);

/** Parse porcelain output into worktree records. */
export function parseWorktrees(stdout) {
  return stdout
    .split(/\r?\n\r?\n/)
    .filter((block) => block.trim())
    .map((block) => {
      const wt = { path: null, head: null, branch: null, detached: false, bare: false, locked: false, prunable: false };
      for (const line of block.split(/\r?\n/)) {
        const sep = line.indexOf(" ");
        const key = sep === -1 ? line : line.slice(0, sep);
        const value = sep === -1 ? "" : line.slice(sep + 1);
        if (key === "worktree") wt.path = value;
        else if (key === "HEAD") wt.head = value;
        else if (key === "branch") wt.branch = value.replace(/^refs\/heads\//, "");
        else if (key === "detached") wt.detached = true;
        else if (key === "bare") wt.bare = true;
        else if (key === "locked") wt.locked = true;
        else if (key === "prunable") wt.prunable = true;
      }
      return wt;
    })
    .filter((wt) => wt.path);
}

/** List worktrees for the repository containing `repoPath`. */
export async function listWorktrees(repoPath) {
  const { stdout } = await run("git", ["worktree", "list", "--porcelain"], { cwd: repoPath });
  return parseWorktrees(stdout);
}

// Run directly: node src/worktrees.mjs <repo path>...
if (import.meta.filename === process.argv[1]) {
  for (const repo of process.argv.slice(2)) {
    console.log(`\n${repo}`);
    try {
      for (const wt of await listWorktrees(repo)) {
        console.log(`  ${wt.branch ?? (wt.detached ? "(detached)" : "(unknown)")}  ${wt.path}`);
      }
    } catch (err) {
      console.log(`  error: ${err.message.split("\n")[0]}`);
    }
  }
}
