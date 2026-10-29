//! The app's side of the desktop tools (`crate::desktop`): copying to the
//! clipboard, and saying a tool's failure once rather than every time.

use super::state::App;
use crate::desktop::{self, CopyFailure};

/// A notice shown the first time only.
#[derive(Debug, Default)]
pub struct SayOnce {
    said: bool,
}

impl SayOnce {
    /// `notice` the first time, `None` after.
    pub fn say(&mut self, notice: String) -> Option<String> {
        (!std::mem::replace(&mut self.said, true)).then_some(notice)
    }
}

impl App {
    /// Put `text` on the clipboard; a failure shows a notice, except that
    /// a copy tool that cannot be run is said once only.
    pub(super) fn copy_to_clipboard(&mut self, text: &str) {
        match desktop::copy(text) {
            Ok(()) => {}
            Err(CopyFailure::NotRun(notice)) => {
                if let Some(notice) = self.copy_not_run.say(notice) {
                    self.notice = Some(notice);
                }
            }
            Err(CopyFailure::Failed(notice)) => self.notice = Some(notice),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_notice_is_said_once() {
        let mut once = SayOnce::default();
        assert_eq!(once.say("first".into()), Some("first".into()));
        assert_eq!(once.say("second".into()), None);
    }
}
