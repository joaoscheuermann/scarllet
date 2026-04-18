//! Reusable TUI widgets for the chat interface.
//!
//! [`ChatMessageWidget`] renders one top-level chat entry (user or
//! agent subtree), and [`ScrollView`] hosts the vertically scrollable
//! list that composes them into the chat pane. [`markdown`] exposes
//! the GFM-capable renderer used by chat message bodies.

pub mod chat_message;
pub mod markdown;
pub mod scroll_view;

pub use chat_message::ChatMessageWidget;
pub use scroll_view::{ScrollItem, ScrollView, ScrollViewState};
