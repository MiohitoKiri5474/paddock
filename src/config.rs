//! `config.toml` in the config directory:
//!
//! ```toml
//! [keys]
//! prefix = "C-b"
//!
//! [colors]
//! current_item = 236        # or "#303030": the current sidebar item's background
//! ```

use anyhow::{bail, Context, Result};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::style::Color;
use serde::Deserialize;
use std::path::Path;

pub const FILE_NAME: &str = "config.toml";
const DEFAULT_PREFIX: &str = "C-a";
/// A dark grey, a little darker than most dark themes' background.
const DEFAULT_CURRENT_ITEM: Color = Color::Indexed(236);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    pub prefix: KeyEvent,
    /// Background of the current sidebar item.
    pub current_item: Color,
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
    current_item: Option<RawColor>,
}

/// A colour as written: a 256-colour index or `"#rrggbb"`.
#[derive(Deserialize)]
#[serde(untagged)]
enum RawColor {
    Index(u8),
    Hex(String),
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
        let current_item = match raw.colors.current_item {
            None => DEFAULT_CURRENT_ITEM,
            Some(RawColor::Index(i)) => Color::Indexed(i),
            Some(RawColor::Hex(hex)) => parse_hex(&hex)?,
        };
        Ok(Self {
            prefix: parse_key(prefix)?,
            current_item,
        })
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            prefix: KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL),
            current_item: DEFAULT_CURRENT_ITEM,
        }
    }
}

/// Parse a colour written `#rrggbb`.
fn parse_hex(spec: &str) -> Result<Color> {
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
    fn the_current_item_colour_is_an_index_or_hex_and_dark_grey_by_default() {
        assert_eq!(Config::default().current_item, Color::Indexed(236));
        let parse = |t: &str| Config::parse(&format!("[colors]\ncurrent_item = {t}\n"));
        assert_eq!(parse("238").unwrap().current_item, Color::Indexed(238));
        assert_eq!(
            parse("\"#1a2B3c\"").unwrap().current_item,
            Color::Rgb(0x1a, 0x2b, 0x3c)
        );
        for bad in ["256", "-1", "\"236\"", "\"#12345\"", "\"red\""] {
            assert!(parse(bad).is_err(), "{bad}");
        }
        assert!(Config::parse("[colors]\ncurrent = 1\n").is_err());
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
