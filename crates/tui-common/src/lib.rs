//! The parts both applications in this workspace need.
//!
//! Everything here was duplicated between them before the move, line for line
//! in most cases. What stayed behind in each app is the part that genuinely
//! differs: its state, its key bindings, its screens, and the loop that maps
//! its own actions onto them.

pub mod config;
pub mod events;
pub mod http;
pub mod layout;
pub mod terminal;
