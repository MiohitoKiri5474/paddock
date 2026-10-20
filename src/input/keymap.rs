//! Key bindings: every paddock action by name, its default keys, and the
//! keys `[keys.commands]`, `[keys.sidebar]` and `[keys.dashboard]` in
//! `config.toml` give it instead. Dispatch, the key help, `paddock keys`
//! and the README's key tables all come from the tables here.

mod key;
mod tables;

use super::prefix::Command;
use anyhow::{bail, Result};
use crossterm::event::KeyEvent;
pub use key::Key;
use std::collections::BTreeMap;
pub use tables::{Action, Binding, Panel, COMMANDS, DASHBOARD, SIDEBAR};

/// The keys of one table: for each binding, the keys it has now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Keys<A: 'static> {
    table: &'static [Binding<A>],
    keys: Vec<Vec<Key>>,
}

impl<A: Action> Keys<A> {
    /// The table's default keys.
    pub fn defaults(table: &'static [Binding<A>]) -> Self {
        let keys = table
            .iter()
            .map(|b| b.keys.iter().filter_map(|k| Key::parse(k).ok()).collect())
            .collect();
        Self { table, keys }
    }

    /// What `event` does, if anything.
    pub fn get(&self, event: &KeyEvent) -> Option<A> {
        let key = Key::of(event);
        self.bindings().find_map(|(b, keys)| {
            let i = keys.iter().position(|k| *k == key)?;
            Some(b.action.nth(i))
        })
    }

    /// Each binding with its keys now, in table order.
    pub fn bindings(&self) -> impl Iterator<Item = (&'static Binding<A>, &[Key])> {
        self.table.iter().zip(self.keys.iter().map(Vec::as_slice))
    }

    /// The table with `[keys.<section>]`'s keys (`given`, by action name)
    /// instead of the defaults; `""` or none unbinds. Alt keys add a
    /// warning to `warnings`.
    pub fn configured(
        section: &str,
        table: &'static [Binding<A>],
        given: &BTreeMap<String, Vec<String>>,
        prefix: Key,
        warnings: &mut Vec<String>,
    ) -> Result<Self> {
        if let Some(name) = given.keys().find(|n| !table.iter().any(|b| b.name == *n)) {
            let names: Vec<&str> = table.iter().map(|b| b.name).collect();
            bail!(
                "[keys.{section}] {name}: unknown action (the actions are {})",
                names.join(", ")
            );
        }
        let mut keys = Vec::new();
        for b in table {
            let specs: Vec<&str> = match given.get(b.name) {
                Some(specs) => specs.iter().map(String::as_str).collect(),
                None => b.keys.to_vec(),
            };
            let mut bound = Vec::new();
            for spec in specs.into_iter().filter(|s| !s.is_empty()) {
                bound.push(bound_key(section, b.name, spec, prefix, warnings)?);
            }
            keys.push(bound);
        }
        let configured = Self { table, keys };
        configured.check_unique(section, given)?;
        Ok(configured)
    }

    /// Fail if a key is bound to two actions.
    fn check_unique(&self, section: &str, given: &BTreeMap<String, Vec<String>>) -> Result<()> {
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
            if let Some((_, b)) = all[i + 1..].iter().find(|(k, b)| k == key && b != a) {
                bail!(
                    "[keys.{section}] {:?} is bound to both {a}{} and {b}{}; \
                     unbind one, e.g. {b} = \"\"",
                    key.name(),
                    origin(a),
                    origin(b)
                );
            }
        }
        Ok(())
    }

    /// `[keys.<section>]` as TOML, with the help as comments.
    pub fn toml(&self, section: &str) -> String {
        let mut out = format!("[keys.{section}]\n");
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

    /// The keys as one line of the help: `j down  k up  Enter open ...`,
    /// with each action's first key.
    pub fn hint(&self) -> String {
        let pairs: Vec<String> = self
            .bindings()
            .filter_map(|(b, keys)| Some(format!("{} {}", keys.first()?.name(), b.name)))
            .collect();
        pairs.join("  ")
    }
}

/// `spec` parsed as a key of action `name` in `[keys.<section>]`.
fn bound_key(
    section: &str,
    name: &str,
    spec: &str,
    prefix: Key,
    warnings: &mut Vec<String>,
) -> Result<Key> {
    let key = Key::parse(spec).map_err(|e| anyhow::anyhow!("[keys.{section}] {name}: {e}"))?;
    if key == prefix {
        bail!(
            "[keys.{section}] {name}: {spec:?} is the prefix key, which always starts a \
             command (pressed twice, it goes to the terminal)"
        );
    }
    if key.uses_alt() {
        warnings.push(format!(
            "[keys.{section}] {name}: {spec:?} uses Alt, which not every terminal sends; \
             paddock's own keys avoid it"
        ));
    }
    Ok(key)
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
    pub sidebar: Keys<Panel>,
    pub dashboard: Keys<Panel>,
}

/// An action's keys in `config.toml`, by section and action name.
pub type Given = BTreeMap<String, Vec<String>>;

impl Keymap {
    /// The keys `config.toml` gives, the defaults for the rest.
    pub fn configured(
        prefix: Key,
        [commands, sidebar, dashboard]: [&Given; 3],
        warnings: &mut Vec<String>,
    ) -> Result<Self> {
        Ok(Self {
            commands: Keys::configured("commands", COMMANDS, commands, prefix, warnings)?,
            sidebar: Keys::configured("sidebar", SIDEBAR, sidebar, prefix, warnings)?,
            dashboard: Keys::configured("dashboard", DASHBOARD, dashboard, prefix, warnings)?,
        })
    }

    /// Every section as TOML.
    pub fn toml(&self) -> String {
        [
            self.commands.toml("commands"),
            self.sidebar.toml("sidebar"),
            self.dashboard.toml("dashboard"),
        ]
        .join("\n")
    }
}

impl Default for Keymap {
    fn default() -> Self {
        Self {
            commands: Keys::defaults(COMMANDS),
            sidebar: Keys::defaults(SIDEBAR),
            dashboard: Keys::defaults(DASHBOARD),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyModifiers};

    fn key(code: KeyCode, mods: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, mods)
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
        Keys::configured("commands", COMMANDS, &given(pairs), ctrl_a(), &mut vec![])
    }

    #[test]
    fn names_parse_back_to_the_same_key() {
        for keys in [
            Keymap::default().commands.keys,
            Keymap::default().sidebar.keys,
            Keymap::default().dashboard.keys,
        ] {
            for k in keys.into_iter().flatten() {
                assert_eq!(Key::parse(&k.name()).unwrap(), k, "{}", k.name());
            }
        }
        for spec in ["C-a", "S-Tab", "M-x", "F5", "C-S-Left", "Space"] {
            assert_eq!(Key::parse(spec).unwrap().name(), spec);
        }
    }

    #[test]
    fn every_default_key_parses_and_every_name_is_unique() {
        fn check<A: Action>(table: &'static [Binding<A>]) {
            let keys = Keys::defaults(table);
            for ((b, parsed), i) in keys.bindings().zip(0..) {
                assert_eq!(parsed.len(), b.keys.len(), "{}", b.name);
                assert!(
                    table[i + 1..].iter().all(|o| o.name != b.name),
                    "{}",
                    b.name
                );
            }
        }
        check(COMMANDS);
        check(SIDEBAR);
        check(DASHBOARD);
        // Valid together: no key twice, none the default prefix.
        let defaults = Keymap::configured(ctrl_a(), [&Given::new(); 3], &mut vec![]);
        assert_eq!(defaults.unwrap(), Keymap::default());
    }

    #[test]
    fn go_to_tab_goes_to_the_tab_of_its_key() {
        let keys = Keys::defaults(COMMANDS);
        let digit = |c| keys.get(&key(KeyCode::Char(c), KeyModifiers::NONE));
        assert_eq!(digit('1'), Some(Command::GoToTab(0)));
        assert_eq!(digit('9'), Some(Command::GoToTab(8)));
        assert_eq!(digit('0'), None);
        let f = commands(&[("go_to_tab", &["F1", "F2"])]).unwrap();
        assert_eq!(
            f.get(&key(KeyCode::F(2), KeyModifiers::NONE)),
            Some(Command::GoToTab(1))
        );
    }

    #[test]
    fn a_remapped_action_leaves_its_old_key_unbound() {
        let keys = commands(&[("new_tab", &["t"]), ("split_right", &["\\", "|"])]).unwrap();
        let get = |c| keys.get(&key(KeyCode::Char(c), KeyModifiers::NONE));
        assert_eq!(get('t'), Some(Command::NewTab));
        assert_eq!(get('c'), None);
        assert_eq!(get('\\'), Some(Command::SplitColumns));
        assert_eq!(get('|'), Some(Command::SplitColumns));
    }

    #[test]
    fn an_empty_string_or_list_unbinds() {
        for unbound in [&[""][..], &[]] {
            let keys = commands(&[("detach", unbound)]).unwrap();
            assert_eq!(keys.get(&key(KeyCode::Char('d'), KeyModifiers::NONE)), None);
        }
    }

    #[test]
    fn an_unknown_action_is_named_with_its_section() {
        let err = commands(&[("new_tabs", &["t"])]).unwrap_err().to_string();
        assert!(
            err.contains("[keys.commands] new_tabs: unknown action"),
            "{err}"
        );
        assert!(err.contains("new_tab, new_space"), "{err}");
    }

    #[test]
    fn an_unreadable_key_is_named_with_its_action() {
        let err = commands(&[("new_tab", &["Ctrl+t"])])
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("[keys.commands] new_tab: invalid key \"Ctrl+t\""),
            "{err}"
        );
    }

    #[test]
    fn a_key_bound_twice_names_both_actions() {
        let err = commands(&[("new_tab", &["n"])]).unwrap_err().to_string();
        assert!(
            err.contains("\"n\" is bound to both new_tab and next_tab (default)"),
            "{err}"
        );
        assert!(err.contains("next_tab = \"\""), "{err}");
        assert!(commands(&[("new_tab", &["n"]), ("next_tab", &[])]).is_ok());
        let sidebar = given(&[("fork", &["x"])]);
        let err = Keys::configured("sidebar", SIDEBAR, &sidebar, ctrl_a(), &mut vec![])
            .unwrap_err()
            .to_string();
        assert!(err.contains("[keys.sidebar] \"x\""), "{err}");
    }

    #[test]
    fn the_prefix_key_cannot_be_bound() {
        let err = commands(&[("new_tab", &["C-a"])]).unwrap_err().to_string();
        assert!(
            err.contains("[keys.commands] new_tab: \"C-a\" is the prefix key"),
            "{err}"
        );
        let other_prefix = Key::parse("C-b").unwrap();
        let ok = Keys::configured(
            "commands",
            COMMANDS,
            &given(&[("new_tab", &["C-a"])]),
            other_prefix,
            &mut vec![],
        );
        assert!(ok.is_ok());
    }

    #[test]
    fn alt_keys_work_with_a_warning() {
        let mut warnings = vec![];
        let keys = Keys::configured(
            "commands",
            COMMANDS,
            &given(&[("new_tab", &["M-t"])]),
            ctrl_a(),
            &mut warnings,
        )
        .unwrap();
        assert_eq!(
            keys.get(&key(KeyCode::Char('t'), KeyModifiers::ALT)),
            Some(Command::NewTab)
        );
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
        let unbound = commands(&[("detach", &[])]).unwrap().toml("commands");
        assert!(unbound.contains("detach = \"\""), "{unbound}");
        let quoted = commands(&[("help", &["\"", "\\"])])
            .unwrap()
            .toml("commands");
        let back: toml::Table = toml::from_str(&quoted).unwrap();
        assert_eq!(
            back["keys"]["commands"]["help"],
            toml::Value::Array(vec!["\"".into(), "\\".into()])
        );
    }

    #[test]
    fn hints_and_labels_show_the_keys() {
        assert_eq!(
            Keys::defaults(SIDEBAR).hint(),
            "j down  k up  Enter open  x close  f fork  Esc back"
        );
        let defaults = Keys::defaults(COMMANDS);
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
