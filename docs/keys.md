# Keys

The default keys after the prefix, in the sidebar, the dashboard and copy mode, and the direct chords. Back to the [README](../README.md).

Every key is an action with a name you can rebind (see [Configure](config.md)); `paddock keys` prints them all. By default, prefix `C-a`, then:

| Key | Action | Name |
|---|---|---|
| `c` | new tab in this space | `new_tab` |
| `C` | new space | `new_space` |
| `n` | next tab | `next_tab` |
| `p` | previous tab | `previous_tab` |
| `Tab` | last space and tab shown | `last_page` |
| `1` `2` `3` `4` `5` `6` `7` `8` `9` | go to tab 1, 2, ... 9 | `go_to_tab` |
| `,` | rename tab | `rename_tab` |
| `$` | rename space | `rename_space` |
| `R` | set space root here | `set_root` |
| `<` | move tab left | `move_tab_left` |
| `>` | move tab right | `move_tab_right` |
| `\|` | split side by side | `split_right` |
| `-` | split top and bottom | `split_down` |
| `h` `Left` | focus left | `focus_left` |
| `j` `Down` | focus down | `focus_down` |
| `k` `Up` | focus up | `focus_up` |
| `l` `Right` | focus right | `focus_right` |
| `=` | even out the focused split | `equalize` |
| `H` | move split border left | `resize_left` |
| `J` | move split border down | `resize_down` |
| `K` | move split border up | `resize_up` |
| `L` | move split border right | `resize_right` |
| `z` | zoom the focused split | `zoom` |
| `x` | close pane (asks if busy) | `close` |
| `f` | fork Claude into a new tab | `fork` |
| `W` | new worktree as a new space | `new_worktree` |
| `O` | open a worktree as a new space | `open_worktree` |
| `s` | focus sidebar on current agent | `focus_sidebar` |
| `w` | focus sidebar on the space | `focus_spaces` |
| `b` | show or hide sidebar | `toggle_sidebar` |
| `{` | sidebar narrower | `sidebar_narrower` |
| `}` | sidebar wider | `sidebar_wider` |
| `a` | next session needing you | `next_attention` |
| `o` | dashboard | `dashboard` |
| `g` | find a space, tab or agent | `finder` |
| `d` | detach (terminals go on) | `detach` |
| `[` | copy mode: move, select, copy | `copy_mode` |
| `/` | search the history | `search` |
| `?` | this help | `help` |
| `G` | show or hide lazygit in a float | `[floats] G` |

`C-a` pressed twice sends a literal `C-a`. `go_to_tab`'s first key goes to tab 1, its second to tab 2, and so on. `close` closes the pane; the last one closes its tab, the last tab its space, and it asks first if an agent is busy (`an agent is running: close? y/n`). Renaming, naming a new space (prefilled with `space-N`; `Enter` keeps it) and that question open a small box in the middle of the screen; `Esc` or a click outside it cancels. `{`/`}` can also be done by dragging the sidebar's border.

Zoom: `C-a z` makes the focused split fill its tab, and the tab bar shows a `Z` after the tab's name; `C-a z` again gives every split its place back, with the focus where it was. The other splits are hidden but keep running. Moving the focus (`C-a h/j/k/l`, a direct chord, or `C-a a` and the finder jumping to a hidden split), splitting, evening out (`C-a =`), or closing the zoomed split undoes the zoom first, as in tmux. With a float shown, `C-a z` hides the float, then zooms. The zoom lives on paddock's server with the tab's layout, so it survives detaching and comes back with a restored layout. (On the tmux backend it is tmux's `resize-pane -Z`, and a zoom made in `tmux -L paddock attach` shows in paddock too.) An agent in a hidden split that finishes a turn stays `done` until you see it. `zoom` has no direct chord by default.

Resizing: `C-a H`, `J`, `K` and `L` move the focused split's border left, down, up or right by 2 columns (or rows), the way tmux's `resize-pane` does: the border after the split, or the one before it for the last split of a row or column. After `C-a H`, the four keys work again without the prefix for 500 ms, each press starting the wait over, as tmux's `bind -r` does: `C-a H H H` moves the border 6 columns. Any other key ends that and is read as usual (so `H` typed later goes to the terminal); `[keys] repeat_ms` sets the wait, and `0` turns it off. You can also press on the line between two splits and drag it: the splits follow the pointer, and the size is kept on release. A press on that line never selects text. (On the tmux backend, dragging needs tmux 3.8 or later, whose layout paddock reads as JSON (`#{window_layout}`); with an older tmux a drag on that line does nothing, and the keys still work.) A split stops at 1 column or row, without a message. The sizes live on paddock's server (in tmux, on the tmux backend), so they survive detaching; `C-a =` evens them out again, and resizing a zoomed tab unzooms it first, as in tmux. The resize keys have no direct chord by default.

The finder (`C-a g`) opens a box framed like the prompts: a text field on top and, under it, every space (name, directory, branch), every tab (`space › tab`, directory, branch) and every agent (name, state, `space › tab`), each with its kind as a marker. Typing filters them fuzzily (case-insensitive unless you type an uppercase letter; contiguous and word-start matches rank first) against name, kind, directory and branch, best match first. `Up`/`Down` (or `C-p`/`C-n`) move, `Enter` shows the one selected (its space and tab, with its pane focused), `Esc` or a click outside cancels (a float shown stays shown); a click on a row opens it and the wheel moves the selection. While the field is empty, `b`, `w`, `i` and `d` show only agents that are waiting (blocked), running (working), idle or done, `a` all again, and the title says which; with text in the field they are ordinary characters. The finder's keys are fixed, except that `finder` in `[keys.commands]` names the key that opens it.

Floats (`[floats]`): `C-a G` shows lazygit in a box over the whole screen, sidebar included, framed like the prompts and titled with its command; `C-a G` again hides it and the program keeps running, so showing it again brings back the same program and state. When the program exits, the float closes, and the next `C-a G` starts it again. Each space has its own float per key, started in the focused terminal's directory; it lives on paddock's server with its space (so it survives detaching) but is never a tab, never in `C-a Tab`'s history, and paddock does not follow an agent in it. One float shows at a time. While it shows, every key, paste and mouse event inside it goes to it, except the prefix: `C-a` commands still work, and one that changes the page or gives the sidebar or dashboard the keyboard hides the float first. A click outside it does nothing. A float whose key you remove from `[floats]` is closed, with its program, the next time paddock opens. Floats are not saved in `layout.json`: after a server restart, their keys start them again. (On the tmux backend, floats are tmux windows, so `tmux -L paddock attach` lists them next to the tabs.)

The help (`C-a ?`) says how the prefix works in its top border, so the keys fit an 80x24 terminal.

## Sidebar

In the sidebar (`[keys.sidebar]`; prefix commands work too):

| Key | Action | Name |
|---|---|---|
| `j` `Down` | next space or agent | `down` |
| `k` `Up` | previous space or agent | `up` |
| `Enter` | show the space's tabs, or jump to the agent's pane | `open` |
| `x` | close the space (a worktree space paddock made: delete it, asking), or the agent's pane | `close` |
| `f` | fork the agent (on a space, its agent that needs you most) | `fork` |
| `W` | new worktree of the space's repository, as a new space | `new_worktree` |
| `o` | open a worktree of the space's repository as a new space | `open_worktree` |
| `Esc` | give the keyboard back | `back` |

## Dashboard

In the dashboard (`[keys.dashboard]`):

| Key | Action | Name |
|---|---|---|
| `j` `Down` | next session | `down` |
| `k` `Up` | previous session | `up` |
| `Enter` | jump to the session's terminal | `open` |
| `D` | dismiss the session | `dismiss` |
| `x` | close the session's terminal | `close` |
| `f` | fork the Claude session | `fork` |
| `p` | open the session's pull request in the browser | `pr` |
| `I` | type into the previewed terminal (Esc leaves) | `interact` |
| `/` | filter the list (Enter keeps, Esc clears) | `filter` |
| `Esc` | close the dashboard | `back` |

While typing into the preview, every key goes to the terminal except `Esc`, which only leaves and is never passed on; this is fixed, so a remap can never trap you there. `/` opens a one-line filter under the dashboard's title: the list keeps only the blocks whose session name, branch or state fuzzy-match what you type; `Enter` keeps the filter and returns the keys to the list, `Esc` clears it and leaves the filter line. Task rows show line changes, ahead/behind, the diff and changed files against the base branch (`origin/HEAD`, else `main`/`master`), the last commit's age, and its pull request in the PR column (below), else `merged` once the branch is in its base. Prompt editing (`Enter`, `Esc`, `Left`/`Right`, `Home`/`End`, `Backspace`/`Delete`, `C-u` (clear), `C-w` (delete word)), the dashboard filter's keys (the same editing; `Enter` keeps, `Esc` clears) and the `y` that confirms a close or a worktree delete are fixed too, as are `j`/`k`/`Enter`/`Esc` in the worktree list.

## Mouse

Mouse: click to focus a split, switch tabs (`+` after the tabs opens one, `<`/`>` scroll a full bar), or open a space or agent; the wheel over the tab bar switches tabs; wheel scrolls history; drag selects and copies (hold Shift over programs that use the mouse); dragging the line between two splits resizes them, and dragging the sidebar's border resizes the sidebar.

## Copy mode

`C-a [` puts the terminal you type into (the float shown, else the focused split) in copy mode: its view stops following the program, a cursor appears, and the keys of `[keys.copy]` move it over the whole history and the screen, select, copy and search. The program keeps running; what it writes meanwhile shows when you leave. The bottom line reads ` COPY  line N/M ` (with ` match i/n ` after a search) and as many of the keys as fit; it moves to the top while the cursor is on the bottom row. `C-a /` opens copy mode with the search prompt already open.

| Key | Action | Name |
|---|---|---|
| `h` `Left` | cursor left | `left` |
| `j` `Down` | cursor down | `down` |
| `k` `Up` | cursor up | `up` |
| `l` `Right` | cursor right | `right` |
| `w` | start of the next word | `next_word` |
| `b` | start of the word | `previous_word` |
| `e` | end of the word | `word_end` |
| `0` | start of the line | `line_start` |
| `$` | end of the line | `line_end` |
| `g` | top of the history | `top` |
| `G` | bottom of history | `bottom` |
| `C-u` | half a page up | `half_page_up` |
| `C-d` | half a page down | `half_page_down` |
| `C-b` | a page up | `page_up` |
| `C-f` | a page down | `page_down` |
| `v` | start or stop a selection | `select` |
| `V` | start or stop a line selection | `select_lines` |
| `y` `Enter` | copy the selection and leave | `copy` |
| `/` | search back towards the top | `search_up` |
| `?` | search on towards the bottom | `search_down` |
| `n` | next match, the search's way | `next_match` |
| `N` | previous match | `previous_match` |
| `Esc` `q` | leave without copying | `leave` |

`y` copies the selection to the clipboard the way dragging does (a line selection without its last newline) and leaves; with nothing selected it just leaves. `v` or `V` again stops selecting; the other switches kind. A search is plain text, case-insensitive unless it has an uppercase letter (as in the finder), over the whole history and screen, rows wrapped into one line counting as one; its prompt is edited like the others (`Enter` searches, `Esc` closes it), and paste goes into it. `Enter` jumps to the nearest match back up from the cursor for `/`, down for `?`; `n` goes on the same way, `N` the other, both wrapping around. Every match on screen is highlighted; with none, a notice says so and the cursor stays. The prefix works in copy mode, except in the search prompt, which takes every key as the other prompts do; a command leaves copy mode first. Direct chords do not fire. The wheel over the terminal scrolls the copy view; a click or drag leaves copy mode and acts as usual. Copy mode also ends when its terminal closes or leaves the screen, when the terminal is resized, and if more than 8 MiB of output piles up meanwhile (with a notice). A default copy key that is your prefix (`C-b` under `prefix = "C-b"`) is left out.

## Direct

Some commands also fire on one chord, no prefix (`[keys.direct]`), whenever a terminal has the keyboard: not in the sidebar, the dashboard, a prompt, the finder, copy mode or a float (a float gets every key but the prefix). Any action from the prefix table can be bound here; by default:

| Key | Action | Name |
|---|---|---|
| `C-M-c` | new tab in this space | `new_tab` |
| `C-\` | split side by side | `split_right` |
| `C--` | split top and bottom | `split_down` |
| `C-h` | focus left | `focus_left` |
| `C-j` | focus down | `focus_down` |
| `C-k` | focus up | `focus_up` |
| `C-l` | focus right | `focus_right` |

`C-h`, `C-j`, `C-k` and `C-l` are Backspace, Enter, clear and kill-line in many programs, and paddock takes them: a notice at start says so until you set `[keys.direct]` yourself (even empty, or to the same chords; `focus_left = ""` gives `C-h` back to the terminal). `C-M-c` needs a terminal that sends Alt, and paddock gives no warning about it (see [Configure](config.md)). Next and previous tab have no chord by default (`C-a n`, `C-a p`); `C-M-[` would reach paddock only under the kitty keyboard protocol (elsewhere it is two `Esc`). A direct chord matches exactly as bound (not with an extra Shift), and may be the same key as a `[keys.commands]` one, since those need the prefix first. The help (`C-a ?`) shows each command's chord after it in brackets.
