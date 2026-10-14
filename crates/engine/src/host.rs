//! Commands the host application adds to the engine (Shorts Factory's `studio.*`: the channel
//! library, publishing, AI and downloads). They live above the engine's layer, so the engine only
//! keeps a registry: the app registers them before the first command is looked up, and from then
//! on they are commands like any other — in menus' dispatch, the control channel and MCP.

use std::sync::{Mutex, PoisonError};

use crate::commands::CommandSpec;

static HOST: Mutex<Vec<CommandSpec>> = Mutex::new(Vec::new());

/// Adds host commands. Call before the first [`crate::commands::command_specs`] (at start-up):
/// commands registered later are not seen.
pub fn register(specs: impl IntoIterator<Item = CommandSpec>) {
    HOST.lock().unwrap_or_else(PoisonError::into_inner).extend(specs);
}

pub(crate) fn specs() -> Vec<CommandSpec> {
    HOST.lock().unwrap_or_else(PoisonError::into_inner).clone()
}
