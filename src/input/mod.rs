//! Turning outer-terminal input into terminal input or paddock commands.

pub mod keymap;
pub mod keys;
pub mod mouse;
pub mod paste;
pub mod prefix;

pub use keys::encode_key;
pub use mouse::{route as route_mouse, MouseAction};
pub use paste::encode_paste;
pub use prefix::{Command, Direction, KeyAction, PrefixKeys};
