//! The last page (`C-a Tab`): the space and tab shown before the current
//! one, across spaces, like tmux's `last-window`. Kept while paddock runs.

use super::state::App;

/// A page: a tab (tmux window `window`) of a space (tmux session `space`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Page {
    pub space: String,
    pub window: String,
}

/// The page shown now and the one shown before it.
#[derive(Debug, Default)]
pub struct Pages {
    current: Option<Page>,
    last: Option<Page>,
}

impl Pages {
    /// Note that `page` is shown now; the one shown until now becomes the
    /// last page.
    pub fn visit(&mut self, page: Page) {
        if self.current.as_ref() != Some(&page) {
            self.last = self.current.replace(page);
        }
    }

    /// The page shown before the current one.
    pub fn last(&self) -> Option<&Page> {
        self.last.as_ref()
    }
}

impl App {
    /// Note the page shown now (called after every event, so every way of
    /// switching counts).
    pub(super) fn track_page(&mut self) {
        let Some(space) = self.current_space() else {
            return;
        };
        let Some(window) = space.current_window() else {
            return;
        };
        let page = Page {
            space: space.id.clone(),
            window: window.id.clone(),
        };
        self.pages.visit(page);
    }

    /// `C-a Tab`: show the last page; if its tab is gone, its space's
    /// current tab; if its space is gone too, say so.
    pub(super) fn last_page(&mut self) {
        let Some(page) = self.pages.last().cloned() else {
            self.notice = Some("no last page".into());
            return;
        };
        let Some(i) = self.spaces.iter().position(|s| s.id == page.space) else {
            self.notice = Some("no last page".into());
            return;
        };
        self.select_space(i);
        let tab = self.spaces[i]
            .windows()
            .iter()
            .position(|w| w.id == page.window);
        if let Some(tab) = tab {
            self.report(|app| app.select_tab(tab));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn page(space: &str, window: &str) -> Page {
        Page {
            space: space.into(),
            window: window.into(),
        }
    }

    #[test]
    fn the_last_page_is_the_one_shown_before_the_current_one() {
        let mut p = Pages::default();
        assert_eq!(p.last(), None);
        p.visit(page("$1", "@1"));
        assert_eq!(p.last(), None);
        p.visit(page("$1", "@1"));
        assert_eq!(p.last(), None, "the same page again changes nothing");
        p.visit(page("$1", "@2"));
        assert_eq!(p.last(), Some(&page("$1", "@1")));
        p.visit(page("$2", "@5"));
        assert_eq!(p.last(), Some(&page("$1", "@2")), "across spaces");
        // Going back makes the page left the last one: pressing again returns.
        p.visit(page("$1", "@2"));
        assert_eq!(p.last(), Some(&page("$2", "@5")));
    }
}
