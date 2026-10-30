//! A running paddock in a pseudo-terminal, and what it draws.

use super::*;

/// A running paddock in a pseudo-terminal.
pub struct Paddock {
    pub(super) child: Box<dyn Child + Send + Sync>,
    /// Shared with the reader, which answers paddock's cursor queries.
    pub(super) writer: Arc<Mutex<Box<dyn Write + Send>>>,
    master: Box<dyn MasterPty + Send>,
    /// Held while paddock runs, and closed once it is dropped: macOS
    /// throws away what a pseudo-terminal still holds unread once its
    /// last slave closes, so what paddock writes as it exits (an error)
    /// would be lost to a reader slower than the exit.
    _slave: Box<dyn SlavePty + Send>,
    screen: Arc<Mutex<vt100::Parser>>,
}

impl Paddock {
    pub(super) fn launch(sandbox: &Sandbox) -> Self {
        let pair = native_pty_system()
            .openpty(PtySize {
                rows: ROWS,
                cols: COLS,
                pixel_width: 0,
                pixel_height: 0,
            })
            .expect("openpty");
        let mut cmd = CommandBuilder::new(env!("CARGO_BIN_EXE_paddock"));
        cmd.env_clear();
        for (k, v) in sandbox.env() {
            cmd.env(k, v);
        }
        cmd.cwd(sandbox.path("home"));
        let child = pair.slave.spawn_command(cmd).expect("spawn paddock");
        let writer = Arc::new(Mutex::new(pair.master.take_writer().expect("writer")));
        let screen = Arc::new(Mutex::new(vt100::Parser::new(
            nonzero(ROWS),
            nonzero(COLS),
            0,
        )));
        let reader = pair.master.try_clone_reader().expect("reader");
        spawn_reader(reader, Arc::clone(&screen), Arc::clone(&writer));
        Self {
            child,
            writer,
            master: pair.master,
            _slave: pair.slave,
            screen,
        }
    }

    /// Bytes as an outer terminal would send them.
    pub fn send(&mut self, bytes: &[u8]) {
        let mut writer = self.writer.lock().expect("writer lock");
        writer.write_all(bytes).expect("write to pty");
        writer.flush().expect("flush pty");
    }

    pub fn type_line(&mut self, line: &str) {
        self.send(line.as_bytes());
        self.send(b"\r");
    }

    /// A wheel notch at (col, row), 0-based, as an SGR mouse report. Like
    /// every mouse event here, `col` counts from the right of the sidebar,
    /// as [`Self::rows`] does.
    pub fn wheel(&mut self, up: bool, col: u16, row: u16) {
        let button = if up { 64 } else { 65 };
        let col = col + self.left();
        self.send(format!("\x1b[<{button};{};{}M", col + 1, row + 1).as_bytes());
    }

    /// A left-button mouse event at (col, row), 0-based, as an SGR report:
    /// `code` 0 is a press, 32 a drag; `release` ends the press. Shift adds 4.
    pub fn mouse(&mut self, code: u16, col: u16, row: u16, release: bool) {
        let col = col + self.left();
        self.send_mouse(code, col, row, release);
    }

    /// A left click at screen column `col` (counting from the screen's left
    /// edge, so it can hit the sidebar) and row `row`, 0-based.
    pub fn click_screen(&mut self, col: u16, row: u16) {
        self.send_mouse(0, col, row, false);
        self.send_mouse(0, col, row, true);
    }

    /// Press at screen column `from` of `row`, drag to column `to` and
    /// release there, counting from the screen's left edge.
    pub fn drag_screen(&mut self, row: u16, from: u16, to: u16) {
        self.send_mouse(0, from, row, false);
        self.send_mouse(32, to, row, false);
        self.send_mouse(0, to, row, true);
    }

    fn send_mouse(&mut self, code: u16, col: u16, row: u16, release: bool) {
        let fin = if release { 'm' } else { 'M' };
        self.send(format!("\x1b[<{code};{};{}{fin}", col + 1, row + 1).as_bytes());
    }

    /// A left click at (col, row), 0-based.
    pub fn click(&mut self, col: u16, row: u16) {
        self.mouse(0, col, row, false);
        self.mouse(0, col, row, true);
    }

    /// Press at `from`, drag to `to` and release there, as (col, row) 0-based.
    pub fn drag(&mut self, from: (u16, u16), to: (u16, u16), shift: bool) {
        let mods = if shift { 4 } else { 0 };
        self.mouse(mods, from.0, from.1, false);
        self.mouse(32 + mods, to.0, to.1, false);
        self.mouse(mods, to.0, to.1, true);
    }

    /// Where `text` is first drawn, as (col, row) 0-based, right of the sidebar.
    pub fn find(&self, text: &str) -> Option<(u16, u16)> {
        self.rows().iter().enumerate().find_map(|(y, r)| {
            let x = r.find(text)?;
            Some((
                u16::try_from(r[..x].chars().count()).ok()?,
                u16::try_from(y).ok()?,
            ))
        })
    }

    /// The first screen column right of the sidebar (0 while it is hidden).
    pub fn left(&self) -> u16 {
        let first = self.full_rows().into_iter().next().unwrap_or_default();
        let border = first.chars().nth(usize::from(SIDEBAR - 1));
        if first.starts_with(SIDEBAR_TITLE) && border == Some('│') {
            SIDEBAR
        } else {
            0
        }
    }

    /// The sidebar's rows, without its border, trimmed at the end; empty
    /// while it is hidden.
    pub fn sidebar(&self) -> Vec<String> {
        if self.left() == 0 {
            return Vec::new();
        }
        let parser = self.screen.lock().expect("screen lock");
        parser
            .screen()
            .rows(0, SIDEBAR - 1)
            .map(|r| r.trim_end().to_string())
            .collect()
    }

    /// Resize the outer terminal (sends SIGWINCH to paddock).
    pub fn resize(&mut self, cols: u16, rows: u16) {
        let size = PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        };
        self.master.resize(size).expect("resize pty");
        self.screen
            .lock()
            .expect("screen lock")
            .screen_mut()
            .set_size(nonzero(rows), nonzero(cols));
    }

    /// The prefix (`C-a`) followed by a command key.
    pub fn command(&mut self, key: &[u8]) {
        self.send(b"\x01");
        self.send(key);
    }

    /// The tab bar row.
    pub fn tab_bar(&self) -> String {
        self.rows().into_iter().next().unwrap_or_default()
    }

    /// The tabs in the tab bar, numbered as the keys `1`-`9` reach them
    /// (`1:home`), and which one is current. Tabs are the filled blocks of
    /// the bar: cells with a background of their own (the default tab and
    /// current tab colours).
    pub fn tabs(&self) -> (Vec<String>, Option<usize>) {
        let left = self.left();
        let parser = self.screen.lock().expect("screen lock");
        let screen = parser.screen();
        let width = screen.size().1.get();
        let mut tabs: Vec<(String, bool)> = Vec::new();
        let mut open = false;
        for x in left..width {
            let Some(cell) = screen.cell(0, x) else { break };
            let bg = cell.bgcolor();
            let current = bg == CURRENT_TAB;
            if bg != TAB && !current {
                open = false;
                continue;
            }
            if !open {
                tabs.push((String::new(), current));
                open = true;
            }
            if let Some(tab) = tabs.last_mut() {
                tab.0.push_str(if cell.contents().is_empty() {
                    " "
                } else {
                    cell.contents()
                });
            }
        }
        let current = tabs.iter().position(|(_, c)| *c);
        let labels = tabs
            .iter()
            .enumerate()
            .map(|(i, (text, _))| format!("{}:{}", i + 1, text.trim()))
            .collect();
        (labels, current)
    }

    /// The current tab's label (e.g. `2:home`), or empty.
    pub fn current_tab(&self) -> String {
        let (labels, current) = self.tabs();
        current.map(|i| labels[i].clone()).unwrap_or_default()
    }

    /// Wait until the tab bar shows exactly these tabs (e.g. `1:home`).
    pub fn wait_for_tabs(&self, labels: &[&str]) {
        let start = Instant::now();
        while start.elapsed() < TIMEOUT {
            if self.tabs().0 == labels {
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        panic!(
            "tabs are {:?}, not {labels:?}; screen:\n{}",
            self.tabs().0,
            self.full_screen()
        );
    }

    pub fn wait_for_current_tab(&self, label: &str) {
        let start = Instant::now();
        while start.elapsed() < TIMEOUT {
            if self.current_tab() == label {
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        panic!(
            "current tab is {:?}, not {label:?}; screen:\n{}",
            self.current_tab(),
            self.screen()
        );
    }

    /// Wait until the prompt popup shows `title` in its border and `text`
    /// at the start of its line.
    pub fn wait_for_popup(&self, title: &str, text: &str) {
        let (title, line) = (format!(" {title} "), format!("│ {text}"));
        self.wait_for(&format!("popup {title:?} with {text:?}"), |_| {
            let s = self.full_screen();
            s.contains(&title) && s.contains(&line)
        });
    }

    /// Wait until the prompt popup is gone: no popup title is drawn (the
    /// dashboard preview has a rounded box too, so corners don't tell).
    pub fn wait_for_no_popup(&self) {
        self.wait_for("the popup gone", |_| {
            let s = self.full_screen();
            !POPUP_TITLES.iter().any(|t| s.contains(&format!(" {t} ")))
        });
    }

    /// `C-a d`
    pub fn detach_keys(&mut self) {
        self.send(b"\x01d");
    }

    /// What is drawn right of the sidebar: the tab bar, then the panes.
    pub fn screen(&self) -> String {
        self.rows().join("\n")
    }

    /// The whole screen, sidebar included.
    pub fn full_screen(&self) -> String {
        self.screen.lock().expect("screen lock").screen().contents()
    }

    /// Whether the cell at screen (row, col), 0-based, is drawn reversed.
    pub fn reversed(&self, row: u16, col: u16) -> bool {
        let parser = self.screen.lock().expect("screen lock");
        parser.screen().cell(row, col).is_some_and(|c| c.inverse())
    }

    /// Whether the cell at screen (row, col), 0-based, is drawn dim.
    pub fn dim(&self, row: u16, col: u16) -> bool {
        let parser = self.screen.lock().expect("screen lock");
        parser.screen().cell(row, col).is_some_and(|c| c.dim())
    }

    /// Foreground and background colour of the cell at (row, col), 0-based.
    pub fn colors(&self, row: u16, col: u16) -> (vt100::Color, vt100::Color) {
        let parser = self.screen.lock().expect("screen lock");
        let cell = parser.screen().cell(row, col).expect("cell on screen");
        (cell.fgcolor(), cell.bgcolor())
    }

    /// Rows right of the sidebar (column 0 is the session area's left
    /// edge), trimmed at the end.
    pub fn rows(&self) -> Vec<String> {
        let left = self.left();
        let parser = self.screen.lock().expect("screen lock");
        let width = parser.screen().size().1.get();
        parser
            .screen()
            .rows(left, width.saturating_sub(left))
            .map(|r| r.trim_end().to_string())
            .collect()
    }

    /// Whole rows, sidebar included, trimmed at the end.
    pub fn full_rows(&self) -> Vec<String> {
        let parser = self.screen.lock().expect("screen lock");
        let width = parser.screen().size().1.get();
        parser
            .screen()
            .rows(0, width)
            .map(|r| r.trim_end().to_string())
            .collect()
    }

    /// Wait until `pred` holds for the drawn screen; panic with the screen if not.
    pub fn wait_for(&self, what: &str, pred: impl Fn(&str) -> bool) {
        if !self.holds_within_timeout(pred) {
            panic!(
                "timed out waiting for {what}; screen:\n{}",
                self.full_screen()
            );
        }
    }

    /// Whether `pred` comes to hold for the drawn screen within
    /// [`TIMEOUT`]: [`Self::wait_for`] for a caller that says more than
    /// the screen when it does not.
    pub fn holds_within_timeout(&self, pred: impl Fn(&str) -> bool) -> bool {
        let start = Instant::now();
        while start.elapsed() < TIMEOUT {
            if pred(&self.screen()) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        false
    }

    /// Wait for paddock to say it attached again after a server upgrade.
    /// If it does not, the failure says whether paddock still runs (a
    /// paddock waiting for an answer, or one that quit) and what the
    /// server logged as it handed over, as well as the screen.
    pub fn wait_upgraded(&mut self, sandbox: &Sandbox) {
        if self.holds_within_timeout(|s| s.contains("programs kept running")) {
            return;
        }
        let paddock = match self.child.try_wait() {
            Ok(None) => "still running".to_string(),
            Ok(Some(status)) => format!("exited: {status:?}"),
            Err(e) => format!("unknown: {e}"),
        };
        let log = std::fs::read_to_string(sandbox.state_dir().join("server.log"))
            .unwrap_or_else(|e| format!("(no server.log: {e})"));
        panic!(
            "timed out waiting for programs kept running; paddock {paddock}; \
             screen:\n{}\nserver.log:\n{log}",
            self.full_screen()
        );
    }

    /// [`Self::wait_for`], sending `poke` at each look while `pred` does
    /// not hold yet: for what only happens once the terminal writes again
    /// (tmux names a tab only then), a key whose echo is that write.
    pub fn wait_for_poking(&self, what: &str, poke: &[u8], pred: impl Fn(&str) -> bool) {
        self.wait_for(what, |screen| {
            if pred(screen) {
                return true;
            }
            let mut writer = self.writer.lock().expect("writer lock");
            writer.write_all(poke).expect("write to pty");
            writer.flush().expect("flush pty");
            false
        });
    }

    pub fn wait_for_text(&self, text: &str) {
        self.wait_for(text, |s| s.contains(text));
    }

    /// Wait for paddock to exit and return whether it exited successfully.
    pub fn wait_exit(&mut self) -> bool {
        let start = Instant::now();
        while start.elapsed() < TIMEOUT {
            if let Some(status) = self.child.try_wait().expect("try_wait") {
                return status.success();
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        panic!("paddock did not exit; screen:\n{}", self.screen());
    }
}

impl Drop for Paddock {
    fn drop(&mut self) {
        if let Ok(None) = self.child.try_wait() {
            let _ = self.child.kill();
        }
    }
}

fn nonzero(n: u16) -> std::num::NonZeroU16 {
    std::num::NonZeroU16::new(n).expect("non-zero size")
}

/// Feed everything paddock draws into the VT parser, answering cursor
/// position queries the way a real terminal would.
fn spawn_reader(
    mut reader: Box<dyn Read + Send>,
    screen: Arc<Mutex<vt100::Parser>>,
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
) {
    std::thread::spawn(move || {
        let mut buf = [0u8; 1 << 16];
        loop {
            let n = match reader.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => n,
            };
            let mut parser = screen.lock().expect("screen lock");
            parser.process(&buf[..n]);
            if buf[..n]
                .windows(CURSOR_QUERY.len())
                .any(|w| w == CURSOR_QUERY)
            {
                let (row, col) = parser.screen().cursor_position();
                let reply = format!("\x1b[{};{}R", row + 1, col + 1);
                let _ = writer
                    .lock()
                    .expect("writer lock")
                    .write_all(reply.as_bytes());
            }
        }
    });
}
