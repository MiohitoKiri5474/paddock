//! `[floats]` in `config.toml`: programs shown in a centred box over the
//! screen, each toggled by the prefix and its key.
//!
//! ```toml
//! [floats]
//! width = 80        # percent of the screen
//! height = 80
//! g = "lazygit"     # the default; "" removes it
//! y = "yazi"
//! ```

use crate::input::keymap::{Key, Keys};
use crate::input::Command;
use anyhow::{bail, Result};
use crossterm::event::KeyModifiers;

/// The box's width and height, in percent of the screen, by default.
pub const DEFAULT_SIZE: u16 = 80;
/// The largest width or height: the whole screen.
const MAX_PERCENT: i64 = 100;
/// The floats every config starts with, as `(key, command)`.
const DEFAULTS: &[(&str, &str)] = &[("g", "lazygit")];
/// The `[floats]` setting for the box's width, which is not a key.
const WIDTH: &str = "width";
/// The `[floats]` setting for the box's height, which is not a key.
const HEIGHT: &str = "height";

/// One float: the key after the prefix that shows or hides it, and the
/// command it runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Float {
    pub key: Key,
    pub command: String,
}

/// Every float, and the size of the box they show in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Floats {
    /// The floats, defaults first, then in the order of their keys.
    pub list: Vec<Float>,
    /// Percent of the screen's width the box takes.
    pub width: u16,
    /// Percent of the screen's height the box takes.
    pub height: u16,
}

impl Default for Floats {
    fn default() -> Self {
        let list = DEFAULTS
            .iter()
            .map(|(key, command)| Float {
                key: Key::parse(key).unwrap_or_else(|e| panic!("default float {key}: {e}")),
                command: (*command).to_string(),
            })
            .collect();
        Self {
            list,
            width: DEFAULT_SIZE,
            height: DEFAULT_SIZE,
        }
    }
}

impl Floats {
    /// The defaults changed by `table` (`[floats]` as written): a key
    /// with a command adds or replaces a float, with `""` removes it.
    /// A key that is the prefix, or is bound in `commands`, is an error.
    pub fn configured(table: &toml::Table, prefix: Key, commands: &Keys<Command>) -> Result<Self> {
        let mut floats = Self::default();
        let mut given: Vec<Key> = Vec::new();
        for (spec, value) in table {
            match spec.as_str() {
                WIDTH => floats.width = percent(WIDTH, value)?,
                HEIGHT => floats.height = percent(HEIGHT, value)?,
                _ => {
                    let key = float_key(spec, prefix, &given)?;
                    given.push(key);
                    let Some(command) = value.as_str() else {
                        bail!("[floats] {spec}: expected a command in quotes, or \"\"");
                    };
                    floats.set(key, command);
                }
            }
        }
        floats.check_commands(commands)?;
        Ok(floats)
    }

    /// Add the float on `key`, replace its command, or with `""` remove it.
    fn set(&mut self, key: Key, command: &str) {
        let at = self.list.iter().position(|f| f.key == key);
        match (at, command.trim()) {
            (Some(i), "") => {
                self.list.remove(i);
            }
            (None, "") => {}
            (Some(i), command) => self.list[i].command = command.to_string(),
            (None, command) => self.list.push(Float {
                key,
                command: command.to_string(),
            }),
        }
    }

    /// Fail if a float's key also runs a command after the prefix.
    fn check_commands(&self, commands: &Keys<Command>) -> Result<()> {
        for f in &self.list {
            // Commands match first, also without Shift (`S-Tab` reaches
            // `Tab`'s command), so either would hide the float.
            let plain = f.key.without(KeyModifiers::SHIFT);
            let bound = commands
                .bindings()
                .find(|(_, keys)| keys.contains(&f.key) || keys.contains(&plain));
            if let Some((b, _)) = bound {
                bail!(
                    "[floats] {:?} is also bound to {} in [keys.commands]; \
                     unbind one, e.g. {} = \"\" in [floats]",
                    f.key.name(),
                    b.name,
                    bare_or_quoted(&toml::Value::String(f.key.name()))
                );
            }
        }
        Ok(())
    }

    /// Rows for the README's table of keys after the prefix, one per
    /// float, to follow the commands' rows.
    #[cfg(test)]
    pub fn markdown(&self) -> String {
        self.list
            .iter()
            .map(|f| {
                format!(
                    "| `{key}` | show or hide {} in a float | `[floats] {key}` |\n",
                    f.command,
                    key = f.key.name()
                )
            })
            .collect()
    }

    /// The keys, in order, as `[floats]` TOML with the size.
    pub fn toml(&self) -> String {
        let mut out = format!(
            "[floats]\n{WIDTH} = {}\n{HEIGHT} = {}\n",
            self.width, self.height
        );
        for f in &self.list {
            let key = toml::Value::String(f.key.name());
            let command = toml::Value::String(f.command.clone());
            let line = format!("{} = {command}", bare_or_quoted(&key));
            out.push_str(&format!("{line:<40} # the prefix, then {}\n", f.key.name()));
        }
        out
    }
}

/// A TOML key: bare when it may be, else quoted.
fn bare_or_quoted(key: &toml::Value) -> String {
    let text = key.as_str().unwrap_or_default();
    let bare = !text.is_empty()
        && text
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if bare {
        text.to_string()
    } else {
        key.to_string()
    }
}

/// `spec` read as a float's key: not the prefix, and not given before.
fn float_key(spec: &str, prefix: Key, given: &[Key]) -> Result<Key> {
    let key = Key::parse(spec).map_err(|e| anyhow::anyhow!("[floats] {spec}: {e}"))?;
    if key == prefix {
        bail!("[floats] {spec}: {spec:?} is the prefix key, which always starts a command");
    }
    if given.contains(&key) {
        bail!("[floats] {spec}: the same key is listed twice");
    }
    Ok(key)
}

/// `value` read as `[floats] <name>`: a whole percent from 1 to 100.
fn percent(name: &str, value: &toml::Value) -> Result<u16> {
    match value.as_integer() {
        Some(p @ 1..=MAX_PERCENT) => Ok(u16::try_from(p).unwrap_or(u16::MAX)),
        _ => bail!("[floats] {name}: expected a percent of the screen, 1 to {MAX_PERCENT}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::keymap::{Given, COMMAND_KEYS};

    fn prefix() -> Key {
        Key::parse("C-a").unwrap()
    }

    fn parse(text: &str) -> Result<Floats> {
        let table: toml::Table = toml::from_str(text).unwrap();
        Floats::configured(&table, prefix(), &Keys::defaults(&COMMAND_KEYS))
    }

    fn list(f: &Floats) -> Vec<(String, &str)> {
        f.list
            .iter()
            .map(|f| (f.key.name(), f.command.as_str()))
            .collect()
    }

    fn err(text: &str) -> String {
        format!("{:#}", parse(text).unwrap_err())
    }

    #[test]
    fn lazygit_on_g_at_80_percent_by_default() {
        let f = Floats::default();
        assert_eq!(list(&f), [("g".to_string(), "lazygit")]);
        assert_eq!((f.width, f.height), (80, 80));
        assert_eq!(parse("").unwrap(), f);
    }

    #[test]
    fn floats_are_added_replaced_and_sized() {
        let f = parse("width = 90\nheight = 50\ng = \"tig\"\nT = \"htop -d 5\"\n").unwrap();
        assert_eq!(
            list(&f),
            [("g".to_string(), "tig"), ("T".to_string(), "htop -d 5")]
        );
        assert_eq!((f.width, f.height), (90, 50));
        let ctrl = parse("C-y = \"yazi\"\n").unwrap();
        assert_eq!(ctrl.list[1].key, Key::parse("C-y").unwrap());
    }

    #[test]
    fn an_empty_command_removes_a_float() {
        assert!(parse("g = \"\"\n").unwrap().list.is_empty());
        assert!(parse("g = \"  \"\n").unwrap().list.is_empty());
        assert_eq!(parse("y = \"\"\n").unwrap(), Floats::default());
    }

    #[test]
    fn a_key_bound_to_a_command_is_an_error_unless_one_is_unbound() {
        let e = err("c = \"htop\"\n");
        assert!(e.contains("\"c\" is also bound to new_tab"), "{e}");
        assert!(e.contains("c = \"\" in [floats]"), "{e}");
        // The float's key freed by unbinding the command.
        let given: Given = [("new_tab".to_string(), vec!["t".to_string()])].into();
        let (commands, _) = Keys::configured(&COMMAND_KEYS, &given, prefix()).unwrap();
        let table: toml::Table = toml::from_str("c = \"htop\"\n").unwrap();
        assert!(Floats::configured(&table, prefix(), &commands).is_ok());
        // The default float clashes with a command moved onto its key.
        let given: Given = [("new_tab".to_string(), vec!["g".to_string()])].into();
        let (commands, _) = Keys::configured(&COMMAND_KEYS, &given, prefix()).unwrap();
        let none = toml::Table::new();
        let e = format!(
            "{:#}",
            Floats::configured(&none, prefix(), &commands).unwrap_err()
        );
        assert!(e.contains("\"g\" is also bound to new_tab"), "{e}");
    }

    #[test]
    fn bad_keys_sizes_and_commands_are_errors() {
        assert!(err("\"Ctrl+g\" = \"x\"\n").contains("[floats] Ctrl+g: invalid key"));
        assert!(err("C-a = \"x\"\n").contains("is the prefix key"));
        assert!(err("C-y = \"x\"\nc-y = \"z\"\n").contains("listed twice"));
        // Reached by a command first, through its key without Shift.
        assert!(err("\"S-Tab\" = \"x\"\n").contains("also bound to last_page"));
        assert!(err("\"S-Left\" = \"x\"\n").contains("also bound to focus_left"));
        assert!(err("g = 1\n").contains("expected a command"));
        for bad in ["0", "101", "\"80\"", "-5", "80.5"] {
            let e = err(&format!("width = {bad}\n"));
            assert!(
                e.contains("[floats] width: expected a percent"),
                "{bad}: {e}"
            );
        }
        assert!(err("height = 0\n").contains("[floats] height"));
    }

    #[test]
    fn the_readme_lists_the_default_floats_and_size() {
        let readme = include_str!("../../README.md");
        let configure = &readme[readme.find("## Configure").unwrap()..];
        for f in &Floats::default().list {
            let line = format!("\n{} = {:?}", f.key.name(), f.command);
            assert!(configure.contains(&line), "README is missing {line:?}");
            let key = format!("`C-a {}` shows {}", f.key.name(), f.command);
            assert!(readme.contains(&key), "README is missing {key:?}");
        }
        // Its keys in the table of keys after the prefix, after the
        // commands.
        let rows = Keys::defaults(&COMMAND_KEYS).markdown() + &Floats::default().markdown();
        assert!(readme.contains(&rows), "README is missing:\n{rows}");
        for setting in [WIDTH, HEIGHT] {
            let line = format!("\n{setting} = {DEFAULT_SIZE} ");
            assert!(configure.contains(&line), "README is missing {line:?}");
        }
    }

    #[test]
    fn toml_reads_back_as_the_same_floats() {
        let f = parse("width = 70\nT = \"htop\"\n\"!\" = \"echo 'a'\"\n").unwrap();
        let text = f.toml();
        assert!(
            text.starts_with("[floats]\nwidth = 70\nheight = 80\n"),
            "{text}"
        );
        assert!(text.contains("g = \"lazygit\""), "{text}");
        let table: toml::Table = toml::from_str(&text).unwrap();
        let back = Floats::configured(
            table["floats"].as_table().unwrap(),
            prefix(),
            &Keys::defaults(&COMMAND_KEYS),
        )
        .unwrap();
        assert_eq!(back, f);
    }
}
