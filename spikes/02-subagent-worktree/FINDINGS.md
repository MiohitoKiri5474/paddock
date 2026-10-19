# Spike 02: subagent worktree, branch and directory discovery

Claude Code 2.1.303, `claude -p ... --permission-mode bypassPermissions --model haiku --settings <file>`, throwaway repo outside paddock. Prompt: spawn two parallel subagents, `isolation: "worktree"`, each creates `fix/a` / `fix/b` and commits a file. Both subagents ran as background (async) agents.

## Answer

- **Directory**: reliable from hooks. Every hook event fired for a subagent (SubagentStart, PreToolUse, PostToolUse, SubagentStop) carries `agent_id`, `agent_type` and `cwd` = the subagent's worktree path (`<repo>/.claude/worktrees/agent-<id>`). Hook process `$PWD` is the same. Events of the main agent have no `agent_id` and `cwd` = main repo.
- **Branch**: NOT in any hook payload and NOT reliably in the transcript. Get it from git: `git -C <cwd> branch --show-current` (or `git worktree list --porcelain`). Verified: worktrees end on `fix/a` and `fix/b`.
- **Transcript** is useful for directory only. Subagent transcript lines (`subagents/agent-<id>.jsonl`) have correct `cwd` (worktree) and `agentId`, `isSidechain: true`, but `gitBranch` is `"main"` on every line, even after the subagent ran `git switch -c fix/a`. It is stale (taken from the parent/start state). Do not use it.
- **meta.json** (`subagents/agent-<id>.meta.json`) has `worktreePath`, `spawnedWithWorktree: true`, `worktreeBranch` = the auto-created name `worktree-agent-<id>` (NOT the branch the agent later switched to), `description`, `agentType`, `toolUseId`, `finished`, `spawnDepth`, `requestShape`. `worktreeBranch` is only the initial branch; the agent renamed/switched away from it.

## Evidence per acceptance criterion

1. Session with worktree-isolated subagent creating a branch: done. `git worktree list` after: `.../repo [main]`, `.../.claude/worktrees/agent-a02c2bd27020e23b4 2aa1118 [fix/a]`, `.../agent-a9700dfeed5ef352c b957b23 [fix/b]`. Leftover local branches `worktree-agent-*` remained (the initial branches).
2. Hooks: see table.

| Event | agent_id | cwd | Other useful fields |
|---|---|---|---|
| SessionStart / UserPromptSubmit / Stop / SessionEnd (main) | no | main repo | `source`, `prompt`, `last_assistant_message`, `background_tasks`, `reason` |
| PreToolUse Agent (main) | no | main repo | `tool_input.isolation: "worktree"`, `description`, `prompt`, `subagent_type` (no worktree path yet) |
| PostToolUse Agent (main) | no | main repo | `tool_response.agentId`, `status: async_launched`, `outputFile`; no path/branch |
| SubagentStart | yes | worktree | `agent_id`, `agent_type`; fires right after creation, so `cwd` is known from the start |
| PreToolUse/PostToolUse (subagent) | yes | worktree | `agent_type`, tool input/response |
| SubagentStop | yes | worktree | `agent_transcript_path`, `last_assistant_message`, `background_tasks[]` (id, status, description, agent_type) |

   A subagent's Bash `git switch -c` shows only as a normal Bash tool event (command text in `tool_input.command`); no branch field.
3. Transcript: see Answer. Per-line `cwd` correct, `gitBranch` stale, meta.json as above. Transcript path is also in hooks: `transcript_path` (main), `agent_transcript_path` (SubagentStop).
4. Fixtures saved in `fixtures/`.

## Not observed

- `PermissionRequest`, `Notification`, `CwdChanged`: never fired (bypassPermissions, no cwd change). Payload shape unverified.
- `WorktreeCreate` was NOT registered: a WorktreeCreate hook replaces Claude's default worktree creation, so registering a logging hook would have changed the behaviour under test. `WorktreeRemove` never fired (worktrees removed manually by git afterwards, not by Claude).
- `$TMUX_PANE` was empty (not run inside tmux). Parallel hook writers interleaved lines in the shared log; paddock's hook writer should write each event atomically (single write, or a unique file per event).
- Not tested: foreground (sync) subagents, worktree with the subagent making no changes (Claude may auto-remove such a worktree), subagent never branching (stays on `worktree-agent-<id>`).

## Notes for state mapping

- Main `Stop` fired while both subagents were still running (async agents); its `background_tasks` lists them with `status: running`. A main `Stop` therefore does not mean subagents are done.
- SubagentStop = Done (carries `last_assistant_message`). Second agent's SubagentStop arrived after the main agent's next UserPromptSubmit (the completion notification).

## Recommendation for ticket 09

- **Row identity / grouping**: key subagent rows by `(session_id, agent_id)` from hooks; then collapse to one task row per branch (ADR 0004).
- **Directory**: hook `cwd` on SubagentStart (and any subagent-tagged event). Fallback: `worktreePath` in `subagents/agent-<id>.meta.json`.
- **Branch**: run `git -C <cwd> branch --show-current` on SubagentStart, then re-check on each subagent PostToolUse of a Bash tool (or poll) because the agent switches branch after start (starts as `worktree-agent-<id>`, detached/renamed later). Cross-check with `git worktree list --porcelain`. Never trust transcript `gitBranch`.
- **Description**: `meta.json.description`, or `tool_input.description` from PreToolUse Agent in the main session (match via `tool_response.agentId` on PostToolUse Agent, or `toolUseId` in meta). Fallback `background_tasks[].description`.
- **States**: Running = SubagentStart / any subagent tool event; Done = SubagentStop; Stopped = SessionEnd (or terminal gone) while a subagent has no SubagentStop; Waiting = PermissionRequest/Notification (shape unverified, confirm in a default permission-mode run); Idle = main `Stop`.
- Dismissed worktrees: if `cwd` no longer exists, mark the row stopped/gone rather than erroring.
