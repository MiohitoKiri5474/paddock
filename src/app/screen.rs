//! The outer terminal paddock runs in: raw mode, alternate screen, mouse,
//! bracketed paste, focus reports and kitty key disambiguation, restored on
//! drop (and so also when paddock fails).

use anyhow::Result;
use crossterm::event::{
    DisableBracketedPaste, DisableFocusChange, DisableMouseCapture, EnableBracketedPaste,
    EnableFocusChange, EnableMouseCapture, KeyboardEnhancementFlags, PopKeyboardEnhancementFlags,
    PushKeyboardEnhancementFlags,
};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use std::io::{stdout, Stdout};

pub type Screen = ratatui::Terminal<CrosstermBackend<Stdout>>;

pub struct ScreenGuard;

impl ScreenGuard {
    /// Take over the outer terminal. Must run before anything else reads
    /// stdin: ratatui queries the cursor position while starting.
    pub fn enter() -> Result<(Self, Screen)> {
        enable_raw_mode()?;
        let guard = Self;
        execute!(
            stdout(),
            EnterAlternateScreen,
            EnableMouseCapture,
            EnableBracketedPaste,
            EnableFocusChange,
            PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
        )?;
        let mut screen = ratatui::Terminal::new(CrosstermBackend::new(stdout()))?;
        screen.clear()?;
        Ok((guard, screen))
    }
}

impl Drop for ScreenGuard {
    fn drop(&mut self) {
        // Best effort: there is nowhere to report a failure while restoring.
        let _ = execute!(
            stdout(),
            PopKeyboardEnhancementFlags,
            DisableFocusChange,
            DisableBracketedPaste,
            DisableMouseCapture,
            LeaveAlternateScreen
        );
        let _ = disable_raw_mode();
    }
}
