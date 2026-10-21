//! Key bindings: every paddock action by name, its default keys, and the
//! keys `[keys.commands]`, `[keys.sidebar]` and `[keys.dashboard]` in
//! `config.toml` give it instead. Dispatch, the key help, `paddock keys`
//! and the README's key tables all come from the tables here.

mod key;
mod tables;

use super::prefix::Command;
use anyhow::{bail, Result};
use crossterm::event::{KeyEvent, KeyModifiers};
pub use key::Key;
use std::collections::BTreeMap;
pub use tables::{Binding, DashboardAction, SidebarAction, COMMANDS, DASHBOARD, SIDEBAR};

/// An action's keys in `config.toml`, by action name, for one section.
pub type Given = BTreeMap<String, Vec<String>>;

/// Warnings about settings that work but may surprise.
pub type Warnings = Vec<String>;

/// A `[keys.<name>]` section: its actions, and the modifiers a key pressed
/// may have on top of the key bound and still match it.
#[derive(Debug, PartialEq, Eq)]
pub struct Section<A: 'static> {
    pub name: &'static str,
    pub table: &'static [Binding<A>],
    pub ignored: KeyModifiers,
}

/// After the prefix, Shift does not matter (Shift+arrow moves focus too).
pub const COMMAND_KEYS: Section<Command> = Section {
    name: "commands",
    table: COMMANDS,
    ignored: KeyModifiers::SHIFT,
};

/// In the sidebar and the dashboard, Ctrl and Shift do not matter.
pub const SIDEBAR_KEYS: Section<SidebarAction> = Section {
    name: "sidebar",
    table: SIDEBAR,
    ignored: KeyModifiers::CONTROL.union(KeyModifiers::SHIFT),
};

pub const DASHBOARD_KEYS: Section<DashboardAction> = Section {
    name: "dashboard",
    table: DASHBOARD,
    ignored: KeyModifiers::CONTROL.union(KeyModifiers::SHIFT),
};

/// The keys of one section: each binding with the keys it has now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Keys<A: 'static> {
    section: &'static Section<A>,
    bound: Vec<(&'static Binding<A>, Vec<Key>)>,
}

impl<A: Copy> Keys<A> {
    /// The section's default keys.
    pub fn defaults(section: &'static Section<A>) -> Self {
        let bound = section
            .table
            .iter()
            .map(|b| {
                let keys = b.keys.iter().map(|k| {
                    Key::parse(k).unwrap_or_else(|e| panic!("default key of {}: {e}", b.name))
                });
                (b, keys.collect())
            })
            .collect();
        Self { section, bound }
    }

    /// What `event` does, and which of the action's keys it is (`go_to_tab`
    /// goes to the tab of that number): the key exactly as bound, else
    /// without the section's ignored modifiers.
    pub fn get(&self, event: &KeyEvent) -> Option<(A, usize)> {
        self.reached_by(Key::of(event)).map(|(b, i)| (b.action, i))
    }

    /// The binding pressing `key` reaches, and which of its keys that is:
    /// the key exactly as bound, else without the section's ignored
    /// modifiers.
    pub fn reached_by(&self, key: Key) -> Option<(&'static Binding<A>, usize)> {
        key.lookup(self.section.ignored, |k| self.find(k))
    }

    /// The modifiers a key pressed may have on top of the key bound.
    pub fn ignored(&self) -> KeyModifiers {
        self.section.ignored
    }

    /// What `event` does, if anything.
    pub fn action(&self, event: &KeyEvent) -> Option<A> {
        self.get(event).map(|(action, _)| action)
    }

    fn find(&self, key: Key) -> Option<(&'static Binding<A>, usize)> {
        self.bound.iter().find_map(|(b, keys)| {
            let i = keys.iter().position(|k| *k == key)?;
            Some((*b, i))
        })
    }

    /// Each binding with its keys now, in table order.
    pub fn bindings(&self) -> impl Iterator<Item = (&'static Binding<A>, &[Key])> {
        self.bound.iter().map(|(b, keys)| (*b, keys.as_slice()))
    }

    /// The section with `given`'s keys instead of the defaults; `""` or
    /// none unbinds. Returns warnings about Alt keys too.
    pub fn configured(
        section: &'static Section<A>,
        given: &Given,
        prefix: Key,
    ) -> Result<(Self, Warnings)> {
        let name = section.name;
        let table = section.table;
        if let Some(action) = given.keys().find(|n| !table.iter().any(|b| b.name == *n)) {
            let names: Vec<&str> = table.iter().map(|b| b.name).collect();
            bail!(
                "[keys.{name}] {action}: unknown action (the actions are {})",
                names.join(", ")
            );
        }
        let mut warnings = Warnings::new();
        let mut bound = Vec::new();
        for b in table {
            let specs: Vec<&str> = match given.get(b.name) {
                Some(specs) => specs.iter().map(String::as_str).collect(),
                None => b.keys.to_vec(),
            };
            let mut keys = Vec::new();
            for spec in specs.into_iter().filter(|s| !s.is_empty()) {
                let (key, warning) = bound_key(name, b.name, spec, prefix)?;
                if keys.contains(&key) {
                    bail!("[keys.{name}] {}: {spec:?} is listed twice", b.name);
                }
                keys.push(key);
                warnings.extend(warning);
            }
            bound.push((b, keys));
        }
        let configured = Self { section, bound };
        configured.check_unique(given)?;
        Ok((configured, warnings))
    }

    /// Fail if a key is bound to two actions.
    fn check_unique(&self, given: &Given) -> Result<()> {
        let all: Vec<(Key, &str)> = self
            .bindings()
            .flat_map(|(b, keys)| keys.iter().map(|k| (*k, b.name)))
            .collect();
        let origin = |name: &str| {
            if given.contains_key(name) {
                ""
            } else {
                " (default)"
            }
        };
        for (i, (key, a)) in all.iter().enumerate() {
            if let Some((_, b)) = all[i + 1..].iter().find(|(k, _)| k == key) {
                bail!(
                    "[keys.{}] {:?} is bound to both {a}{} and {b}{}; \
                     unbind one, e.g. {b} = \"\"",
                    self.section.name,
                    key.name(),
                    origin(a),
                    origin(b)
                );
            }
        }
        Ok(())
    }

    /// `[keys.<section>]` as TOML, with the help as comments.
    pub fn toml(&self) -> String {
        let mut out = format!("[keys.{}]\n", self.section.name);
        for (b, keys) in self.bindings() {
            let names: Vec<toml::Value> =
                keys.iter().map(|k| toml::Value::String(k.name())).collect();
            let value = match names.as_slice() {
                [] => "\"\"".to_string(),
                [one] => one.to_string(),
                _ => toml::Value::Array(names).to_string(),
            };
            let line = format!("{} = {value}", b.name);
            out.push_str(&format!("{line:<40} # {}\n", b.help));
        }
        out
    }

    /// A Markdown table of the keys, as the README shows them.
    #[cfg(test)]
    pub fn markdown(&self) -> String {
        let mut out = String::from("| Key | Action | Name |\n|---|---|---|\n");
        for (b, keys) in self.bindings().filter(|(_, k)| !k.is_empty()) {
            let keys: Vec<String> = keys
                .iter()
                .map(|k| format!("`{}`", k.name().replace('|', "\\|")))
                .collect();
            out.push_str(&format!(
                "| {} | {} | `{}` |\n",
                keys.join(" "),
                b.help,
                b.name
            ));
        }
        out
    }

    /// The keys as help: `j down`, `k up`, ..., with each action's first key.
    pub fn hints(&self) -> Vec<String> {
        self.bindings()
            .filter_map(|(b, keys)| Some(format!("{} {}", keys.first()?.name(), b.name)))
            .collect()
    }
}

/// `spec` parsed as a key of `action` in `[keys.<section>]`, with a
/// warning if it uses Alt.
fn bound_key(
    section: &str,
    action: &str,
    spec: &str,
    prefix: Key,
) -> Result<(Key, Option<String>)> {
    let key = Key::parse(spec).map_err(|e| anyhow::anyhow!("[keys.{section}] {action}: {e}"))?;
    if key == prefix {
        bail!(
            "[keys.{section}] {action}: {spec:?} is the prefix key, which always starts a \
             command (pressed twice, it goes to the terminal)"
        );
    }
    let warning = key.uses_alt().then(|| {
        format!(
            "[keys.{section}] {action}: {spec:?} uses Alt, which not every terminal sends; \
             paddock's own keys avoid it"
        )
    });
    Ok((key, warning))
}

/// How a binding's keys read in the help: `h Left`, or `1...9` for many.
pub fn label(keys: &[Key]) -> String {
    match keys {
        [first, .., last] if keys.len() > 3 => format!("{}...{}", first.name(), last.name()),
        _ => keys.iter().map(Key::name).collect::<Vec<_>>().join(" "),
    }
}

/// Every binding: after the prefix, in the sidebar, in the dashboard.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Keymap {
    pub commands: Keys<Command>,
    pub sidebar: Keys<SidebarAction>,
    pub dashboard: Keys<DashboardAction>,
}

impl Keymap {
    /// The keys `config.toml` gives, the defaults for the rest, and
    /// warnings about them.
    pub fn configured(
        prefix: Key,
        [commands, sidebar, dashboard]: [&Given; 3],
    ) -> Result<(Self, Warnings)> {
        let (commands, mut warnings) = Keys::configured(&COMMAND_KEYS, commands, prefix)?;
        let (sidebar, more) = Keys::configured(&SIDEBAR_KEYS, sidebar, prefix)?;
        warnings.extend(more);
        let (dashboard, more) = Keys::configured(&DASHBOARD_KEYS, dashboard, prefix)?;
        warnings.extend(more);
        let keymap = Self {
            commands,
            sidebar,
            dashboard,
        };
        Ok((keymap, warnings))
    }

    /// Every section as TOML.
    pub fn toml(&self) -> String {
        [
            self.commands.toml(),
            self.sidebar.toml(),
            self.dashboard.toml(),
        ]
        .join("\n")
    }
}

impl Default for Keymap {
    fn default() -> Self {
        Self {
            commands: Keys::defaults(&COMMAND_KEYS),
            sidebar: Keys::defaults(&SIDEBAR_KEYS),
            dashboard: Keys::defaults(&DASHBOARD_KEYS),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyCode;

    fn key(code: KeyCode, mods: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, mods)
    }

    fn plain(c: char) -> KeyEvent {
        key(KeyCode::Char(c), KeyModifiers::NONE)
    }

    fn ctrl_a() -> Key {
        Key::parse("C-a").unwrap()
    }

    fn given(pairs: &[(&str, &[&str])]) -> Given {
        pairs
            .iter()
            .map(|(n, keys)| (n.to_string(), keys.iter().map(|k| k.to_string()).collect()))
            .collect()
    }

    fn commands(pairs: &[(&str, &[&str])]) -> Result<Keys<Command>> {
        Ok(Keys::configured(&COMMAND_KEYS, &given(pairs), ctrl_a())?.0)
    }

    fn err(result: Result<impl std::fmt::Debug>) -> String {
        result.unwrap_err().to_string()
    }

    #[test]
    fn names_parse_back_to_the_same_key() {
        let keymap = Keymap::default();
        let all = keymap
            .commands
            .bindings()
            .map(|(_, k)| k)
            .chain(keymap.sidebar.bindings().map(|(_, k)| k))
            .chain(keymap.dashboard.bindings().map(|(_, k)| k));
        for k in all.flatten() {
            assert_eq!(Key::parse(&k.name()).unwrap(), *k, "{}", k.name());
        }
        for spec in ["C-a", "S-Tab", "M-x", "F5", "C-S-Left", "Space"] {
            assert_eq!(Key::parse(spec).unwrap().name(), spec);
        }
    }

    #[test]
    fn every_default_key_parses_and_every_name_is_unique() {
        fn check<A: Copy>(section: &'static Section<A>) {
            let keys = Keys::defaults(section);
            for ((b, parsed), i) in keys.bindings().zip(0..) {
                assert_eq!(parsed.len(), b.keys.len(), "{}", b.name);
                let later = &section.table[i + 1..];
                assert!(later.iter().all(|o| o.name != b.name), "{}", b.name);
            }
        }
        check(&COMMAND_KEYS);
        check(&SIDEBAR_KEYS);
        check(&DASHBOARD_KEYS);
        // Valid together: no key twice, none the default prefix, no warning.
        let (defaults, warnings) = Keymap::configured(ctrl_a(), [&Given::new(); 3]).unwrap();
        assert_eq!(defaults, Keymap::default());
        assert!(warnings.is_empty());
    }

    #[test]
    fn go_to_tab_reports_which_of_its_keys_was_pressed() {
        let keys = Keys::defaults(&COMMAND_KEYS);
        assert_eq!(keys.get(&plain('1')), Some((Command::GoToTab(0), 0)));
        assert_eq!(keys.get(&plain('9')).map(|(_, i)| i), Some(8));
        assert_eq!(keys.get(&plain('0')), None);
        let f = commands(&[("go_to_tab", &["F1", "F2"])]).unwrap();
        let f2 = f.get(&key(KeyCode::F(2), KeyModifiers::NONE));
        assert_eq!(f2.map(|(_, i)| i), Some(1));
    }

    #[test]
    fn shift_does_not_matter_after_the_prefix_but_ctrl_does() {
        let keys = Keys::defaults(&COMMAND_KEYS);
        let left = keys.action(&key(KeyCode::Left, KeyModifiers::SHIFT));
        assert_eq!(left, Some(Command::Focus(crate::input::Direction::Left)));
        let ctrl_c = keys.action(&key(KeyCode::Char('c'), KeyModifiers::CONTROL));
        assert_eq!(ctrl_c, None);
        // A key bound with Shift still wins over its plain one.
        let both = commands(&[("previous_tab", &["S-Left"])]).unwrap();
        let shifted = both.action(&key(KeyCode::Left, KeyModifiers::SHIFT));
        assert_eq!(shifted, Some(Command::PreviousTab));
    }

    #[test]
    fn ctrl_and_shift_do_not_matter_in_the_sidebar_and_dashboard() {
        let sidebar = Keys::defaults(&SIDEBAR_KEYS);
        let ctrl_j = key(KeyCode::Char('j'), KeyModifiers::CONTROL);
        assert_eq!(sidebar.action(&ctrl_j), Some(SidebarAction::Down));
        let shift_down = key(KeyCode::Down, KeyModifiers::SHIFT);
        assert_eq!(sidebar.action(&shift_down), Some(SidebarAction::Down));
        let dashboard = Keys::defaults(&DASHBOARD_KEYS);
        let ctrl_enter = key(KeyCode::Enter, KeyModifiers::CONTROL);
        assert_eq!(dashboard.action(&ctrl_enter), Some(DashboardAction::Open));
        let alt_j = key(KeyCode::Char('j'), KeyModifiers::ALT);
        assert_eq!(dashboard.action(&alt_j), None);
    }

    #[test]
    fn a_remapped_action_leaves_its_old_key_unbound() {
        let keys = commands(&[("new_tab", &["t"]), ("split_right", &["\\", "|"])]).unwrap();
        assert_eq!(keys.action(&plain('t')), Some(Command::NewTab));
        assert_eq!(keys.action(&plain('c')), None);
        assert_eq!(keys.action(&plain('\\')), Some(Command::SplitColumns));
        assert_eq!(keys.action(&plain('|')), Some(Command::SplitColumns));
    }

    #[test]
    fn an_empty_string_or_list_unbinds() {
        for unbound in [&[""][..], &[]] {
            let keys = commands(&[("detach", unbound)]).unwrap();
            assert_eq!(keys.action(&plain('d')), None);
        }
    }

    #[test]
    fn an_unknown_action_is_named_with_its_section() {
        let e = err(commands(&[("new_tabs", &["t"])]));
        assert!(
            e.contains("[keys.commands] new_tabs: unknown action"),
            "{e}"
        );
        assert!(e.contains("new_tab, new_space"), "{e}");
    }

    #[test]
    fn an_unreadable_key_is_named_with_its_action() {
        let e = err(commands(&[("new_tab", &["Ctrl+t"])]));
        assert!(
            e.contains("[keys.commands] new_tab: invalid key \"Ctrl+t\""),
            "{e}"
        );
        let e = err(commands(&[("new_tab", &["S-1"])]));
        assert!(
            e.contains("[keys.commands] new_tab: invalid key \"S-1\""),
            "{e}"
        );
    }

    #[test]
    fn a_key_bound_twice_names_both_actions() {
        let e = err(commands(&[("new_tab", &["n"])]));
        assert!(
            e.contains("\"n\" is bound to both new_tab and next_tab (default)"),
            "{e}"
        );
        assert!(e.contains("next_tab = \"\""), "{e}");
        assert!(commands(&[("new_tab", &["n"]), ("next_tab", &[])]).is_ok());
        let sidebar = given(&[("fork", &["x"])]);
        let e = err(Keys::configured(&SIDEBAR_KEYS, &sidebar, ctrl_a()));
        assert!(e.contains("[keys.sidebar] \"x\""), "{e}");
    }

    #[test]
    fn a_key_listed_twice_for_one_action_is_an_error() {
        let e = err(commands(&[("new_tab", &["n", "C-n", "n"])]));
        assert!(
            e.contains("[keys.commands] new_tab: \"n\" is listed twice"),
            "{e}"
        );
    }

    #[test]
    fn the_prefix_key_cannot_be_bound_however_it_is_written() {
        for spec in ["C-a", "c-a", "C-A"] {
            let e = err(commands(&[("new_tab", &[spec])]));
            assert!(e.contains("is the prefix key"), "{spec}: {e}");
        }
        let other = Key::parse("C-b").unwrap();
        let ok = Keys::configured(&COMMAND_KEYS, &given(&[("new_tab", &["C-a"])]), other);
        assert!(ok.is_ok());
    }

    #[test]
    fn alt_keys_work_with_a_warning() {
        let (keys, warnings) =
            Keys::configured(&COMMAND_KEYS, &given(&[("new_tab", &["M-t"])]), ctrl_a()).unwrap();
        let alt_t = key(KeyCode::Char('t'), KeyModifiers::ALT);
        assert_eq!(keys.action(&alt_t), Some(Command::NewTab));
        assert_eq!(warnings.len(), 1);
        assert!(
            warnings[0].contains("new_tab: \"M-t\" uses Alt"),
            "{}",
            warnings[0]
        );
    }

    #[test]
    fn toml_quotes_and_lists_keys() {
        let toml = Keymap::default().toml();
        assert!(toml.contains("[keys.commands]\nnew_tab = \"c\""), "{toml}");
        assert!(toml.contains("focus_left = [\"h\", \"Left\"]"), "{toml}");
        let unbound = commands(&[("detach", &[])]).unwrap().toml();
        assert!(unbound.contains("detach = \"\""), "{unbound}");
        let quoted = commands(&[("help", &["\"", "\\"])]).unwrap().toml();
        let back: toml::Table = toml::from_str(&quoted).unwrap();
        assert_eq!(
            back["keys"]["commands"]["help"],
            toml::Value::Array(vec!["\"".into(), "\\".into()])
        );
    }

    #[test]
    fn hints_and_labels_show_the_keys() {
        let hints = Keys::defaults(&SIDEBAR_KEYS).hints().join("  ");
        assert_eq!(
            hints,
            "j down  k up  Enter open  x close  f fork  W new_worktree  o open_worktree  Esc back"
        );
        let defaults = Keys::defaults(&COMMAND_KEYS);
        let labels: Vec<String> = defaults.bindings().map(|(_, k)| label(k)).collect();
        assert!(labels.contains(&"1...9".to_string()));
        assert!(labels.contains(&"h Left".to_string()));
    }

    #[test]
    fn the_readme_lists_the_default_keys() {
        let readme = include_str!("../../README.md");
        let keymap = Keymap::default();
        for table in [
            keymap.commands.markdown(),
            keymap.sidebar.markdown(),
            keymap.dashboard.markdown(),
        ] {
            assert!(readme.contains(&table), "README is missing:\n{table}");
        }
    }
}
