//! Key bindings: every paddock action by name, its default keys, and the
//! keys `[keys.commands]`, `[keys.direct]`, `[keys.sidebar]`,
//! `[keys.dashboard]` and `[keys.copy]` in `config.toml` give it instead. Dispatch, the key
//! help, `paddock keys` and the README's key tables all come from the
//! tables here.

mod key;
mod tables;

use super::prefix::Command;
use anyhow::{bail, Result};
use crossterm::event::{KeyEvent, KeyModifiers};
pub use key::Key;
use std::collections::BTreeMap;
pub use tables::{
    Binding, CopyAction, DashboardAction, SidebarAction, COMMANDS, COPY, DASHBOARD, SIDEBAR,
};

/// An action's keys in `config.toml`, by action name, for one section.
pub type Given = BTreeMap<String, Vec<String>>;

/// The `[keys.…]` sections as `config.toml` gives them. `direct` is `None`
/// while `[keys.direct]` is not written at all.
#[derive(Debug, Default)]
pub struct GivenKeys {
    pub commands: Given,
    pub direct: Option<Given>,
    pub sidebar: Given,
    pub dashboard: Given,
    pub copy: Given,
}

/// Warnings about settings that work but may surprise.
pub type Warnings = Vec<String>;

/// A `[keys.<name>]` section: its actions, and the modifiers a key pressed
/// may have on top of the key bound and still match it.
#[derive(Debug, PartialEq, Eq)]
pub struct Section<A: 'static> {
    pub name: &'static str,
    pub table: &'static [Binding<A>],
    pub ignored: KeyModifiers,
    /// Whether the section's defaults are the table's direct chords
    /// instead of its keys.
    pub defaults_are_chords: bool,
    /// Whether a default key that is the prefix is left out instead of
    /// refused: copy mode's `C-b` under a `C-b` prefix could never be
    /// pressed there, and is no mistake of the user's.
    pub drops_prefix_defaults: bool,
}

impl<A> Section<A> {
    /// The default keys of `b`.
    fn default_keys(&self, b: &Binding<A>) -> &'static [&'static str] {
        if self.defaults_are_chords {
            b.chords
        } else {
            b.keys
        }
    }
}

/// After the prefix, Shift does not matter (Shift+arrow moves focus too).
pub const COMMAND_KEYS: Section<Command> = Section {
    name: "commands",
    table: COMMANDS,
    ignored: KeyModifiers::SHIFT,
    defaults_are_chords: false,
    drops_prefix_defaults: false,
};

/// Without the prefix, a chord matches exactly: a key not bound goes to
/// the terminal untouched.
pub const DIRECT_KEYS: Section<Command> = Section {
    name: "direct",
    table: COMMANDS,
    ignored: KeyModifiers::NONE,
    defaults_are_chords: true,
    drops_prefix_defaults: false,
};

/// In the sidebar and the dashboard, Ctrl and Shift do not matter.
pub const SIDEBAR_KEYS: Section<SidebarAction> = Section {
    name: "sidebar",
    table: SIDEBAR,
    ignored: KeyModifiers::CONTROL.union(KeyModifiers::SHIFT),
    defaults_are_chords: false,
    drops_prefix_defaults: false,
};

pub const DASHBOARD_KEYS: Section<DashboardAction> = Section {
    name: "dashboard",
    table: DASHBOARD,
    ignored: KeyModifiers::CONTROL.union(KeyModifiers::SHIFT),
    defaults_are_chords: false,
    drops_prefix_defaults: false,
};

/// In copy mode, Ctrl matters (`b` and `C-b` are different moves); Shift
/// does not, as after the prefix.
pub const COPY_KEYS: Section<CopyAction> = Section {
    name: "copy",
    table: COPY,
    ignored: KeyModifiers::SHIFT,
    defaults_are_chords: false,
    drops_prefix_defaults: true,
};

/// Direct chords that take a control character from the terminal
/// (Backspace, Tab, line feed, Enter, Esc): worth a warning unless the user
/// chose them.
const CONTROL_CHARS: &[&str] = &["C-h", "C-i", "C-j", "C-m", "C-["];

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
                let keys = section.default_keys(b).iter().map(|k| {
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

    /// The keys the action named `name` has now (none if it is not here).
    fn keys_of(&self, name: &str) -> &[Key] {
        self.bound
            .iter()
            .find(|(b, _)| b.name == name)
            .map_or(&[], |(_, keys)| keys.as_slice())
    }

    /// The section with `given`'s keys instead of the defaults; `""` or
    /// none unbinds. Returns warnings about the Alt keys given too.
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
            let defaults = section.default_keys(b);
            let specs: Vec<&str> = match given.get(b.name) {
                Some(specs) => specs.iter().map(String::as_str).collect(),
                // See `Section::drops_prefix_defaults`.
                None => defaults
                    .iter()
                    .copied()
                    .filter(|d| {
                        !section.drops_prefix_defaults || Key::parse(d).ok() != Some(prefix)
                    })
                    .collect(),
            };
            let mut keys = Vec::new();
            for spec in specs.into_iter().filter(|s| !s.is_empty()) {
                let (key, warning) = bound_key(name, b.name, spec, prefix)?;
                if keys.contains(&key) {
                    bail!("[keys.{name}] {}: {spec:?} is listed twice", b.name);
                }
                keys.push(key);
                // A default is paddock's choice, not something to warn of,
                // written out (as `paddock keys` prints it) or not.
                if !defaults.iter().any(|d| Key::parse(d).ok() == Some(key)) {
                    warnings.extend(warning);
                }
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

    /// [`Self::hints`] for the actions `names` only, in that order; those
    /// without a key are left out.
    pub fn hints_for(&self, names: &[&str]) -> Vec<String> {
        names
            .iter()
            .filter_map(|n| Some(format!("{} {n}", self.keys_of(n).first()?.name())))
            .collect()
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

/// Every binding: after the prefix, without it, in the sidebar, in the
/// dashboard, in copy mode.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Keymap {
    pub commands: Keys<Command>,
    /// Commands that fire on one chord, no prefix.
    pub direct: Keys<Command>,
    pub sidebar: Keys<SidebarAction>,
    pub dashboard: Keys<DashboardAction>,
    pub copy: Keys<CopyAction>,
}

impl Keymap {
    /// The keys `config.toml` gives, the defaults for the rest, and
    /// warnings: about the Alt keys given, and, while `[keys.direct]` is
    /// not written at all, about the control characters its defaults take
    /// from the terminal (written, even empty, the user chose them).
    pub fn configured(prefix: Key, given: &GivenKeys) -> Result<(Self, Warnings)> {
        let mut warnings = Warnings::new();
        let no_direct = Given::new();
        let direct_given = given.direct.as_ref().unwrap_or(&no_direct);
        let keymap = Self {
            commands: configured_section(&COMMAND_KEYS, &given.commands, prefix, &mut warnings)?,
            direct: configured_section(&DIRECT_KEYS, direct_given, prefix, &mut warnings)?,
            sidebar: configured_section(&SIDEBAR_KEYS, &given.sidebar, prefix, &mut warnings)?,
            dashboard: configured_section(
                &DASHBOARD_KEYS,
                &given.dashboard,
                prefix,
                &mut warnings,
            )?,
            copy: configured_section(&COPY_KEYS, &given.copy, prefix, &mut warnings)?,
        };
        if given.direct.is_none() {
            warnings.extend(keymap.control_chars_warning());
        }
        Ok((keymap, warnings))
    }

    /// Each command with its keys after the prefix and its direct chords.
    pub fn commands_with_chords(
        &self,
    ) -> impl Iterator<Item = (&'static Binding<Command>, &[Key], &[Key])> {
        self.commands
            .bindings()
            .map(|(b, keys)| (b, keys, self.direct.keys_of(b.name)))
    }

    /// A warning naming the direct chords that are control characters the
    /// terminal would get otherwise (`C-h` is Backspace), if any.
    fn control_chars_warning(&self) -> Option<String> {
        let taken: Vec<String> = self
            .direct
            .bindings()
            .flat_map(|(_, keys)| keys.iter().map(Key::name))
            .filter(|name| CONTROL_CHARS.contains(&name.as_str()))
            .collect();
        (!taken.is_empty()).then(|| {
            format!(
                "[keys.direct] takes {} from the terminal",
                taken.join(" and ")
            )
        })
    }

    /// Every section as TOML.
    pub fn toml(&self) -> String {
        [
            self.commands.toml(),
            self.direct.toml(),
            self.sidebar.toml(),
            self.dashboard.toml(),
            self.copy.toml(),
        ]
        .join("\n")
    }
}

impl Default for Keymap {
    fn default() -> Self {
        Self {
            commands: Keys::defaults(&COMMAND_KEYS),
            direct: Keys::defaults(&DIRECT_KEYS),
            sidebar: Keys::defaults(&SIDEBAR_KEYS),
            dashboard: Keys::defaults(&DASHBOARD_KEYS),
            copy: Keys::defaults(&COPY_KEYS),
        }
    }
}

/// The keys of `section` as `config.toml` gives them in `given` (the
/// defaults for actions left out), as [`Keys::configured`] reads them; its
/// warnings are added to `warnings`, so a keymap's sections share one list.
fn configured_section<A: Copy>(
    section: &'static Section<A>,
    given: &Given,
    prefix: Key,
    warnings: &mut Warnings,
) -> Result<Keys<A>> {
    let (keys, more) = Keys::configured(section, given, prefix)?;
    warnings.extend(more);
    Ok(keys)
}

/// A `[keys.<section>]` table from `(action, keys)` pairs, for tests.
#[cfg(test)]
pub fn given(pairs: &[(&str, &[&str])]) -> Given {
    pairs
        .iter()
        .map(|(n, keys)| (n.to_string(), keys.iter().map(|k| k.to_string()).collect()))
        .collect()
}

#[cfg(test)]
mod tests;
