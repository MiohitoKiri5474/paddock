//! The tabs of the session view and which one is current. Generic over what a
//! tab holds so the rules can be tested without tmux.

pub struct Tab<T> {
    pub name: String,
    pub content: T,
}

pub struct Tabs<T> {
    tabs: Vec<Tab<T>>,
    current: usize,
}

impl<T> Default for Tabs<T> {
    fn default() -> Self {
        Self {
            tabs: Vec::new(),
            current: 0,
        }
    }
}

impl<T> Tabs<T> {
    pub fn is_empty(&self) -> bool {
        self.tabs.is_empty()
    }

    pub fn current_index(&self) -> usize {
        self.current
    }

    pub fn current(&self) -> Option<&Tab<T>> {
        self.tabs.get(self.current)
    }

    pub fn current_mut(&mut self) -> Option<&mut Tab<T>> {
        self.tabs.get_mut(self.current)
    }

    pub fn iter(&self) -> impl Iterator<Item = &Tab<T>> {
        self.tabs.iter()
    }

    /// Add a tab at the end. It does not become current.
    pub fn push(&mut self, name: impl Into<String>, content: T) {
        self.tabs.push(Tab {
            name: name.into(),
            content,
        });
    }

    /// Make tab `index` current; out-of-range indexes are ignored.
    pub fn select(&mut self, index: usize) -> bool {
        let ok = index < self.tabs.len();
        if ok {
            self.current = index;
        }
        ok
    }

    pub fn select_last(&mut self) {
        self.current = self.tabs.len().saturating_sub(1);
    }

    /// Next tab, wrapping around.
    pub fn next(&mut self) {
        if !self.tabs.is_empty() {
            self.current = (self.current + 1) % self.tabs.len();
        }
    }

    /// Previous tab, wrapping around.
    pub fn previous(&mut self) {
        if !self.tabs.is_empty() {
            self.current = (self.current + self.tabs.len() - 1) % self.tabs.len();
        }
    }

    pub fn position(&self, pred: impl Fn(&T) -> bool) -> Option<usize> {
        self.tabs.iter().position(|t| pred(&t.content))
    }

    pub fn find_mut(&mut self, pred: impl Fn(&T) -> bool) -> Option<&mut Tab<T>> {
        self.tabs.iter_mut().find(|t| pred(&t.content))
    }

    /// Remove tab `index`. The current tab stays current; if it was the one
    /// removed, the tab that took its place (or the new last tab) is current.
    pub fn remove(&mut self, index: usize) -> Option<Tab<T>> {
        if index >= self.tabs.len() {
            return None;
        }
        let tab = self.tabs.remove(index);
        if index < self.current || self.current >= self.tabs.len() {
            self.current = self.current.saturating_sub(1);
        }
        Some(tab)
    }
}

impl<T> IntoIterator for Tabs<T> {
    type Item = Tab<T>;
    type IntoIter = std::vec::IntoIter<Tab<T>>;

    fn into_iter(self) -> Self::IntoIter {
        self.tabs.into_iter()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tabs(n: usize) -> Tabs<usize> {
        let mut t = Tabs::default();
        for i in 0..n {
            t.push(format!("t{i}"), i);
        }
        t
    }

    fn current(t: &Tabs<usize>) -> Option<usize> {
        t.current().map(|t| t.content)
    }

    #[test]
    fn next_and_previous_wrap() {
        let mut t = tabs(3);
        t.previous();
        assert_eq!(current(&t), Some(2));
        t.next();
        assert_eq!(current(&t), Some(0));
        t.next();
        assert_eq!(current(&t), Some(1));
    }

    #[test]
    fn select_ignores_missing_tabs() {
        let mut t = tabs(2);
        assert!(t.select(1));
        assert!(!t.select(5));
        assert_eq!(current(&t), Some(1));
        t.push("new", 9);
        t.select_last();
        assert_eq!(current(&t), Some(9));
    }

    #[test]
    fn removing_keeps_the_current_tab_when_possible() {
        let mut t = tabs(4);
        t.select(2);
        t.remove(0);
        assert_eq!(current(&t), Some(2), "earlier tab removed");
        t.remove(2);
        assert_eq!(current(&t), Some(2), "later tab removed");
        t.remove(t.position(|&c| c == 2).unwrap());
        assert_eq!(current(&t), Some(1), "current last tab removed");
        t.remove(0);
        assert!(t.is_empty());
    }

    #[test]
    fn removing_the_current_middle_tab_selects_its_successor() {
        let mut t = tabs(3);
        t.select(1);
        t.remove(1);
        assert_eq!(current(&t), Some(2));
        assert_eq!(t.iter().count(), 2);
    }

    #[test]
    fn empty_tabs_are_safe() {
        let mut t: Tabs<usize> = Tabs::default();
        t.next();
        t.previous();
        assert!(t.current().is_none());
        assert!(t.remove(0).is_none());
    }
}
