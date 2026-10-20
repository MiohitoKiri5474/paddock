//! The last page (`C-a Tab`): the space and tab shown before the current
//! one, across spaces, like tmux's `last-window`. Kept while paddock runs.

use super::state::App;

/// A page: a tab (tmux window `window`) of a space (tmux session `space`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Page {
    pub space: String,
    pub window: String,
}

/// How many pages are remembered.
const HISTORY: usize = 32;

/// The pages shown, oldest first; the last is the one shown now.
#[derive(Debug, Default)]
pub struct Pages {
    shown: Vec<Page>,
}

impl Pages {
    /// Note that `page` is shown now.
    pub fn visit(&mut self, page: Page) {
        if self.shown.last() != Some(&page) {
            self.shown.push(page);
        }
        if self.shown.len() > HISTORY {
            self.shown.remove(0);
        }
    }

    /// Keep up with closed tabs and spaces: `now` gives what each page is
    /// now (its space's current tab if the tab closed), or `None` if its
    /// space is gone too, and the page is dropped.
    pub fn retain(&mut self, now: impl Fn(&Page) -> Option<Page>) {
        let mut kept: Vec<Page> = Vec::with_capacity(self.shown.len());
        for page in self.shown.iter().filter_map(now) {
            if kept.last() != Some(&page) {
                kept.push(page);
            }
        }
        self.shown = kept;
    }

    /// The latest page shown before the current one that is not the
    /// current one.
    pub fn last(&self) -> Option<&Page> {
        let current = self.shown.last()?;
        self.shown.iter().rev().find(|p| *p != current)
    }
}

impl App {
    /// Note the page shown now (called after every event, so every way of
    /// switching counts), and keep the pages up with closed tabs and spaces.
    pub(super) fn track_page(&mut self) {
        let Some(page) = self.current_page() else {
            return;
        };
        self.pages.visit(page);
        let spaces = &self.spaces;
        self.pages.retain(|p| {
            let space = spaces.iter().find(|s| s.id == p.space)?;
            let window = match space.windows().iter().find(|w| w.id == p.window) {
                Some(w) => w,
                None => space.current_window()?,
            };
            Some(Page {
                space: space.id.clone(),
                window: window.id.clone(),
            })
        });
    }

    /// The space and tab shown now.
    fn current_page(&self) -> Option<Page> {
        let space = self.current_space()?;
        Some(Page {
            space: space.id.clone(),
            window: space.current_window()?.id.clone(),
        })
    }

    /// `C-a Tab`: show the last page that still exists and is not the
    /// current one, else say there is none.
    pub(super) fn last_page(&mut self) {
        let target = self.pages.last().cloned().and_then(|page| {
            let i = self.spaces.iter().position(|s| s.id == page.space)?;
            let tab = self.spaces[i]
                .windows()
                .iter()
                .position(|w| w.id == page.window)?;
            Some((i, tab))
        });
        let Some((space, tab)) = target else {
            self.notice = Some(NO_LAST_PAGE.into());
            return;
        };
        self.select_space(space);
        self.report(|app| app.select_tab(tab));
    }
}

/// What `C-a Tab` says when there is no page to go back to.
const NO_LAST_PAGE: &str = "no last page";

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
    fn a_closed_page_falls_back_to_its_space_and_never_to_the_current_page() {
        let mut p = Pages::default();
        for (s, w) in [("$1", "@1"), ("$2", "@2"), ("$2", "@3"), ("$2", "@2")] {
            p.visit(page(s, w));
        }
        // @2 closed: space $2 shows @3 now.
        p.visit(page("$2", "@3"));
        let exists = |q: &Page| (q.window != "@2").then(|| q.clone());
        let fallback = |q: &Page| exists(q).or_else(|| (q.space == "$2").then(|| page("$2", "@3")));
        p.retain(fallback);
        assert_eq!(p.last(), Some(&page("$1", "@1")), "not @3, shown now");
        // Nothing older: no last page.
        let mut q = Pages::default();
        q.visit(page("$1", "@1"));
        q.visit(page("$2", "@2"));
        q.visit(page("$1", "@1"));
        q.retain(|q: &Page| (q.space == "$1").then(|| q.clone()));
        assert_eq!(q.last(), None);
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
