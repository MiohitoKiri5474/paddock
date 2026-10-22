//! What a tmux session holds, as paddock shows it (ADR 0006): its windows
//! (tabs) in order, and each window's panes (splits) with their place.
//! Windows marked as floats are kept apart: they are never tabs.

use ratatui::layout::Rect;

/// Separates the fields of [`FORMAT`]; never part of a name.
const SEP: char = '\x1f';

/// The tmux window option that marks a window as a float; its value is
/// the float's key, as written by [`crate::input::keymap::Key::name`].
pub const FLOAT_OPTION: &str = "@paddock-float";

/// `list-panes -s -F` format: one line per pane, names last.
pub const FORMAT: &str = "#{window_id}\x1f#{window_active}\x1f#{automatic-rename}\x1f\
#{window_zoomed_flag}\x1f#{pane_id}\x1f#{pane_active}\x1f#{pane_left}\x1f#{pane_top}\x1f#{pane_width}\x1f#{pane_height}\x1f\
#{@paddock-float}\x1f#{session_name}\x1f#{window_name}";

/// Fields per line of [`FORMAT`].
const FIELDS: usize = 13;

/// One pane: its id (`%N`) and where it sits in its window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pane {
    pub id: String,
    pub rect: Rect,
}

/// One window of a session: a tab.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Window {
    pub id: String,
    pub name: String,
    /// tmux still names it (the user has not renamed it).
    pub auto_name: bool,
    pub panes: Vec<Pane>,
    /// The focused pane's id.
    pub active: String,
    /// The focused pane fills the window (`resize-pane -Z`); the others
    /// are hidden but keep running.
    pub zoomed: bool,
}

impl Window {
    pub fn contains(&self, pane: &str) -> bool {
        self.panes.iter().any(|p| p.id == pane)
    }

    /// Whether `pane` is drawn when the window is: any of its panes, or
    /// only the focused one while it is zoomed.
    pub fn shows(&self, pane: &str) -> bool {
        self.contains(pane) && (!self.zoomed || self.active == pane)
    }
}

/// A window marked as a float: its key and the window itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FloatWindow {
    /// The float's key, as stored in [`FLOAT_OPTION`].
    pub key: String,
    /// The window, its panes and focus as for a tab.
    pub window: Window,
}

/// A session's name, its tabs in order with the current one's index, and
/// its floats.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Structure {
    pub name: String,
    /// The tabs: every window but the floats.
    pub windows: Vec<Window>,
    /// Index of the current tab in `windows`.
    pub current: usize,
    /// The windows marked as floats, in tmux's order.
    pub floats: Vec<FloatWindow>,
    /// tmux made a float its session's current window (it does when the
    /// current tab closes and the float is next); `current` is then the
    /// tab before it.
    pub float_active: bool,
}

impl Structure {
    /// Every window, the tabs first, then the floats.
    pub fn all_windows(&self) -> impl Iterator<Item = &Window> {
        self.windows
            .iter()
            .chain(self.floats.iter().map(|f| &f.window))
    }
}

/// Parse `list-panes -s -F FORMAT` output (panes come by window, in order).
pub fn parse(out: &str) -> Structure {
    let mut s = Structure::default();
    for line in out.lines() {
        let f: Vec<&str> = line.splitn(FIELDS, SEP).collect();
        let [window, window_active, auto, zoomed, pane, pane_active, x, y, w, h, float, session, name] =
            f[..]
        else {
            continue;
        };
        let num = |v: &str| v.parse::<u16>().unwrap_or(0);
        s.name = session.to_string();
        let new_window = || Window {
            id: window.to_string(),
            name: name.to_string(),
            auto_name: auto == "1",
            panes: Vec::new(),
            active: pane.to_string(),
            zoomed: zoomed == "1",
        };
        let win = if float.is_empty() {
            if s.windows.last().is_none_or(|l| l.id != window) {
                if window_active == "1" {
                    s.current = s.windows.len();
                }
                s.windows.push(new_window());
            }
            s.windows.last_mut()
        } else {
            if s.floats.last().is_none_or(|l| l.window.id != window) {
                if window_active == "1" {
                    s.float_active = true;
                    s.current = s.windows.len().saturating_sub(1);
                }
                s.floats.push(FloatWindow {
                    key: float.to_string(),
                    window: new_window(),
                });
            }
            s.floats.last_mut().map(|f| &mut f.window)
        };
        let Some(win) = win else {
            continue;
        };
        if pane_active == "1" {
            win.active = pane.to_string();
        }
        win.panes.push(Pane {
            id: pane.to_string(),
            rect: Rect::new(num(x), num(y), num(w), num(h)),
        });
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(fields: &[&str]) -> String {
        fields.join("\x1f") + "\n"
    }

    #[test]
    fn panes_group_into_windows_with_names_focus_and_places() {
        let out = [
            line(&[
                "@1", "0", "1", "0", "%1", "1", "0", "0", "100", "30", "", "api", "zsh",
            ]),
            line(&[
                "@4", "1", "0", "0", "%2", "0", "0", "0", "50", "30", "", "api", "my logs",
            ]),
            line(&[
                "@4", "1", "0", "0", "%5", "1", "51", "0", "49", "30", "", "api", "my logs",
            ]),
            "garbage\n".to_string(),
        ]
        .concat();
        let s = parse(&out);
        assert_eq!(s.name, "api");
        assert_eq!(s.current, 1);
        let ids: Vec<&str> = s.windows.iter().map(|w| w.id.as_str()).collect();
        assert_eq!(ids, ["@1", "@4"]);
        let logs = &s.windows[1];
        assert_eq!((logs.name.as_str(), logs.auto_name), ("my logs", false));
        assert!(s.windows[0].auto_name);
        assert_eq!(logs.active, "%5");
        assert_eq!(logs.panes[1].rect, Rect::new(51, 0, 49, 30));
        assert!(logs.contains("%2") && !logs.contains("%1"));
        assert!(s.floats.is_empty() && !s.float_active);
    }

    #[test]
    fn a_zoomed_window_is_marked() {
        let out = [
            line(&[
                "@1", "1", "1", "1", "%1", "0", "0", "0", "50", "30", "", "api", "zsh",
            ]),
            line(&[
                "@1", "1", "1", "1", "%2", "1", "0", "0", "100", "30", "", "api", "zsh",
            ]),
            line(&[
                "@2", "0", "1", "0", "%3", "1", "0", "0", "100", "30", "", "api", "zsh",
            ]),
        ]
        .concat();
        let s = parse(&out);
        assert!(s.windows[0].zoomed && !s.windows[1].zoomed);
        assert_eq!(s.windows[0].active, "%2");
        assert!(s.windows[0].shows("%2") && !s.windows[0].shows("%1"));
        assert!(s.windows[1].shows("%3") && !s.windows[1].shows("%2"));
    }

    #[test]
    fn names_may_hold_the_separator_of_nothing_else() {
        let out = line(&[
            "@1", "1", "1", "0", "%1", "1", "0", "0", "8", "2", "", "a b", "x:y",
        ]);
        let s = parse(&out);
        assert_eq!(
            (s.name.as_str(), s.windows[0].name.as_str()),
            ("a b", "x:y")
        );
    }

    #[test]
    fn floats_are_kept_apart_from_the_tabs() {
        let out = [
            line(&[
                "@1", "0", "1", "0", "%1", "1", "0", "0", "100", "30", "", "api", "zsh",
            ]),
            line(&[
                "@7", "0", "1", "0", "%9", "1", "0", "0", "80", "24", "g", "api", "lazygit",
            ]),
            line(&[
                "@2", "1", "1", "0", "%3", "1", "0", "0", "100", "30", "", "api", "zsh",
            ]),
        ]
        .concat();
        let s = parse(&out);
        let tabs: Vec<&str> = s.windows.iter().map(|w| w.id.as_str()).collect();
        assert_eq!(tabs, ["@1", "@2"]);
        assert_eq!(s.current, 1);
        assert_eq!(s.floats.len(), 1);
        assert_eq!(s.floats[0].key, "g");
        assert_eq!(s.floats[0].window.active, "%9");
        assert_eq!(s.floats[0].window.panes[0].rect, Rect::new(0, 0, 80, 24));
        assert!(!s.float_active);
        let all: Vec<&str> = s.all_windows().map(|w| w.id.as_str()).collect();
        assert_eq!(all, ["@1", "@2", "@7"]);
    }

    #[test]
    fn a_float_tmux_made_current_leaves_the_tab_before_it_current() {
        let out = [
            line(&[
                "@1", "0", "1", "0", "%1", "1", "0", "0", "100", "30", "", "api", "zsh",
            ]),
            line(&[
                "@2", "0", "1", "0", "%2", "1", "0", "0", "100", "30", "", "api", "zsh",
            ]),
            line(&[
                "@7", "1", "1", "0", "%9", "1", "0", "0", "80", "24", "C-g", "api", "x",
            ]),
        ]
        .concat();
        let s = parse(&out);
        assert!(s.float_active);
        assert_eq!(s.current, 1);
        assert_eq!(s.floats[0].key, "C-g");
    }
}
