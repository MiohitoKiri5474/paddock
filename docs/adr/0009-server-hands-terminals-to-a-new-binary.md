# The server hands its terminals to a new binary by exec

Takes up the "possible later" of [ADR 0007](0007-native-backend-with-a-server.md): handing PTYs to the new binary on upgrade. Until now, upgrading meant restarting the server. That ended every program, and only the layout came back ([ADR 0008](0008-layout-saved-and-restored.md)). Now `paddock server upgrade` asks the running server to hand everything to a new binary: the one the command runs as, or the one it names. Agents, editors and builds keep running, with their screens and scrollback.

## How it goes

1. **The request skips the version check.** The upgrade runs exactly when client and server speak different protocol versions (ticket 51), so it cannot be a `Request` behind the hello.
   - The client reads the server's hello, whatever its version. Instead of its own hello, it sends one frame of kind 3, `Upgrade::Ask { exe }`, naming the binary to hand over to by an absolute path. The probe runs from `/` and `exec` from the server's directory, so the server refuses a relative path.
   - The server answers on the same connection. It sends `Upgrade::Probed` just before `exec`, then `Upgrade::Done { pid }` or `Upgrade::Refused(why)`.
   - This frame is frozen: it never changes with `protocol::VERSION`.
   - A server from before this ADR reads an unknown frame kind and closes the connection before answering. The client says the server cannot hand over (`NoHandover`), and paddock can still offer the restart.
   - A connection that ends after `Probed`, a broken frame, or no answer within 60 s is not that case. The client says what happened instead: past `Probed`, the server may have ended.
   - The server hands over to one binary at a time.
2. **The probe.** The server takes a record of what it holds (see step 4), with its state locked only while it does so. It runs `<exe> server --check-handover` with that record on stdin, for at most 10 s, and serves on meanwhile.
   - Only exit 0 with `paddock handover ok` on stdout counts as a yes.
   - A binary that does not know the format, or an older paddock that prints its usage, refuses. The first line of its stderr goes back to the command.
   - The server reads the probe's output without waiting, and closes it when the probe ends or is killed. A program the probe left running holds nothing of the server's.
3. **Freeze.** Once the probe says yes, the server takes its state lock and keeps it to the end. With the lock held:
   - No reader holds output it has not fed: readers wait for output with `poll`, outside the lock, and read it only once they hold the lock. So what they have not read stays in the PTY for the next server to read.
   - No waiter has reaped a program it has not marked: a waiter sees an exit with `waitid(…, WNOWAIT)`, which leaves the program unreaped, and reaps it and marks its terminal under the lock.
   - Every program has taken all its queued input. If some input still waits, the server lets go of the lock and looks again, for up to 2 s. Then it refuses ("input still waits for vim in %3"), rather than lose part of a paste.
   - `layout.json`'s writer is idle: the server also holds its lock. It writes the layout as it is now, whatever the 250 ms gap, so a new server that fails leaves the next one the layout of the moment of the switch.

   So the server stalls for the freeze, the second record and the `exec`, not for the probe.
4. **The record.** The server writes what it hands over to a temp file in the state directory, removed as soon as it is open. The record has its own version (`handover::FORMAT`), apart from the client protocol, so two paddocks of different protocol versions can hand over as long as the new one reads the old one's format. It holds:
   - the instance id, the last id given, the current space and the restored count;
   - every space, tab and float with their ids, names, trees, shares, focus, focus order and zoom;
   - every terminal's id, shell pid, tty, size, whether it was reaped, and the number of its PTY's master descriptor;
   - the descriptors of `server.lock` and the listening socket;
   - each terminal's snapshot: the bytes a client attaches with (scrollback, both screens, cursor, modes, scroll region, modifyOtherKeys), as raw blobs after the JSON header.

   The probe reads the record taken before the freeze, and the new image reads the one taken during it. The two differ only in content (output since, a new tab), never in format or kind, and both pass the same checks. A content-only difference is all the probe cannot vouch for.

   So that every later paddock can still take over from this one, the format changes by two rules. A field added to the header gets a default (`#[serde(default)]`), and the format stays the same. A change that cannot work that way bumps `FORMAT`, and the new paddock keeps a reader for the format before it.
5. **Exec.** The server tells every attached client `Notice::Upgrading` and lets each go once it has read that (at most 1 s). It sends `Probed`, then clears close-on-exec on these descriptors, which keep their numbers:
   - the lock;
   - the listening socket;
   - every PTY master;
   - the record;
   - the requesting connection.

   It then `exec`s `<exe> server --take-over <record> <connection>`. Every other descriptor closes.
6. **Take over.** The new image reads the record, then closes every descriptor it holds that the record and its command line do not name (see below). It rebuilds every space with the same ids. Each terminal gets a fresh emulator fed its snapshot; the replies the snapshot provokes are dropped, not sent to the program. The new image starts each terminal's reader and waiter. It rebuilds which agent session runs in each terminal (ADR 0008's resume) by reading its hook log again from the start: the instance is the same, so the log is too. It then answers `Done` on the inherited connection. A session resumed in the seconds before the switch, which has not reported yet, is not saved as resumed until it does. A resume command not yet typed (an upgrade within about 3 s of a restore) is dropped, since the thread that types it does not survive the `exec`: that terminal keeps a plain shell. Then it serves as any server does, without binding, locking or restoring `layout.json`.
7. **Clients attach again.** Once a paddock has been told `Upgrading` on any space's connection, it takes the end of a space's connection (`Event::Lost`) as part of the upgrade, not as a lost server. It does so for every space, also one let go before it read the notice (more than 1 s behind).
   - **When.** Only once every space's connection made before the notice has ended does it attach every space again. A space made meanwhile is already on the new server, so it is not waited for. It attaches the same spaces in the same order, with the same one current. So a busy space whose notice comes late never sees a second attach.
   - **Old connections.** paddock numbers each connection. It closes the replaced ones and leaves out whatever they still report (late output, notices, their end), so nothing old reaches the new screens.
   - **Waiting.** It asks until the new image answers, its hello and then its spaces, for up to 10 s, because the old image takes connections until its `exec` and closes them unanswered. A connection closed unanswered is never taken for a server with no spaces. On Linux the old image cannot start a thread to serve a connection while another of its threads `exec`s, so it closes that connection, and never lets that end it.
   - **Where it lands.** Each space's current tab and focus are kept by the server, so they come back with it. paddock says "paddock server upgraded: programs kept running".
   - **What is lost.** Keys typed between the notice and the new attach go to a closed connection and are lost.
   - If the `exec` fails, the clients attach again to the old server.
   - A paddock that cannot talk to the new server (it speaks another protocol version, or failed) quits, saying why. One that finds no server (the new image failed and ended) says the new server did not take over, and points to `server.log`.
   - A paddock more than 1 s behind on every space is let go before it reads the notice on any of them. It takes that as a lost server and quits; the next launch attaches.
   - A paddock from before this ADR cannot read the notice, takes it as a lost connection, and quits as before.

## Who owns what during the switch

- **The lock and the socket.** `exec` keeps the process, so the pid, the `flock` on `server.lock` (and the pid written in it) and the listening socket never change hands. No second server can start, and no client finds the socket missing. A connection that comes during the switch waits in the listen backlog and is served by the new image. A connection that the old image had already accepted closes unanswered, as at a restart. On macOS, `accept` sets close-on-exec only after it returns, so a connection accepted just as the old image `exec`s can pass over without anyone knowing it: the new image closes every descriptor it was not handed, before it starts a thread. Otherwise that client would wait for a hello forever.
- **The programs.** They stay children of the same process, so the new image reaps them with `waitpid`. A program that exited during the switch is still a zombie, or was reaped and marked before the record was written: never reaped by nobody, never signalled after it was reaped. A waiter that finds no child (`ECHILD`) treats the program as gone.
- **Instance id and terminal ids.** They stay the same, so `PADDOCK_PANE`, `PADDOCK_INSTANCE` and the hook event log `events/native-<id>.jsonl` still name the right terminals and server.

## Failure at each step

| Step | Failure | What happens |
| --- | --- | --- |
| Request | Server from before this ADR | It closes the connection; the command says it cannot hand over, and how to restart instead |
| Request | Server ending, or a relative path | `Refused`, nothing changes |
| Probe | Cannot write the record; binary missing, refuses, prints no token, or runs past 10 s (killed and reaped) | `Refused(its words)`: the old server serves on, unchanged |
| Freeze | Input still waits after 2 s | `Refused("input still waits for … in …")`, the lock is let go |
| Freeze | Cannot write the record or `layout.json` | `Refused(why)`, the lock is let go |
| Exec | `exec` returns an error | Close-on-exec is put back, `Refused(why)` after `Probed`, the lock is let go: the old server serves on |
| Take over | The new image cannot rebuild what the probe accepted (out of descriptors or memory) | It answers `Refused(why)` (saying it has ended) and exits. The programs are hung up as their PTYs close. `layout.json`, written just before `exec`, rebuilds the layout at the next start, as after a crash |
| Take over | The new image dies before answering | The command says the server ended as it took over (see `server.log`); an attached paddock finds no server to attach to, and quits saying so |

Every step before exec leaves the old server running as it was. After exec, nothing can go back: the old image is gone, and its binary may be too (a package manager replaces it). The probe makes the last two rows a matter of resources only.

## Testing

The harness tests hand over to the same binary and to a copy of it: the copy shows that the record and the descriptors really pass into another binary. On the client side, fake servers stand in for the cases below:

- one speaking protocol version 999;
- one from before this ADR;
- one that refuses before the switch (`Refused` with no `Probed`), which paddock's prompt (ticket 51) answers with the restart prompt;
- one that ends after `Probed`.

For the prompt, a proxy stands between paddock and a real server: it moves the server's socket aside, listens in its place, and passes every byte both ways, but rewrites the server's hello to say version 999 until the server answers `Done`. So paddock finds a server of "another version", and the upgrade it asks for is real: the probe, the freeze, the `exec` of this build's binary, and the programs running on. The proxy can also drop `Done`, to test that the same server answering afterwards means done, and then give another pid, to test that another server means the old one ended.

A real handover between two protocol versions, or two record formats, needs two releases built from different sources. That cannot be tested until a second paddock with this ADR exists. Until then, the frozen frame is pinned byte for byte by a unit test, and a record of another format is refused by number.

## Considered options

- **Pass the descriptors to a new process over a Unix socket (`SCM_RIGHTS`).** The old server could keep running until the new one said it was ready, so every failure would leave it unchanged. But the programs would stay children of the old process. When it exits, launchd adopts them, and macOS has no subreaper to make the new server their parent. The new server could only watch them with `kqueue`, and could signal a pid already reaped and reused, which paddock never does. The lock and the socket would also have to change hands between two processes.
- **Probe the frozen record**: the probe would vouch for the very record the new image reads, but the server would stall for up to 10 s while it runs.
- **Restart, as before** ([ADR 0007](0007-native-backend-with-a-server.md)): stays for servers that cannot hand over.
- **Keep the client's connection through the switch**: it would have to pass to the new image mid-frame, and the client may not speak the new protocol. Attaching again costs one snapshot per terminal, which an attach sends anyway.
- **Let attached clients quit, and attach on the next launch**: simpler, but every upgrade would close the window the user works in.
- **Tell an upgrade from a lost server by retrying while the server's pid lives**: needs no new notice, but a client the server lets go for not reading, or one replaced by a takeover, would attach again too.

## Consequences

- `paddock server upgrade [<binary>]` hands the running server to the binary it is run as, or to `<binary>`.
- A new paddock that finds a server of another protocol version offers the same upgrade, to its own binary ("Upgrade it? Its programs keep running. [Y/n]"; ticket 51, `Backend::upgrade_other`), and then opens on it. It tells how the upgrade went by what answers afterwards, not by the answer alone, because a `Refused` after `Probed` can come from a failed `exec` (the old server serves on) or from the new image (it has ended):
  - The same server (the same pid) answers at this paddock's version: the upgrade is done, even if `Done` was lost. Another pid means the old server ended and another started since, from `layout.json`: as below, its programs have ended.
  - No hello within 5 s: paddock cannot tell, and stops saying so.
  - The server still answers at the other version: it did not hand over (from before this ADR, a refusal, or no answer). paddock says why and offers the restart of [ADR 0007](0007-native-backend-with-a-server.md) as before.
  - Nothing answers: it failed after the switch, and its programs have ended. paddock says so and offers no restart, as nothing runs to restart. It waits for the lock to go and starts a new server, which rebuilds `layout.json` as after a restart.
  - Saying no to either prompt stops paddock, as before.
- An attached paddock attaches again by itself and keeps its place. `Notice::Upgrading` is a new message, so the protocol is version 2.
- What a snapshot does not carry is lost at the switch, as on reattach: charsets, the title and hyperlinks.
- A signal that comes between `exec` and the new image's handlers takes the default action: SIGTERM ends the server without hanging its shells up in order.
- Output and input are never lost or doubled at the switch. Input waiting for a program that does not read refuses the upgrade instead.
