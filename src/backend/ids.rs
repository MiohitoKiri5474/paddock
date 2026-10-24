//! The ids the backend gives spaces, tabs and terminals. Their spelling is
//! the backend's (tmux writes `$N`, `@N` and `%N`); paddock only compares
//! them, shows them in errors, and keeps a space's in `layout.toml` and a
//! terminal's in the hook event log, which is why they read back from text.

use serde::{Deserialize, Serialize};
use std::borrow::Borrow;
use std::fmt;
use std::ops::Deref;

macro_rules! id {
    ($(#[$doc:meta])* $name:ident) => {
        $(#[$doc])*
        #[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            /// The id as the backend spelled it.
            pub(super) fn new(id: impl Into<String>) -> Self {
                Self(id.into())
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl Deref for $name {
            type Target = str;

            fn deref(&self) -> &str {
                &self.0
            }
        }

        /// So maps keyed by id answer for the text of one, as stored.
        impl Borrow<str> for $name {
            fn borrow(&self) -> &str {
                &self.0
            }
        }

        impl PartialEq<str> for $name {
            fn eq(&self, other: &str) -> bool {
                self.0 == other
            }
        }

        impl PartialEq<&str> for $name {
            fn eq(&self, other: &&str) -> bool {
                self.0 == *other
            }
        }

        /// Rows keep ids as text (empty for an external session).
        impl PartialEq<String> for $name {
            fn eq(&self, other: &String) -> bool {
                self.0 == *other
            }
        }

        impl PartialEq<$name> for String {
            fn eq(&self, other: &$name) -> bool {
                *self == other.0
            }
        }

        impl PartialEq<$name> for &str {
            fn eq(&self, other: &$name) -> bool {
                *self == other.0
            }
        }

        #[cfg(test)]
        impl From<&str> for $name {
            fn from(id: &str) -> Self {
                Self::new(id)
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }
    };
}

id!(
    /// A space's id, stable while the space runs.
    SpaceId
);
id!(
    /// A tab's or a float's id: both are pages of a space.
    TabId
);
id!(
    /// A terminal's id, stable while the terminal runs.
    TerminalId
);

impl TerminalId {
    /// The terminal a hook named (`$TMUX_PANE`), as the event log, the
    /// sessions and their rows keep it: the one place a terminal's id is
    /// read back from text outside the backend.
    pub fn from_hook(pane: &str) -> Self {
        Self(pane.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn an_id_reads_back_as_the_text_it_was_written_as() {
        let id = SpaceId::new("$4");
        assert_eq!(serde_json::to_string(&id).unwrap(), "\"$4\"");
        assert_eq!(serde_json::from_str::<SpaceId>("\"$4\"").unwrap(), id);
        assert_eq!(id.to_string(), "$4");
    }

    #[test]
    fn a_map_keyed_by_terminal_answers_for_the_text_a_hook_gave() {
        let map = HashMap::from([(TerminalId::new("%3"), 1)]);
        assert_eq!(map.get("%3"), Some(&1));
        assert_eq!(TerminalId::from_hook("%3"), TerminalId::new("%3"));
    }
}
