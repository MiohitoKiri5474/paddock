//! `config.toml` in the config directory:
//!
//! ```toml
//! [keys]
//! prefix = "C-b"
//!
//! [colors]
//! current_space = 236        # or "#303030": the current space's background
//! tab_bar = 235              # the tab bar's row
//! tab = 237                  # a tab that is not current
//! tab_current = 4            # the current tab (default: the terminal's blue)
//! ```

use crate::ui::tab_bar::TabColors;
use anyhow::{bail, Context, Result};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::style::Color;
use serde::Deserialize;
use std::path::Path;

pub const FILE_NAME: &str = "config.toml";
const DEFAULT_PREFIX: &str = "C-a";
/// A dark grey, a little darker than most dark themes' background.
const DEFAULT_CURRENT_SPACE: Color = Color::Indexed(236);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    pub prefix: KeyEvent,
    /// Background of the current space.
    pub current_space: Color,
    pub tabs: TabColors,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct RawConfig {
    #[serde(default)]
    keys: RawKeys,
    #[serde(default)]
    colors: RawColors,
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
        let prefix = raw.keys.prefix.as_deref().unwrap_or(DEFAULT_PREFIX);
        let c = raw.colors;
        let tabs = TabColors::default();
        Ok(Self {
            prefix: parse_key(prefix)?,
            current_space: colour(c.current_space, DEFAULT_CURRENT_SPACE)?,
            tabs: TabColors {
                bar: colour(c.tab_bar, tabs.bar)?,
                tab: colour(c.tab, tabs.tab)?,
                current: colour(c.tab_current, tabs.current)?,
            },
        })
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            prefix: KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL),
            current_space: DEFAULT_CURRENT_SPACE,
            tabs: TabColors::default(),
        }
    }
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

/// Parse a prefix key such as `C-a`, `C-b` or `C-Space`. Only Ctrl keys are
/// allowed: plain keys would be unusable for typing, and paddock does not rely
/// on Option/Alt (design: any terminal app).
pub fn parse_key(spec: &str) -> Result<KeyEvent> {
    let invalid = || {
        format!("invalid prefix key {spec:?}: expected C- followed by a letter, Space, or one of \\ ] ^ _ (for example \"C-a\")")
    };
    let Some(key) = spec.strip_prefix("C-") else {
        bail!(invalid())
    };
    let c = match key {
        k if k.eq_ignore_ascii_case("space") => ' ',
        k if k.chars().count() == 1 => k.chars().next().unwrap_or(' '),
        _ => bail!(invalid()),
    };
    if !(c.is_ascii_lowercase() || matches!(c, ' ' | '\\' | ']' | '^' | '_')) {
        bail!(invalid());
    }
    Ok(KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL))
}

/// How a key is written in help text, e.g. `C-a`.
pub fn key_name(key: &KeyEvent) -> String {
    let base = match key.code {
        KeyCode::Char(' ') => "Space".to_string(),
        KeyCode::Char(c) => c.to_string(),
        other => format!("{other:?}"),
    };
    if key.modifiers.contains(KeyModifiers::CONTROL) {
        format!("C-{base}")
    } else {
        base
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctrl(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
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
        assert_eq!(parse_key("C-Space").unwrap(), ctrl(' '));
        assert_eq!(parse_key("C-]").unwrap(), ctrl(']'));
    }

    #[test]
    fn rejects_bad_keys_and_unknown_settings() {
        for bad in ["a", "C-", "C-ab", "M-a", "C-A", "C-1", "ctrl-a"] {
            let err = parse_key(bad).unwrap_err().to_string();
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
    fn names_keys_for_help() {
        assert_eq!(key_name(&ctrl('a')), "C-a");
        assert_eq!(key_name(&ctrl(' ')), "C-Space");
    }
}
