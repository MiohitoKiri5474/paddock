//! Tests of the key bindings: parsing, sections, configuration errors,
//! the TOML and the README tables.

use super::*;
use crate::terminal::emulator::Motion;
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
        .chain(keymap.direct.bindings().map(|(_, k)| k))
        .chain(keymap.sidebar.bindings().map(|(_, k)| k))
        .chain(keymap.dashboard.bindings().map(|(_, k)| k))
        .chain(keymap.copy.bindings().map(|(_, k)| k));
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
            assert_eq!(parsed.len(), section.default_keys(b).len(), "{}", b.name);
            let later = &section.table[i + 1..];
            assert!(later.iter().all(|o| o.name != b.name), "{}", b.name);
        }
    }
    check(&COMMAND_KEYS);
    check(&DIRECT_KEYS);
    check(&SIDEBAR_KEYS);
    check(&DASHBOARD_KEYS);
    check(&COPY_KEYS);
    // Valid together: no key twice, none the default prefix, no
    // warning (the direct chords' Alt is paddock's choice) once
    // `[keys.direct]` is written.
    let written = GivenKeys {
        direct: Some(Given::new()),
        ..GivenKeys::default()
    };
    let (defaults, warnings) = Keymap::configured(ctrl_a(), &written).unwrap();
    assert_eq!(defaults, Keymap::default());
    assert!(warnings.is_empty(), "{warnings:?}");
}

#[test]
fn direct_chords_default_to_ctrl_and_ctrl_alt_keys_and_match_exactly() {
    let keys = Keys::defaults(&DIRECT_KEYS);
    let ctrl = KeyModifiers::CONTROL;
    let ctrl_alt = ctrl | KeyModifiers::ALT;
    let focus = |d| Some(Command::Focus(d));
    use crate::input::Direction::*;
    assert_eq!(keys.action(&key(KeyCode::Char('h'), ctrl)), focus(Left));
    assert_eq!(keys.action(&key(KeyCode::Char('j'), ctrl)), focus(Down));
    assert_eq!(keys.action(&key(KeyCode::Char('k'), ctrl)), focus(Up));
    assert_eq!(keys.action(&key(KeyCode::Char('l'), ctrl)), focus(Right));
    // Cycling tabs has no chord: `C-M-[` is two Esc outside kitty.
    assert_eq!(keys.action(&key(KeyCode::Char('['), ctrl_alt)), None);
    assert_eq!(keys.action(&key(KeyCode::Char(']'), ctrl_alt)), None);
    let new = keys.action(&key(KeyCode::Char('c'), ctrl_alt));
    assert_eq!(new, Some(Command::NewTab));
    let split = keys.action(&key(KeyCode::Char('\\'), ctrl));
    assert_eq!(split, Some(Command::SplitColumns));
    let split = keys.action(&key(KeyCode::Char('-'), ctrl));
    assert_eq!(split, Some(Command::SplitRows));
    // `C-_` is the same byte, and crossterm reports it as `C-7`
    // without the kitty protocol.
    let split = keys.action(&key(KeyCode::Char('_'), ctrl | KeyModifiers::SHIFT));
    assert_eq!(split, Some(Command::SplitRows));
    let split = keys.action(&key(KeyCode::Char('7'), ctrl));
    assert_eq!(split, Some(Command::SplitRows));
    // The command keys are not chords, nor any other command.
    assert_eq!(keys.action(&plain('h')), None);
    assert_eq!(keys.action(&plain('c')), None);
    assert_eq!(keys.action(&key(KeyCode::Char('x'), ctrl)), None);
    // Bound with no modifier, a chord does not match with Shift.
    let (keys, _) = Keys::configured(&DIRECT_KEYS, &given(&[("help", &["F1"])]), ctrl_a()).unwrap();
    assert_eq!(
        keys.action(&key(KeyCode::F(1), KeyModifiers::NONE)),
        Some(Command::Help)
    );
    assert_eq!(keys.action(&key(KeyCode::F(1), KeyModifiers::SHIFT)), None);
}

#[test]
fn direct_chords_are_rebound_and_unbound_like_commands() {
    let direct = |pairs| Keys::configured(&DIRECT_KEYS, &given(pairs), ctrl_a());
    let (keys, warnings) = direct(&[("focus_left", &["C-b"]), ("new_tab", &[""])]).unwrap();
    let ctrl = KeyModifiers::CONTROL;
    let focus_left = Some(Command::Focus(crate::input::Direction::Left));
    assert_eq!(keys.action(&key(KeyCode::Char('b'), ctrl)), focus_left);
    assert_eq!(keys.action(&key(KeyCode::Char('h'), ctrl)), None);
    let ctrl_alt_c = key(KeyCode::Char('c'), ctrl | KeyModifiers::ALT);
    assert_eq!(keys.action(&ctrl_alt_c), None);
    assert!(warnings.is_empty(), "{warnings:?}");
    let e = err(direct(&[("new_tabs", &["C-t"])]));
    assert!(e.contains("[keys.direct] new_tabs: unknown action"), "{e}");
    let e = err(direct(&[("new_tab", &["C-"])]));
    assert!(
        e.contains("[keys.direct] new_tab: invalid key \"C-\""),
        "{e}"
    );
    let e = err(direct(&[("new_tab", &["C-t", "C-T"])]));
    assert!(
        e.contains("[keys.direct] new_tab: \"C-T\" is listed twice"),
        "{e}"
    );
    let e = err(direct(&[("new_tab", &["C-j"])]));
    assert!(
        e.contains("[keys.direct] \"C-j\" is bound to both new_tab and focus_down (default)"),
        "{e}"
    );
    assert!(e.contains("focus_down = \"\""), "{e}");
    let e = err(direct(&[("close", &["C-a"])]));
    assert!(
        e.contains("[keys.direct] close: \"C-a\" is the prefix key"),
        "{e}"
    );
    // An Alt chord given warns; the Alt defaults do not.
    let (_, warnings) = direct(&[("close", &["M-x"])]).unwrap();
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert!(warnings[0].contains("[keys.direct] close: \"M-x\" uses Alt"));
}

#[test]
fn the_control_character_warning_names_the_chords_taken() {
    let direct = |pairs| Keys::configured(&DIRECT_KEYS, &given(pairs), ctrl_a());
    let warning = |pairs| {
        let keymap = Keymap {
            direct: direct(pairs).unwrap().0,
            ..Keymap::default()
        };
        keymap.control_chars_warning()
    };
    assert_eq!(
        warning(&[]).as_deref(),
        Some("[keys.direct] takes C-h and C-j from the terminal")
    );
    assert_eq!(
        warning(&[("focus_left", &[""]), ("close", &["C-m", "C-i"])]).as_deref(),
        Some("[keys.direct] takes C-j and C-m and C-i from the terminal")
    );
    assert_eq!(
        warning(&[("focus_left", &[""]), ("focus_down", &[""])]),
        None
    );
    // `Keymap::configured` warns only while `[keys.direct]` is not
    // written; written, even empty, the user chose the chords.
    let warnings = |direct: Option<Given>| {
        let written = GivenKeys {
            direct,
            ..GivenKeys::default()
        };
        Keymap::configured(ctrl_a(), &written).unwrap().1
    };
    assert_eq!(
        warnings(None),
        ["[keys.direct] takes C-h and C-j from the terminal"]
    );
    assert!(warnings(Some(Given::new())).is_empty());
    assert!(warnings(Some(given(&[("close", &["C-m"])]))).is_empty());
}

#[test]
fn each_command_comes_with_its_direct_chords() {
    let rows: Vec<(&str, String, String)> = Keymap::default()
        .commands_with_chords()
        .map(|(b, keys, chords)| (b.name, label(keys), label(chords)))
        .collect();
    assert_eq!(rows.len(), COMMANDS.len());
    let row = |name: &str| {
        let (_, keys, chords) = rows.iter().find(|(n, _, _)| *n == name)?;
        Some((keys.as_str(), chords.as_str()))
    };
    assert_eq!(row("new_tab"), Some(("c", "C-M-c")));
    assert_eq!(row("focus_right"), Some(("l Right", "C-l")));
    assert_eq!(row("next_tab"), Some(("n", "")));
    // Found by name, whatever order `[keys.direct]` keeps.
    let remapped = GivenKeys {
        direct: Some(given(&[("help", &["F1"]), ("focus_right", &[""])])),
        ..GivenKeys::default()
    };
    let (keymap, _) = Keymap::configured(ctrl_a(), &remapped).unwrap();
    let help = keymap
        .commands_with_chords()
        .find(|(b, _, _)| b.name == "help");
    assert_eq!(help.map(|(_, _, c)| label(c)).as_deref(), Some("F1"));
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
    assert!(
        toml.contains("[keys.direct]\nnew_tab = \"C-M-c\""),
        "{toml}"
    );
    assert!(toml.contains("\nsplit_right = 'C-\\'"), "{toml}");
    assert!(toml.contains("\nsplit_down = \"C--\""), "{toml}");
    assert!(toml.contains("\nclose = \"\""), "{toml}");
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
fn copy_mode_keys_are_vi_like_and_ctrl_matters() {
    let keys = Keys::defaults(&COPY_KEYS);
    let ctrl = KeyModifiers::CONTROL;
    assert_eq!(
        keys.action(&plain('b')),
        Some(CopyAction::Move(Motion::WordBack))
    );
    assert_eq!(
        keys.action(&key(KeyCode::Char('b'), ctrl)),
        Some(CopyAction::Page(-1))
    );
    assert_eq!(
        keys.action(&key(KeyCode::Char('d'), ctrl)),
        Some(CopyAction::HalfPage(1))
    );
    assert_eq!(keys.action(&plain('d')), None);
    let shift_g = key(KeyCode::Char('G'), KeyModifiers::SHIFT);
    assert_eq!(
        keys.action(&shift_g),
        Some(CopyAction::Move(Motion::Bottom))
    );
    assert_eq!(
        keys.action(&plain('g')),
        Some(CopyAction::Move(Motion::Top))
    );
    let enter = key(KeyCode::Enter, KeyModifiers::NONE);
    assert_eq!(keys.action(&enter), Some(CopyAction::Copy));
    assert_eq!(
        keys.action(&plain('/')),
        Some(CopyAction::Search { forward: false })
    );
    assert_eq!(
        keys.action(&plain('?')),
        Some(CopyAction::Search { forward: true })
    );
    let esc = key(KeyCode::Esc, KeyModifiers::NONE);
    assert_eq!(keys.action(&esc), Some(CopyAction::Leave));
    // Rebound and checked like every section.
    let copy = |pairs| Keys::configured(&COPY_KEYS, &given(pairs), ctrl_a());
    let (keys, _) = copy(&[("leave", &["x"])]).unwrap();
    assert_eq!(keys.action(&plain('x')), Some(CopyAction::Leave));
    assert_eq!(keys.action(&plain('q')), None);
    let e = err(copy(&[("leave", &["y"])]));
    assert!(
        e.contains("[keys.copy] \"y\" is bound to both copy (default) and leave"),
        "{e}"
    );
    // Under a `C-b` prefix, the default `C-b` page up is left out; written
    // out, it is an error as anywhere.
    let ctrl_b = Key::parse("C-b").unwrap();
    let (keys, _) = Keys::configured(&COPY_KEYS, &Given::new(), ctrl_b).unwrap();
    assert_eq!(keys.keys_of("page_up"), &[] as &[Key]);
    let written = given(&[("page_up", &["C-b"])]);
    assert!(Keys::configured(&COPY_KEYS, &written, ctrl_b).is_err());
    let e = err(copy(&[("yank", &["y"])]));
    assert!(e.contains("[keys.copy] yank: unknown action"), "{e}");
    let toml = Keymap::default().toml();
    assert!(
        toml.contains("[keys.copy]\nleft = [\"h\", \"Left\"]"),
        "{toml}"
    );
}

#[test]
fn a_default_that_is_the_prefix_is_an_error_outside_copy_mode() {
    // `C-h` is the default `focus_left` chord: as the prefix, the
    // config is refused, naming it, as before copy mode.
    let ctrl_h = Key::parse("C-h").unwrap();
    let e = err(Keys::configured(&DIRECT_KEYS, &Given::new(), ctrl_h));
    assert!(
        e.contains("[keys.direct] focus_left: \"C-h\" is the prefix key"),
        "{e}"
    );
    let e = err(Keymap::configured(ctrl_h, &GivenKeys::default()));
    assert!(e.contains("is the prefix key"), "{e}");
}

#[test]
fn the_readme_lists_the_default_keys() {
    let readme = include_str!("../../../README.md");
    let keymap = Keymap::default();
    for table in [
        keymap.commands.markdown(),
        keymap.direct.markdown(),
        keymap.sidebar.markdown(),
        keymap.dashboard.markdown(),
        keymap.copy.markdown(),
    ] {
        assert!(readme.contains(&table), "README is missing:\n{table}");
    }
}
