#[cfg(not(feature = "web-compat"))]
pub use crate::native_player::NativePlayer as GamePlayer;
#[cfg(feature = "web-compat")]
pub use crate::webview::GameWebView as GamePlayer;
use std::{path::PathBuf, sync::Arc};
use wgr_core::security::NetworkPolicy;
#[derive(Debug)]
pub enum PlayerEvent {
    Ready,
    Capabilities(serde_json::Value),
    PageCapabilities(serde_json::Value),
    Loaded,
    Report(serde_json::Value),
    Failure(String),
    Back,
    Fullscreen,
}
pub struct PlayerOptions {
    pub native_root: PathBuf,
    pub binary_path: Option<PathBuf>,
    pub entry: String,
    pub origin: String,
    pub loopback: Option<String>,
    pub policy: NetworkPolicy,
    pub profile: PathBuf,
    pub spool: PathBuf,
    pub fixture: bool,
    pub gta: bool,
    pub diagnostic: bool,
    pub cleanup_cache: bool,
    pub muted: bool,
    pub runtime: tokio::runtime::Handle,
    pub cache: Arc<wgr_core::assets::DiskCache>,
}
