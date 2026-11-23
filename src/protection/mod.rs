pub mod invites;
pub mod logic;
pub mod store;

pub const EMOJI_PROTECTION: &str = "🚨";
pub const EMOJI_RAID: &str = "🌊";
pub const EMOJI_ALTS: &str = "🕵️";
pub const EMOJI_GATE: &str = "🐣";
pub const EMOJI_NUKE: &str = "💣";

/// In-memory protection state, shared through `Data`.
#[derive(Default)]
pub struct Runtime {
    pub invites: invites::InviteCache,
    pub joins: std::sync::Mutex<std::collections::HashMap<u64, logic::JoinWindow>>,
    pub nuke: logic::NukeCounter,
    pub alert_edit: std::sync::Mutex<std::collections::HashMap<u64, std::time::Instant>>,
}

impl Runtime {
    pub fn new() -> Self {
        Self::default()
    }
}
