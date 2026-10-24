//! A tmux session's windows and panes (ADR 0006), read into the
//! [`Structure`] paddock shows: windows are tabs, panes are splits, and
//! windows marked as floats are kept apart.

use crate::backend::{Float, FloatId, Split, Structure, Tab, TabId, TerminalId};
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
        let split = Split {
            id: TerminalId::new(pane),
            rect: Rect::new(num(x), num(y), num(w), num(h)),
        };
        // A window's focused pane, as its `active` until one says it is.
        let (splits, active) = if float.is_empty() {
            if s.tabs.last().is_none_or(|l| l.id.as_str() != window) {
                if window_active == "1" {
                    s.current = s.tabs.len();
                }
                s.tabs.push(Tab {
                    id: TabId::new(window),
                    name: name.to_string(),
                    auto_name: auto == "1",
                    splits: Vec::new(),
                    active: split.id.clone(),
                    zoomed: zoomed == "1",
                });
            }
            let Some(t) = s.tabs.last_mut() else {
                continue;
            };
            (&mut t.splits, &mut t.active)
        } else {
            // tmux makes a float its session's current window when the
            // current tab closes and the float is next.
            if s.floats.last().is_none_or(|l| l.id.as_str() != window) {
                if window_active == "1" {
                    s.float_active = true;
                    s.current = s.tabs.len().saturating_sub(1);
                }
                s.floats.push(Float {
                    id: FloatId::new(window),
                    key: float.to_string(),
                    splits: Vec::new(),
                    active: split.id.clone(),
                });
            }
            let Some(f) = s.floats.last_mut() else {
                continue;
            };
            (&mut f.splits, &mut f.active)
        };
        if pane_active == "1" {
            *active = split.id.clone();
        }
        splits.push(split);
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One line of `list-panes -F FORMAT` output, by field: a test names
    /// only the fields it cares about, so a new field changes [`Line::text`]
    /// alone.
    #[derive(Debug, Clone, Copy)]
    struct Line {
        window: &'static str,
        window_active: bool,
        auto_name: bool,
        zoomed: bool,
        pane: &'static str,
        pane_active: bool,
        /// `(left, top, width, height)`.
        place: (u16, u16, u16, u16),
        float: &'static str,
        session: &'static str,
        name: &'static str,
    }

    impl Default for Line {
        fn default() -> Self {
            Self {
                window: "@1",
                window_active: false,
                auto_name: true,
                zoomed: false,
                pane: "%1",
                pane_active: true,
                place: (0, 0, 100, 30),
                float: "",
                session: "api",
                name: "zsh",
            }
        }
    }

    impl Line {
        /// The line as tmux writes it, in [`FORMAT`]'s order.
        fn text(&self) -> String {
            let flag = |b: bool| if b { "1" } else { "0" }.to_string();
            let (x, y, w, h) = self.place;
            let fields = [
                self.window.to_string(),
                flag(self.window_active),
                flag(self.auto_name),
                flag(self.zoomed),
                self.pane.to_string(),
                flag(self.pane_active),
                x.to_string(),
                y.to_string(),
                w.to_string(),
                h.to_string(),
                self.float.to_string(),
                self.session.to_string(),
                self.name.to_string(),
            ];
            assert_eq!(fields.len(), FIELDS);
            fields.join("\x1f") + "\n"
        }
    }

    /// `lines` as one `list-panes` output.
    fn out(lines: &[Line]) -> String {
        lines.iter().map(Line::text).collect()
    }

    #[test]
    fn panes_group_into_windows_with_names_focus_and_places() {
        let logs = Line {
            window: "@4",
            window_active: true,
            auto_name: false,
            name: "my logs",
            ..Line::default()
        };
        let text = out(&[
            Line::default(),
            Line {
                pane: "%2",
                pane_active: false,
                place: (0, 0, 50, 30),
                ..logs
            },
            Line {
                pane: "%5",
                place: (51, 0, 49, 30),
                ..logs
            },
        ]) + "garbage\n";
        let s = parse(&text);
        assert_eq!(s.name, "api");
        assert_eq!(s.current, 1);
        let ids: Vec<&str> = s.tabs.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(ids, ["@1", "@4"]);
        let logs = &s.tabs[1];
        assert_eq!((logs.name.as_str(), logs.auto_name), ("my logs", false));
        assert!(s.tabs[0].auto_name);
        assert_eq!(logs.active, "%5");
        assert_eq!(logs.splits[1].rect, Rect::new(51, 0, 49, 30));
        assert!(logs.contains(&"%2".into()) && !logs.contains(&"%1".into()));
        assert!(s.floats.is_empty() && !s.float_active);
    }

    #[test]
    fn a_zoomed_window_is_marked_and_shows_only_its_focused_pane() {
        let zoomed = Line {
            window_active: true,
            zoomed: true,
            ..Line::default()
        };
        let s = parse(&out(&[
            Line {
                pane_active: false,
                place: (0, 0, 50, 30),
                ..zoomed
            },
            Line {
                pane: "%2",
                ..zoomed
            },
            Line {
                window: "@2",
                pane: "%3",
                ..Line::default()
            },
        ]));
        assert!(s.tabs[0].zoomed && !s.tabs[1].zoomed);
        assert_eq!(s.tabs[0].active, "%2");
        assert!(s.tabs[0].shows(&"%2".into()) && !s.tabs[0].shows(&"%1".into()));
        assert!(s.tabs[1].shows(&"%3".into()) && !s.tabs[1].shows(&"%2".into()));
        let shown: Vec<&str> = s.tabs[0].shown_splits().map(|s| s.id.as_str()).collect();
        assert_eq!(shown, ["%2"]);
        assert_eq!(s.tabs[1].shown_splits().count(), 1);
    }

    #[test]
    fn focusing_another_pane_unzooms_as_tmux_does() {
        let zoomed = Line {
            zoomed: true,
            ..Line::default()
        };
        let text = out(&[
            Line {
                pane_active: false,
                ..zoomed
            },
            Line {
                pane: "%2",
                ..zoomed
            },
        ]);
        let mut window = parse(&text).tabs.remove(0);
        window.focus(&TerminalId::new("%2"));
        assert!(window.zoomed, "the zoomed pane itself");
        window.focus(&TerminalId::new("%1"));
        assert!(!window.zoomed);
        assert_eq!(window.active, "%1");
        let shown: Vec<&str> = window.shown_splits().map(|s| s.id.as_str()).collect();
        assert_eq!(shown, ["%1", "%2"]);
    }

    #[test]
    fn names_may_hold_the_separator_of_nothing_else() {
        let s = parse(&out(&[Line {
            session: "a b",
            name: "x:y",
            ..Line::default()
        }]));
        assert_eq!((s.name.as_str(), s.tabs[0].name.as_str()), ("a b", "x:y"));
    }

    #[test]
    fn floats_are_kept_apart_from_the_tabs() {
        let s = parse(&out(&[
            Line::default(),
            Line {
                window: "@7",
                pane: "%9",
                place: (0, 0, 80, 24),
                float: "g",
                name: "lazygit",
                ..Line::default()
            },
            Line {
                window: "@2",
                window_active: true,
                pane: "%3",
                ..Line::default()
            },
        ]));
        let tabs: Vec<&str> = s.tabs.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(tabs, ["@1", "@2"]);
        assert_eq!(s.current, 1);
        assert_eq!(s.floats.len(), 1);
        assert_eq!(s.floats[0].key, "g");
        assert_eq!(s.floats[0].id, "@7");
        assert_eq!(s.floats[0].active, "%9");
        assert_eq!(s.floats[0].splits[0].rect, Rect::new(0, 0, 80, 24));
        assert!(!s.float_active);
        let all: Vec<&str> = s.splits().map(|s| s.id.as_str()).collect();
        assert_eq!(all, ["%1", "%3", "%9"]);
    }

    #[test]
    fn a_float_tmux_made_current_leaves_the_tab_before_it_current() {
        let s = parse(&out(&[
            Line::default(),
            Line {
                window: "@2",
                pane: "%2",
                ..Line::default()
            },
            Line {
                window: "@7",
                window_active: true,
                pane: "%9",
                place: (0, 0, 80, 24),
                float: "C-g",
                name: "x",
                ..Line::default()
            },
        ]));
        assert!(s.float_active);
        assert_eq!(s.current, 1);
        assert_eq!(s.floats[0].key, "C-g");
    }
}
