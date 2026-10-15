//! Ashdash core: query engine, workflows and data stores. No UI.

pub mod apps;
pub mod brand;
pub mod calc;
pub mod clipboard;
pub mod config;
pub mod engine;
pub mod files;
pub mod fuzzy;
pub mod item;
pub mod system;
pub mod theme;
pub mod usage;
pub mod websearch;
pub mod workflow;

pub use config::{Config, Paths};
pub use engine::{Engine, Outcome};
pub use item::{Action, Icon, Item, Mods};
