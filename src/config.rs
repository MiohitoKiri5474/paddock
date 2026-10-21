//! `config.toml` in the config directory:
//!
//! ```toml
//! [keys]
//! prefix = "C-b"
//!
//! [keys.commands]            # keys after the prefix, by action
//! new_tab = "t"              # a key, a list of keys, or "" for none
//! split_right = ["|", "\\"]
//!
//! [keys.direct]              # the same actions on one chord, no prefix
//! focus_left = ""
//!
//! [keys.sidebar]             # while the sidebar has the keyboard
//! [keys.dashboard]           # while the dashboard is open
//!
//! [colors]
//! current_space = 236        # or "#303030": the current space's, agent's and dashboard selection's background
//! tab_bar = 235              # the tab bar's row
//! tab = 237                  # a tab that is not current
//! tab_current = 4            # the current tab and the popup border (default: the terminal's blue)
//!
//! [sidebar]
//! width = 32                 # columns, until resized (then remembered)
//!
//! [floats]                   # the prefix, then a key, shows or hides a float
//! width = 80                 # percent of the screen
//! height = 80
//! G = "lazygit"              # the default; "" removes it
//!
//! [notify]                   # macOS notifications, on unless set false
//! waiting = true             # a session or one of its subagents starts waiting on you
//! idle = true                # a main agent finishes its turn
//! ```
//!
//! `paddock keys` prints every action with its default keys.

mod floats;

pub use floats::{Float, Floats};

use crate::input::keymap::{Given, GivenKeys, Key, Keymap, Warnings};
use crate::ui::tab_bar::TabColors;
use anyhow::{bail, Context, Result};
use ratatui::style::Color;
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::Path;

pub const FILE_NAME: &str = "config.toml";
const DEFAULT_PREFIX: &str = "C-a";
/// Columns the sidebar takes, border included, until resized.
const DEFAULT_SIDEBAR_WIDTH: u16 = 32;
/// A dark grey, a little darker than most dark themes' background.
const DEFAULT_CURRENT_SPACE: Color = Color::Indexed(236);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    pub prefix: Key,
    pub keys: Keymap,
    /// Settings that work but may surprise, to show at start.
    pub warnings: Warnings,
    /// Background of the current space.
    pub current_space: Color,
    pub tabs: TabColors,
    /// The sidebar's starting width, while none was saved.
    pub sidebar_width: u16,
    /// Which macOS notifications to send.
    pub notify: Notify,
    /// The floats and the size of their box.
    pub floats: Floats,
}

/// Which macOS notifications to send; states, markers and the dashboard
/// show every change either way.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Notify {
    /// When a session, or one of its subagents, starts waiting on you.
    pub waiting: bool,
    /// When a session's main agent finishes its turn (a subagent finishing
    /// never notifies).
    pub idle: bool,
}

impl Default for Notify {
    fn default() -> Self {
        Self {
            waiting: true,
            idle: true,
        }
    }
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct RawConfig {
    #[serde(default)]
    keys: RawKeys,
    #[serde(default)]
    colors: RawColors,
    #[serde(default)]
    sidebar: RawSidebar,
    #[serde(default)]
    notify: RawNotify,
    /// `[floats]` as written; read by [`Floats::configured`].
    #[serde(default)]
    floats: toml::Table,
}

/// A `[notify]` table as written: a switch per kind of notification.
#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct RawNotify {
    waiting: Option<bool>,
    idle: Option<bool>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct RawSidebar {
    width: Option<u16>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct RawColors {
    /// Written `current_item` before spaces were called spaces.
    #[serde(alias = "current_item")]
    current_space: Option<RawColor>,
    tab_bar: Option<RawColor>,
    tab: Option<RawColor>,
    tab_current: Option<RawColor>,
}

/// `raw` as a colour, else `default`.
fn colour(raw: Option<RawColor>, default: Color) -> Result<Color> {
    match raw {
        None => Ok(default),
        Some(RawColor::Index(i)) => Ok(Color::Indexed(i)),
        Some(RawColor::Text(text)) => parse_colour(&text),
    }
}

/// A colour as written: a 256-colour index (bare or quoted) or `"#rrggbb"`.
#[derive(Deserialize)]
#[serde(untagged)]
enum RawColor {
    Index(u8),
    Text(String),
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct RawKeys {
    prefix: Option<String>,
    #[serde(default)]
    commands: RawSection,
    /// `None` while `[keys.direct]` is not written at all.
    direct: Option<RawSection>,
    #[serde(default)]
    sidebar: RawSection,
    #[serde(default)]
    dashboard: RawSection,
}

/// An action's keys as written: one key, or a list of them.
#[derive(Deserialize)]
#[serde(untagged)]
enum RawKeyList {
    One(String),
    Many(Vec<String>),
}

/// A `[keys.<section>]` table as written.
type RawSection = BTreeMap<String, RawKeyList>;

/// A `[keys.<section>]` table as lists of keys by action.
fn given(section: RawSection) -> Given {
    section
        .into_iter()
        .map(|(name, keys)| match keys {
            RawKeyList::One(key) => (name, vec![key]),
            RawKeyList::Many(keys) => (name, keys),
        })
        .collect()
}

impl Config {
    /// Read `config.toml` from `dir`; a missing file means the defaults.
    pub fn load(dir: &Path) -> Result<Self> {
        let path = dir.join(FILE_NAME);
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(e) => return Err(e).with_context(|| format!("cannot read {}", path.display())),
        };
        Self::parse(&text).with_context(|| format!("invalid config in {}", path.display()))
    }

    pub fn parse(text: &str) -> Result<Self> {
        let raw: RawConfig = toml::from_str(text)?;
        let prefix = Key::parse_prefix(raw.keys.prefix.as_deref().unwrap_or(DEFAULT_PREFIX))?;
        let written = GivenKeys {
            commands: given(raw.keys.commands),
            direct: raw.keys.direct.map(given),
            sidebar: given(raw.keys.sidebar),
            dashboard: given(raw.keys.dashboard),
        };
        let (keys, warnings) = Keymap::configured(prefix, &written)?;
        let floats = Floats::configured(&raw.floats, prefix, &keys.commands)?;
        let c = raw.colors;
        let tabs = TabColors::default();
        Ok(Self {
            prefix,
            keys,
            warnings,
            current_space: colour(c.current_space, DEFAULT_CURRENT_SPACE)?,
            tabs: TabColors {
                bar: colour(c.tab_bar, tabs.bar)?,
                tab: colour(c.tab, tabs.tab)?,
                current: colour(c.tab_current, tabs.current)?,
            },
            sidebar_width: raw.sidebar.width.unwrap_or(DEFAULT_SIDEBAR_WIDTH),
            notify: {
                let on = Notify::default();
                Notify {
                    waiting: raw.notify.waiting.unwrap_or(on.waiting),
                    idle: raw.notify.idle.unwrap_or(on.idle),
                }
            },
            floats,
        })
    }
}

impl Default for Config {
    fn default() -> Self {
        let prefix = Key::parse_prefix(DEFAULT_PREFIX).expect("the default prefix parses");
        // As a config with no keys reads: the default direct chords take
        // `C-h` and `C-j` from the terminal, which an unwritten
        // `[keys.direct]` warns of.
        let (keys, warnings) =
            Keymap::configured(prefix, &GivenKeys::default()).expect("the default keys are valid");
        Self {
            prefix,
            keys,
            warnings,
            current_space: DEFAULT_CURRENT_SPACE,
            tabs: TabColors::default(),
            sidebar_width: DEFAULT_SIDEBAR_WIDTH,
            notify: Notify::default(),
            floats: Floats::default(),
        }
    }
}

/// Every key setting with its default, as `paddock keys` prints it.
pub fn default_keys_toml() -> String {
    format!(
        "# paddock's default keys. Copy what you change into {FILE_NAME}\n\
         # (~/.config/paddock/{FILE_NAME}); \"\" unbinds an action.\n\
         # Not configurable: the prefix pressed twice sends it to the terminal,\n\
         # and Esc always leaves typing into the dashboard preview.\n\
         [keys]\nprefix = \"{DEFAULT_PREFIX}\"\n\n{}\n{}",
        Keymap::default().toml(),
        Floats::default().toml()
    )
}

/// Parse a colour written `236` (a 256-colour index) or `#rrggbb`.
fn parse_colour(spec: &str) -> Result<Color> {
    if let Ok(i) = spec.parse::<u8>() {
        return Ok(Color::Indexed(i));
    }
    let digits = spec
        .strip_prefix('#')
        .filter(|d| d.len() == 6 && d.bytes().all(|b| b.is_ascii_hexdigit()));
    let Some(d) = digits else {
        bail!("invalid colour {spec:?}: expected a 256-colour index (0-255) or \"#rrggbb\"");
    };
    let byte = |i: usize| u8::from_str_radix(&d[i..i + 2], 16).unwrap_or(0);
    Ok(Color::Rgb(byte(0), byte(2), byte(4)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::keymap::SidebarAction;
    use crate::input::{Command, Direction};
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    fn ctrl(c: char) -> Key {
        Key::parse(&format!("C-{c}")).unwrap()
    }

    #[test]
    fn defaults_to_ctrl_a() {
        assert_eq!(Config::parse("").unwrap().prefix, ctrl('a'));
        assert_eq!(Config::parse("[keys]\n").unwrap().prefix, ctrl('a'));
        assert_eq!(Config::default().prefix, ctrl('a'));
    }

    #[test]
    fn reads_the_prefix() {
        assert_eq!(
            Config::parse("[keys]\nprefix = \"C-b\"\n").unwrap().prefix,
            ctrl('b')
        );
        assert_eq!(
            Config::parse("[keys]\nprefix = \"C-B\"\n").unwrap().prefix,
            ctrl('b')
        );
    }

    #[test]
    fn rejects_bad_keys_and_unknown_settings() {
        for bad in ["a", "C-", "C-ab", "M-a", "C-1", "ctrl-a"] {
            let err = format!(
                "{:#}",
                Config::parse(&format!("[keys]\nprefix = {bad:?}\n")).unwrap_err()
            );
            assert!(err.contains("invalid prefix key"), "{bad}: {err}");
        }
        assert!(Config::parse("[keys]\nprefx = \"C-b\"\n").is_err());
        assert!(Config::parse("[keys]\nprefix = 1\n").is_err());
    }

    #[test]
    fn the_current_space_colour_is_an_index_or_hex_and_dark_grey_by_default() {
        assert_eq!(Config::default().current_space, Color::Indexed(236));
        let parse = |t: &str| Config::parse(&format!("[colors]\ncurrent_space = {t}\n"));
        assert_eq!(parse("238").unwrap().current_space, Color::Indexed(238));
        assert_eq!(
            parse("\"#1a2B3c\"").unwrap().current_space,
            Color::Rgb(0x1a, 0x2b, 0x3c)
        );
        assert_eq!(parse("\"236\"").unwrap().current_space, Color::Indexed(236));
        for bad in ["256", "-1", "\"256\"", "\"#12345\"", "\"red\""] {
            assert!(parse(bad).is_err(), "{bad}");
        }
        assert!(Config::parse("[colors]\ncurrent = 1\n").is_err());
    }

    #[test]
    fn the_old_current_item_key_still_sets_the_current_space_colour() {
        let c = Config::parse("[colors]\ncurrent_item = 238\n").unwrap();
        assert_eq!(c.current_space, Color::Indexed(238));
    }

    #[test]
    fn tab_bar_colours_default_to_herdrs_and_can_be_set() {
        let tabs = Config::default().tabs;
        assert_eq!(
            (tabs.bar, tabs.tab, tabs.current),
            (Color::Indexed(235), Color::Indexed(237), Color::Blue)
        );
        let c = Config::parse("[colors]\ntab_bar = 233\ntab = \"#202020\"\ntab_current = 33\n")
            .unwrap();
        assert_eq!(
            (c.tabs.bar, c.tabs.tab, c.tabs.current),
            (
                Color::Indexed(233),
                Color::Rgb(32, 32, 32),
                Color::Indexed(33)
            )
        );
        assert!(Config::parse("[colors]\ntabs = 1\n").is_err());
        assert!(Config::parse("[colors]\ntab = \"blue\"\n").is_err());
    }

    #[test]
    fn the_sidebar_width_defaults_to_32_and_can_be_set() {
        assert_eq!(Config::default().sidebar_width, 32);
        assert_eq!(
            Config::parse("[sidebar]\nwidth = 40\n")
                .unwrap()
                .sidebar_width,
            40
        );
        assert!(Config::parse("[sidebar]\nwide = 40\n").is_err());
        assert!(Config::parse("[sidebar]\nwidth = -1\n").is_err());
    }

    #[test]
    fn both_notifications_are_on_by_default_and_each_can_be_turned_off() {
        let on = Notify::default();
        assert!(on.waiting && on.idle);
        assert_eq!(Config::default().notify, on);
        assert_eq!(Config::parse("[notify]\n").unwrap().notify, on);
        assert_eq!(
            Config::parse("[notify]\nidle = false\n").unwrap().notify,
            Notify { idle: false, ..on }
        );
        assert_eq!(
            Config::parse("[notify]\nwaiting = false\nidle = false\n")
                .unwrap()
                .notify,
            Notify {
                waiting: false,
                idle: false
            }
        );
        assert!(Config::parse("[notify]\nidle = \"no\"\n").is_err());
        assert!(Config::parse("[notify]\nidle = 0\n").is_err());
        assert!(Config::parse("[notify]\ndone = false\n").is_err());
    }

    #[test]
    fn load_reports_the_file_and_tolerates_a_missing_one() {
        let dir = std::env::temp_dir().join(format!("paddock-config-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(Config::load(&dir).unwrap(), Config::default());
        std::fs::write(dir.join(FILE_NAME), "[keys]\nprefix = \"x\"\n").unwrap();
        let err = format!("{:#}", Config::load(&dir).unwrap_err());
        assert!(
            err.contains("config.toml") && err.contains("invalid prefix key"),
            "{err}"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_printed_default_keys_read_back_as_the_defaults() {
        let text = default_keys_toml();
        assert!(text.contains("[keys.commands]\nnew_tab = \"c\""), "{text}");
        assert!(
            text.contains("[keys.direct]\nnew_tab = \"C-M-c\""),
            "{text}"
        );
        assert!(text.contains("\nsplit_right = 'C-\\'"), "{text}");
        assert!(text.contains("\nsplit_down = \"C--\""), "{text}");
        // With `[keys.direct]` written out, the control characters it
        // takes are chosen: no warning.
        let read = Config::parse(&text).unwrap();
        assert!(read.warnings.is_empty(), "{:?}", read.warnings);
        assert_eq!(
            Config {
                warnings: Warnings::new(),
                ..Config::default()
            },
            read
        );
    }

    #[test]
    fn remapped_keys_come_from_the_keys_sections() {
        let c = Config::parse(
            "[keys.commands]\nnew_tab = \"t\"\n[keys.sidebar]\nback = [\"q\", \"Esc\"]\n\
             [keys.direct]\nfocus_left = \"\"\n",
        )
        .unwrap();
        let key = |c| KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE);
        assert_eq!(c.keys.commands.action(&key('t')), Some(Command::NewTab));
        assert_eq!(c.keys.commands.action(&key('c')), None);
        assert_eq!(c.keys.sidebar.action(&key('q')), Some(SidebarAction::Back));
        assert!(c.warnings.is_empty());
        let alt = Config::parse("[keys.dashboard]\ninteract = \"M-i\"\n[keys.direct]\n").unwrap();
        assert_eq!(alt.warnings.len(), 1);
    }

    #[test]
    fn direct_chords_come_from_their_section_and_warn_of_control_characters_unless_set() {
        let ctrl = |c| KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL);
        let c = Config::parse("").unwrap();
        assert_eq!(
            c.keys.direct.action(&ctrl('h')),
            Some(Command::Focus(Direction::Left))
        );
        assert_eq!(
            c.keys.direct.action(&ctrl('l')),
            Some(Command::Focus(Direction::Right))
        );
        assert_eq!(
            c.warnings,
            vec!["[keys.direct] takes C-h and C-j from the terminal".to_string()]
        );
        assert_eq!(c, Config::default());
        // Set, even to the same chords or empty, the user knows.
        let set = Config::parse("[keys.direct]\nfocus_left = \"C-h\"\n").unwrap();
        assert!(set.warnings.is_empty(), "{:?}", set.warnings);
        let empty = Config::parse("[keys.direct]\n").unwrap();
        assert!(empty.warnings.is_empty(), "{:?}", empty.warnings);
        assert_eq!(empty.keys, Keymap::default());
        // Other control characters chosen are the user's business too.
        let other = Config::parse("[keys.direct]\nclose = \"C-m\"\n").unwrap();
        assert!(other.warnings.is_empty(), "{:?}", other.warnings);
        assert_eq!(
            set.keys.direct.action(&ctrl('h')),
            Some(Command::Focus(Direction::Left))
        );
        // Unbound, the chord goes to the terminal again.
        let unbound = Config::parse("[keys.direct]\nfocus_left = \"\"\n").unwrap();
        assert_eq!(unbound.keys.direct.action(&ctrl('h')), None);
        assert_eq!(
            unbound.keys.direct.action(&ctrl('l')),
            Some(Command::Focus(Direction::Right))
        );
        // Any command may be bound, on a chord, and a given Alt chord warns.
        let more = Config::parse("[keys.direct]\ndetach = \"C-M-q\"\nhelp = \"F1\"\n").unwrap();
        let ctrl_alt = KeyModifiers::CONTROL | KeyModifiers::ALT;
        let alt_q = KeyEvent::new(KeyCode::Char('q'), ctrl_alt);
        assert_eq!(more.keys.direct.action(&alt_q), Some(Command::Detach));
        let f1 = KeyEvent::new(KeyCode::F(1), KeyModifiers::NONE);
        assert_eq!(more.keys.direct.action(&f1), Some(Command::Help));
        assert_eq!(more.warnings.len(), 1, "{:?}", more.warnings);
        assert!(more.warnings[0].contains("[keys.direct] detach: \"C-M-q\" uses Alt"));
    }

    #[test]
    fn direct_chord_errors_name_the_section_and_a_command_key_is_no_clash() {
        let err = |t: &str| format!("{:#}", Config::parse(t).unwrap_err());
        assert!(err("[keys.direct]\nnew_tabs = \"C-t\"\n").contains("[keys.direct] new_tabs"));
        assert!(err("[keys.direct]\nnew_tab = \"Ctrl+t\"\n").contains("invalid key \"Ctrl+t\""));
        assert!(err("[keys.direct]\nnew_tab = \"C-a\"\n").contains("is the prefix key"));
        assert!(err("[keys.direct]\nnew_tab = [\"C-t\", \"C-t\"]\n").contains("is listed twice"));
        let e = err("[keys.direct]\nnew_tab = \"C-l\"\n");
        assert!(
            e.contains("[keys.direct] \"C-l\" is bound to both new_tab and focus_right (default)"),
            "{e}"
        );
        // `C-_` is `C--`, so it clashes with the default `split_down`.
        assert!(err("[keys.direct]\nclose = \"C-_\"\n").contains("\"C--\" is bound to both"));
        // After the prefix, `c` is a new tab; the same key is free here.
        let c = Config::parse("[keys.direct]\nclose = \"c\"\n").unwrap();
        let plain_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::NONE);
        assert_eq!(c.keys.direct.action(&plain_c), Some(Command::Close));
        assert_eq!(c.keys.commands.action(&plain_c), Some(Command::NewTab));
    }

    #[test]
    fn key_errors_name_the_section_and_unknown_sections_are_rejected() {
        let err = |t: &str| format!("{:#}", Config::parse(t).unwrap_err());
        assert!(err("[keys.commands]\nnew_tabs = \"t\"\n").contains("[keys.commands] new_tabs"));
        assert!(err("[keys.sidebar]\nback = \"Ctrl+q\"\n").contains("invalid key \"Ctrl+q\""));
        assert!(err("[keys.commands]\nhelp = \"C-a\"\n").contains("is the prefix key"));
        assert!(err("[keys.dashboard]\nup = \"D\"\n").contains("bound to both"));
        assert!(Config::parse("[keys.panes]\nx = \"y\"\n").is_err());
        assert!(Config::parse("[keys.commands]\nnew_tab = 1\n").is_err());
    }

    #[test]
    fn the_readme_example_config_is_valid() {
        let readme = include_str!("../README.md");
        let configure = &readme[readme.find("## Configure").unwrap()..];
        let start = configure.find("```toml\n").unwrap() + "```toml\n".len();
        let example = &configure[start..start + configure[start..].find("```").unwrap()];
        let c = Config::parse(example).unwrap();
        assert_ne!(c.keys, Keymap::default());
    }

    #[test]
    fn floats_come_from_their_table_and_may_not_take_a_command_key() {
        assert_eq!(Config::default().floats, Floats::default());
        let c = Config::parse("[floats]\nwidth = 60\nG = \"\"\nT = \"htop\"\n").unwrap();
        assert_eq!(c.floats.width, 60);
        assert_eq!(c.floats.list.len(), 1);
        assert_eq!(c.floats.list[0].command, "htop");
        let err = |t: &str| format!("{:#}", Config::parse(t).unwrap_err());
        assert!(err("[floats]\nn = \"htop\"\n").contains("bound to next_tab"));
        assert!(err("[keys.commands]\nhelp = \"G\"\n").contains("\"G\" is also bound to help"));
        assert!(Config::parse("[keys.commands]\nhelp = \"G\"\n[floats]\nG = \"\"\n").is_ok());
        assert!(err("[keys]\nprefix = \"C-g\"\n[floats]\nC-g = \"x\"\n").contains("prefix key"));
    }

    #[test]
    fn the_prefix_setting_and_the_commands_table_are_different_keys() {
        // TOML cannot have `keys.prefix` as both the prefix and a table.
        assert!(
            Config::parse("[keys]\nprefix = \"C-b\"\n[keys.prefix]\nnew_tab = \"t\"\n").is_err()
        );
        assert!(
            Config::parse("[keys]\nprefix = \"C-b\"\n[keys.commands]\nnew_tab = \"t\"\n").is_ok()
        );
    }
}
