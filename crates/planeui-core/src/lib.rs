//! Platform-independent core of PlaneUI: loading, editing, validating, and writing
//! `aerospace.toml` without touching content PlaneUI does not own.

pub mod aerospace;
pub mod apply;
pub mod commands;
pub mod config_doc;
pub mod config_view;
pub mod history;
pub mod keys;
pub mod store;
pub mod validate;
