//! Ashdash core: query engine, workflows and data stores. No UI.

pub mod actions;
pub mod apps;
pub mod bookmarks;
pub mod brand;
pub mod calc;
pub mod clipboard;
pub mod config;
pub mod confine;
pub mod engine;
pub mod files;
pub mod fuzzy;
pub mod index;
pub mod item;
pub mod providers;
pub mod settings_view;
pub mod system;
pub mod theme;
pub mod usage;
pub mod websearch;
pub mod workflow;

pub use config::{Config, Paths};
pub use engine::{Engine, Outcome};
pub use item::{Action, Icon, Item, Mods};
