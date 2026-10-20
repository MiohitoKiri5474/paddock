//! What a tmux session holds, as paddock shows it (ADR 0006): its windows
//! (tabs) in order, and each window's panes (splits) with their place.

use ratatui::layout::Rect;

/// Separates the fields of [`FORMAT`]; never part of a name.
const SEP: char = '\x1f';

/// `list-panes -s -F` format: one line per pane, names last.
pub const FORMAT: &str = "#{window_id}\x1f#{window_active}\x1f#{automatic-rename}\x1f\
#{pane_id}\x1f#{pane_active}\x1f#{pane_left}\x1f#{pane_top}\x1f#{pane_width}\x1f#{pane_height}\x1f\
#{session_name}\x1f#{window_name}";

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
}

impl Window {
    pub fn contains(&self, pane: &str) -> bool {
        self.panes.iter().any(|p| p.id == pane)
    }
}

/// A session's name, windows in order and the current window's index.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Structure {
    pub name: String,
    pub windows: Vec<Window>,
    pub current: usize,
}

/// Parse `list-panes -s -F FORMAT` output (panes come by window, in order).
pub fn parse(out: &str) -> Structure {
    let mut s = Structure::default();
    for line in out.lines() {
        let f: Vec<&str> = line.splitn(11, SEP).collect();
        let [window, window_active, auto, pane, pane_active, x, y, w, h, session, name] = f[..]
        else {
            continue;
        };
        let num = |v: &str| v.parse::<u16>().unwrap_or(0);
        if s.windows.last().is_none_or(|l| l.id != window) {
            if window_active == "1" {
                s.current = s.windows.len();
            }
            s.windows.push(Window {
                id: window.to_string(),
                name: name.to_string(),
                auto_name: auto == "1",
                panes: Vec::new(),
                active: pane.to_string(),
            });
        }
        s.name = session.to_string();
        let Some(win) = s.windows.last_mut() else {
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
                "@1", "0", "1", "%1", "1", "0", "0", "100", "30", "api", "zsh",
            ]),
            line(&[
                "@4", "1", "0", "%2", "0", "0", "0", "50", "30", "api", "my logs",
            ]),
            line(&[
                "@4", "1", "0", "%5", "1", "51", "0", "49", "30", "api", "my logs",
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
    }

    #[test]
    fn names_may_hold_the_separator_of_nothing_else() {
        let out = line(&["@1", "1", "1", "%1", "1", "0", "0", "8", "2", "a b", "x:y"]);
        let s = parse(&out);
        assert_eq!(
            (s.name.as_str(), s.windows[0].name.as_str()),
            ("a b", "x:y")
        );
    }
}
