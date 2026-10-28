# The server hands its terminals to a new binary by exec

Takes up the "possible later" of [ADR 0007](0007-native-backend-with-a-server.md): handing PTYs to the new binary on upgrade. Until now, upgrading meant restarting the server. That ended every program, and only the layout came back ([ADR 0008](0008-layout-saved-and-restored.md)). Now `paddock server upgrade` asks the running server to hand everything to the binary it runs as. Agents, editors and builds keep running, with their screens and scrollback.

## How it goes

1. **The request skips the version check.** The upgrade runs exactly when client and server speak different protocol versions (ticket 51), so it cannot be a `Request` behind the hello. The client reads the server's hello, whatever its version, and instead of its own hello sends one frame of kind 3, `Upgrade::Ask { exe }`, with the binary to hand over to. The server answers on the same connection with `Upgrade::Done { pid }` or `Upgrade::Refused(why)`. This frame is frozen: it never changes with `protocol::VERSION`. A server from before this ADR reads an unknown frame kind and closes the connection. The client then says the server cannot hand over, and paddock can still offer the restart.
2. **Freeze.** The server takes its state lock and keeps it to the end. With the lock held:
   - No reader holds output it has not fed: readers wait for output with `poll`, outside the lock, and read it only once they hold the lock. So what they have not read stays in the PTY for the next server to read.
   - No waiter has reaped a program it has not marked: a waiter sees an exit with `waitid(…, WNOWAIT)`, which leaves the program unreaped, and reaps it and marks its terminal under the lock.
   - Every program has taken all its queued input. If some input still waits, the server lets go of the lock and looks again, for up to 2 s. Then it refuses ("input still waits for vim in %3"), rather than lose part of a paste.
   - `layout.json`'s writer is idle: the server also holds its lock.
3. **The record.** The server writes what it hands over to a temp file in the state directory, removed as soon as it is open. The record has its own version (`handover::FORMAT`), apart from the client protocol, so two paddocks of different protocol versions can hand over as long as the new one reads the old one's format. It holds:
   - the instance id, the next id, the current space and the restored count;
   - every space, tab and float with their ids, names, trees, shares, focus, focus order and zoom;
   - every terminal's id, shell pid, tty, size, whether it was reaped, and the number of its PTY's master descriptor;
   - the descriptors of `server.lock` and the listening socket;
   - each terminal's snapshot: the bytes a client attaches with (scrollback, both screens, cursor, modes, scroll region, modifyOtherKeys), as raw blobs after the JSON header.

   So that every later paddock can still take over from this one, the format changes by two rules. A field added to the header gets a default (`#[serde(default)]`), and the format stays the same. A change that cannot work that way bumps `FORMAT`, and the new paddock keeps a reader for the format before it.
4. **The probe.** The server runs `<exe> server --check-handover` with the record on stdin, for at most 10 s. Only exit 0 with `paddock handover ok` on stdout counts as a yes. A binary that does not know the format, or an older paddock that prints its usage, refuses, and its words go back to the command.
5. **Exec.** The server clears close-on-exec on these descriptors, which keep their numbers:
   - the lock;
   - the listening socket;
   - every PTY master;
   - the record;
   - the requesting connection.

   It then `exec`s `<exe> server --take-over <record> <connection>`. Every other descriptor closes, so attached clients see their connection end (`Event::Lost`).
6. **Take over.** The new image reads the record and rebuilds every space with the same ids. Each terminal gets a fresh emulator fed its snapshot; the replies the snapshot provokes are dropped, not sent to the program. The new image starts each terminal's reader and waiter, and answers `Done` on the inherited connection. Then it serves as any server does, without binding, locking or restoring `layout.json`.

## Who owns what during the switch

- **The lock and the socket.** `exec` keeps the process, so the pid, the `flock` on `server.lock` (and the pid written in it) and the listening socket never change hands. No second server can start, and no client finds the socket missing. A connection that comes during the switch waits in the listen backlog and is served by the new image. A connection that the old image had already accepted closes unanswered, as at a restart.
- **The programs.** They stay children of the same process, so the new image reaps them with `waitpid`. A program that exited during the switch is still a zombie, or was reaped and marked before the record was written: never reaped by nobody, never signalled after it was reaped. A waiter that finds no child (`ECHILD`) treats the program as gone.
- **Instance id and terminal ids.** They stay the same, so `PADDOCK_PANE`, `PADDOCK_INSTANCE` and the hook event log `events/native-<id>.jsonl` still name the right terminals and server.

## Failure at each step

| Step | Failure | What happens |
| --- | --- | --- |
| Request | Server from before this ADR | It closes the connection; the command says it cannot hand over |
| Request | Server ending | `Refused`, nothing changes |
| Freeze | Input still waits after 2 s | `Refused("input still waits for … in …")`, the lock is let go |
| Record | Cannot write the temp file | `Refused(why)`, the lock is let go |
| Probe | Binary missing, refuses, prints no token, or runs past 10 s (killed and reaped) | `Refused(its words)`, the lock is let go: the old server serves on, unchanged |
| Exec | `exec` returns an error | Close-on-exec is put back, `Refused(why)`, the lock is let go |
| Take over | The new image cannot rebuild what the probe accepted (out of descriptors or memory) | It answers `Refused(why)` and exits. The programs are hung up as their PTYs close. `layout.json`, written by the old image at most 250 ms before, rebuilds the layout at the next start, as after a crash |

Every step before exec leaves the old server running as it was. After exec, nothing can go back: the old image is gone, and its binary may be too (a package manager replaces it). The probe makes this last row a matter of resources only.

## Considered options

- **Pass the descriptors to a new process over a Unix socket (`SCM_RIGHTS`).** The old server could keep running until the new one said it was ready, so every failure would leave it unchanged. But the programs would stay children of the old process. When it exits, launchd adopts them, and macOS has no subreaper to make the new server their parent. The new server could only watch them with `kqueue`, and could signal a pid already reaped and reused, which paddock never does. The lock and the socket would also have to change hands between two processes.
- **Restart, as before** ([ADR 0007](0007-native-backend-with-a-server.md)): stays for servers that cannot hand over.
- **Keep the old client attached through the switch**: its connection would have to pass to the new image, mid-frame, and the old client may not speak the new protocol anyway. A paddock that attaches after the switch sees the same screens.

## Consequences

- `paddock server upgrade` hands the running server to the binary it is run as. Ticket 51's prompt calls the same thing (`Backend::upgrade`).
- An attached paddock loses its connection at the switch and quits saying so. The next launch attaches to the same terminals.
- What a snapshot does not carry is lost at the switch, as on reattach: charsets, the title and hyperlinks.
- A signal that comes between `exec` and the new image's handlers takes the default action: SIGTERM ends the server without hanging its shells up in order.
- Output and input are never lost or doubled at the switch. Input waiting for a program that does not read refuses the upgrade instead.
