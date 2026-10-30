# paddock

A terminal workspace for one person on one Mac (or Linux machine). It keeps every terminal alive on its own background server (`paddock server`), draws them itself (tabs, splits, mouse, scrollback), and tracks coding agents' sessions (Claude Code, Gemini CLI and Antigravity CLI) and the branches they and their subagents work on, so you can see at a glance which ones need you.

Design: [docs/design.md](docs/design.md). Glossary: [CONTEXT.md](CONTEXT.md). Decisions: [docs/adr/](docs/adr/).

## Requirements

- macOS, or Linux on the native backend (the release tarballs are for Apple Silicon Macs and for x86_64 or aarch64 Linux; build from source elsewhere)
- On Linux, for notifications and copying: `notify-send` (libnotify, e.g. `apt install libnotify-bin`), and `wl-copy` (`wl-clipboard`) on Wayland or `xclip` on X11 (on Wayland, when `wl-copy` is missing or fails, paddock tries `xclip`, which works under XWayland). macOS has its own (`osascript`, `pbcopy`). Without one, paddock says so once in the tab bar and goes on; `PADDOCK_COPY_COMMAND` names another copy program (it reads the text on stdin)
- Claude Code, Gemini CLI and/or Antigravity CLI (for session tracking; Gemini CLI tested with 0.64, Antigravity CLI with 1.4.3)
- Rust 1.85+, only to build from source
- Optional: the GitHub CLI (`gh`, logged in), for pull request status on the dashboard, in the sidebar and in `paddock worktree list`

## Install

Pick one of these, then connect Claude Code, Gemini CLI or Antigravity CLI (see [Connect agents](docs/connect-agents.md)).

**Homebrew** (recommended):

```sh
brew install MiohitoKiri5474/paddock/paddock
```

If an earlier `brew install` failed while tapping, run `brew untap miohitokiri5474/paddock` first. Upgrade with `brew upgrade paddock` (see [Upgrade](docs/server.md#upgrade)).

**Release tarball** (Apple Silicon). Download `paddock-v0.1.0-aarch64-apple-darwin.tar.gz` and its `.sha256` from [Releases](https://github.com/MiohitoKiri5474/paddock/releases), then:

```sh
shasum -a 256 -c paddock-v0.1.0-aarch64-apple-darwin.tar.gz.sha256
tar xzf paddock-v0.1.0-aarch64-apple-darwin.tar.gz
xattr -d com.apple.quarantine paddock   # unsigned binary: lets Gatekeeper run it ("No such xattr" is fine)
sudo mkdir -p /usr/local/bin             # missing on a fresh Apple Silicon Mac
sudo mv paddock /usr/local/bin/         # or anywhere on your PATH
```

**Release tarball** (Linux, x86_64 or aarch64; glibc 2.34 or newer: Ubuntu 22.04, Debian 12, Fedora 35, RHEL 9 or later). Download `paddock-v0.1.0-x86_64-unknown-linux-gnu.tar.gz` (or `paddock-v0.1.0-aarch64-unknown-linux-gnu.tar.gz` on aarch64; `uname -m` says which) and its `.sha256` from [Releases](https://github.com/MiohitoKiri5474/paddock/releases), then:

```sh
sha256sum -c paddock-v0.1.0-x86_64-unknown-linux-gnu.tar.gz.sha256
tar xzf paddock-v0.1.0-x86_64-unknown-linux-gnu.tar.gz
mkdir -p ~/.local/bin && mv paddock ~/.local/bin/   # or anywhere on your PATH
```

zsh is not needed. For notifications and copying, install `notify-send` and `wl-copy` or `xclip` (optional; see [Requirements](#requirements)).

**Cargo**, from a tag:

```sh
cargo install --git https://github.com/MiohitoKiri5474/paddock --tag v0.1.0 --locked
```

If it fails with `failed to authenticate when downloading repository: git@github.com:…` (your git config rewrites GitHub URLs to SSH), let cargo use the `git` command: prefix the command with `CARGO_NET_GIT_FETCH_WITH_CLI=true`, or set `git-fetch-with-cli = true` under `[net]` in `~/.cargo/config.toml`.

**From source**, in a clone:

```sh
cargo install --path . --locked     # installs ~/.cargo/bin/paddock
```

### Upgrade

After you upgrade, run `paddock server upgrade`: the running server hands every terminal to the new paddock. A server from v0.1.0-rc.1 or rc.2 cannot hand over: start paddock and press Enter to restart it, which ends its programs and agents and brings back its layout with a fresh shell in each terminal. See [Upgrade](docs/server.md#upgrade).

## First run

```sh
paddock            # open (a second launch offers to take over)
paddock version    # or -V, --version: print `paddock <version>`, for bug reports
paddock help       # or -h, --help: list the commands
```

Open a tab with `C-a c` and run `claude` (or `gemini`, or `agy`) in it; the dashboard (`C-a o`) shows the branches it and its subagents work on. `C-a ?` lists the keys, and `C-a d` detaches (terminals go on).

## Docs

- [Connect agents](docs/connect-agents.md): Claude Code (the plugin, or `paddock hooks install`), Gemini CLI, Antigravity CLI, what each gets, and resuming after a restart
- [Use](docs/use.md): the sidebar, the dashboard, external sessions, pull requests, space roots
- [Keys](docs/keys.md): every key, zoom, resizing, the finder, floats, the mouse, copy mode, direct chords
- [Configure](docs/config.md): every `config.toml` section and option, with its default
- [Worktrees](docs/worktrees.md): `C-a W`, the `paddock worktree` command, and the worktree skills for each agent
- [Server](docs/server.md): upgrading, restoring layout and scrollback, moving from paddock 0.0.x, files, uninstall
- [Develop](docs/develop.md): building and testing

All pages: [docs/](docs/README.md).

## License

MIT, see [LICENSE](LICENSE).

## Credits

paddock's look and much of its behaviour (spaces in a sidebar with an agents section, the filled-block tab bar) are inspired by [herdr](https://github.com/herdrdev/herdr). paddock is a separate implementation and contains none of herdr's code.
