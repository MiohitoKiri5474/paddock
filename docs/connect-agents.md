# Agents

How to connect Claude Code, Gemini CLI and Antigravity CLI to paddock, and what each gets. Back to the [README](../README.md).

## Connect Claude Code

paddock tracks Claude sessions through Claude Code hooks. Install the paddock plugin, which carries them, in Claude Code:

```text
/plugin marketplace add MiohitoKiri5474/paddock
/plugin install paddock@paddock
```

Then restart running Claude sessions. The plugin's hooks run `paddock` from your `PATH`, so make sure `which paddock` finds it in the shell you start Claude from (Homebrew and `cargo install` put it there). Without `paddock` on `PATH` the hooks do nothing, quietly. The plugin also carries the worktree skills (see [Worktree skills for agents](worktrees.md#worktree-skills-for-agents)).

**Fallback, without the plugin**: write the hooks into `~/.claude/settings.json` instead.

```sh
cp ~/.claude/settings.json ~/.claude/settings.json.bak
paddock hooks install               # adds paddock's hooks to ~/.claude/settings.json
```

`hooks install` keeps other hooks (for example Supacode's), removes the old tmux-claude-session-manager hooks, and records the binary's full path, so install from a stable location. Re-run it after upgrading paddock or moving its binary. It says so when the plugin is enabled in that file.

`hooks install` and `hooks uninstall` take `--agent <kind>`, repeatable, to act only on that agent's hooks: `claude`, `gemini` or `agy`. Without it they act on Claude Code, as before, on Gemini CLI only when `gemini` is on your `PATH` (see [Connect Gemini CLI](#connect-gemini-cli)), and on Antigravity CLI only when `agy` is (see [Connect Antigravity CLI](#connect-antigravity-cli)). Each run reads every settings file it acts on before writing any: if one can't be read or parsed, it names that file, exits 1 and changes none. Each file is replaced atomically, but a write that fails partway (a full disk, say) can leave the files written before it. `hooks uninstall` never creates a settings file that does not exist, and keeps lists you left empty in it.

Both at once records each event once, as long as the `paddock` on your `PATH` is the release that added the plugin's hooks or newer: the plugin's hook steps aside when `settings.json` holds paddock's hook for that event. An older `paddock` records each event twice.

## Move from `hooks install` to the plugin

1. Upgrade paddock first (`brew upgrade paddock`, or reinstall it the way you installed it), so the `paddock` on `PATH` knows the plugin's hooks. Then get the plugin with its hooks. If you have the plugin already (an earlier one has no hooks), update it from your shell:

   ```sh
   claude plugin marketplace update paddock
   claude plugin update paddock@paddock
   ```

   Otherwise install it (see [Connect Claude Code](#connect-claude-code)).
2. Restart running Claude sessions, or run `/reload-plugins` in each; that loads the updated plugin, hooks included. Both sets of hooks now run, and each event is still recorded once.
3. Remove the settings hooks, then restart running Claude sessions (`/reload-plugins` does not reload settings hooks):

   ```sh
   paddock hooks uninstall --agent claude   # removes only paddock's hooks from ~/.claude/settings.json
   ```

Config and state are kept.

## Switch install method

With the plugin, nothing else changes: the new `paddock` only has to be on `PATH`. With `hooks install`, the hooks point at the old binary by its full path, so remove them with it before it goes, then install them again with the new one. For example, from Cargo to Homebrew:

```sh
# Detach paddock first (C-a d); your terminals keep running on its server.
~/.cargo/bin/paddock hooks uninstall --agent claude --agent gemini --agent agy
cargo uninstall paddock
brew install MiohitoKiri5474/paddock/paddock
hash -r && which paddock            # /opt/homebrew/bin/paddock
paddock hooks install --agent claude --agent gemini --agent agy
```

Name each agent whose hooks you installed (drop `--agent gemini` or `--agent agy` for one you never connected): a plain `hooks uninstall` or `hooks install` covers Gemini only while `gemini` is on your `PATH`, and agy only while `agy` is. Restart running sessions so they use the new hooks. Config and state are kept.

## Connect Gemini CLI

paddock tracks Gemini CLI sessions (hooks need Gemini CLI 0.26 or newer; tested with 0.64) through Gemini's hooks in `~/.gemini/settings.json` (`$GEMINI_CLI_HOME/.gemini/settings.json` if set). paddock's Gemini extension carries the worktree skill and commands but no hooks (see [Worktree skills for agents](worktrees.md#worktree-skills-for-agents)): install the hooks with paddock.

```sh
cp ~/.gemini/settings.json ~/.gemini/settings.json.bak
paddock hooks install --agent gemini   # adds paddock's hooks to ~/.gemini/settings.json
paddock hooks uninstall --agent gemini # removes only paddock's hooks
```

A plain `paddock hooks install` includes Gemini when `gemini` is on your `PATH`. The hooks (`SessionStart`, `BeforeAgent`, `AfterTool`, `Notification`, `AfterAgent`, `SessionEnd`) call `paddock hook --agent gemini <event>` by paddock's full path, with a 10 s timeout, so re-run the install after moving paddock's binary; your other hooks and settings stay. paddock cannot edit a `settings.json` with comments (Gemini allows them): it says `invalid JSON` and leaves the file as it was, so remove the comments or add the hooks by hand. A plain `paddock hooks install` with `gemini` on your `PATH` then changes no file at all, Claude's included: use `--agent claude` to skip Gemini's. Restart running Gemini sessions afterwards.

What Gemini sessions get: their row, dot, tab marker, state (running after a prompt, waiting on a tool permission, done or idle when a turn ends, gone when Gemini quits; `/clear` keeps the terminal's session), branch and task rows, `C-a a`, notifications, the EXTERNAL group, the AGENT tag `gemini`, and resuming after a server restart (`gemini --resume <id>`, see [Upgrade](server.md#upgrade)). What stays Claude Code's alone: fork, subagent rows (Gemini's subagents are not shown) and the `claude agents --json` snapshot. Without a snapshot, nothing catches what Gemini's hooks miss: after you press Esc or deny a permission, the row stays running or waiting until the next prompt. Gemini runs no hooks in a folder it does not trust (folder trust is on by default), so such a session does not show. An external Gemini session is kept alive by its hooks alone: waiting on you, it stays in the EXTERNAL group until it ends, you dismiss it, or nothing has been heard of it for 24 hours (as when Gemini was killed while waiting); in any other state, quiet for `[dashboard] external_timeout` minutes (idle at its prompt, say), it leaves until its next hook.

## Connect Antigravity CLI

paddock tracks Antigravity CLI (`agy`; tested with 1.4.3) sessions through agy's hooks in `~/.gemini/config/hooks.json`, a file keyed by hook name, and, in paddock's terminals, through the status agy reports to its terminal (OSC 7501, agy 1.4.0 or newer). paddock's agy plugin carries the worktree skill but no hooks (see [Worktree skills for agents](worktrees.md#worktree-skills-for-agents)): install the hooks with paddock.

```sh
cp ~/.gemini/config/hooks.json ~/.gemini/config/hooks.json.bak   # if you have one
paddock hooks install --agent agy     # adds paddock's entry, "paddock", to ~/.gemini/config/hooks.json
paddock hooks uninstall --agent agy   # removes only that entry
```

A plain `paddock hooks install` includes agy when `agy` is on your `PATH`. paddock's entry holds three hooks (`SessionStart`, `PreInvocation`, `Stop`) that call `paddock hook --agent agy <event>` by paddock's full path, with a 10 s timeout, and answer `{}` (agy reads a hook's reply as JSON); re-run the install after moving paddock's binary. Your other named hooks in the file stay, and `uninstall` never creates the file. Hooks in `~/.gemini/antigravity-cli/settings.json` or a workspace's `.agents/hooks.json` are not touched. Restart running agy sessions afterwards.

States: idle when agy starts, running when it starts on a prompt (`PreInvocation`), done or idle once it is fully idle again (`Stop`). Waiting comes only from agy's status reports, read from the terminal's output while paddock runs: paddock sets `AGY_CLI_OSC7501=on` in its terminals, since agy reports only to a terminal that answers its check, and paddock's server (tmux, on the tmux backend) answers in paddock's place (that it does not support them). There agy also reports working, done and failed (failed counts as done); its idle reports never notify, so starting agy and quitting without a prompt says nothing. paddock records a report only while an agy session runs in that terminal, and not again while it says the same. Any program run inside an agy session's terminal can write the same report (`app=antigravity`) and so set that session's state. A terminal opened before you upgraded paddock lacks the variable: open a new one, or `export AGY_CLI_OSC7501=on` in it before running `agy`. Setting it `off` in your shell turns waiting off.

agy sends no hook when it quits: its session ends when its terminal closes, or when the terminal is back at a shell (paddock looks every 3 s, while an agy session runs in one of its terminals). Run `agy` from a shell: started through another program that stays in the foreground (`npx`, `node`, a wrapper script), it ends only when that program quits back to the shell or the terminal closes.

What agy sessions get: their row, dot, tab marker, state, branch and task rows (its first workspace stands in for the directory), `C-a a`, notifications, the EXTERNAL group, the AGENT tag `agy`, and resuming after a server restart (`agy --conversation <id>`, see [Upgrade](server.md#upgrade)). What stays Claude Code's alone: fork, subagent rows and the `claude agents --json` snapshot. An external agy session lives on its hooks alone: no status reports reach paddock from another terminal, so it is never waiting, and with no end event and no snapshot it leaves once nothing has been heard of it for `[dashboard] external_timeout` minutes (after it quit, or idle at its prompt), until its next hook.

Not checked yet, as agy needs a sign-in: that agy's hooks run with the terminal's `PADDOCK_PANE` and `PADDOCK_SOCKET` (or, on the tmux backend, `TMUX` and `TMUX_PANE`), by which paddock tells which terminal an event comes from. If they don't, every agy session shows under EXTERNAL, even one running in a paddock terminal, and is never waiting (its status reports find no session in that terminal). Please report it if you see that.

## Resume after a restart

This is what happens to agent sessions when paddock restarts its server (see [Upgrade](server.md#upgrade)).

A terminal that was running an agent session you had not quit comes back running that same session: paddock types the agent's resume command into its new shell, in its saved directory, once the shell has started, so quitting the agent leaves you at a prompt, and the dashboard shows it as the same session. The commands are `claude --resume <session id>` (Claude Code), `gemini --resume <session id>` (Gemini CLI, whose sessions are kept per directory) and `agy --conversation <session id>` (Antigravity CLI). agy says nothing when it quits, so its session counts as quit once its terminal is back at a shell (paddock looks every 100 ms); the same agy back in the foreground (`fg` after `C-z`, not a fresh `agy`) or any hook of its session takes that back. The notice says how many resume commands it typed ("restored 2 spaces, resumed 1 session"). A session you had quit and one whose directory is gone get a plain shell. So does a session whose saved id is not a UUID: paddock types only UUIDs into a shell (every agent's hooks give one), and the notice names those by agent ("could not resume 1 gemini session"). An agent that quits from a signal less than 2 s before the server ends counts as stopped by the same shutdown (a reboot) and is resumed. Gemini says `exit` for a signal as for `/quit`, so a Gemini session you quit just before the restart comes back too. Gemini's and agy's resume commands follow their docs and have not been run against the real CLIs yet. The first restart after upgrading from a paddock that did not resume sessions (one whose server speaks protocol version 1) resumes nothing: that server saved no sessions.
