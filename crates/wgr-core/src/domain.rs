use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};
use uuid::Uuid;

pub fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceKind {
    RemoteWeb,
    RemoteCatalogApi,
    LocalWebBundle,
    GtaLegacyZip,
}
impl SourceKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::RemoteWeb => "Link do jogo",
            Self::RemoteCatalogApi => "API de biblioteca",
            Self::LocalWebBundle => "Pacote local",
            Self::GtaLegacyZip => "GTA · adaptador legado",
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AuthScheme {
    #[default]
    None,
    Bearer,
    Header {
        name: String,
    },
    Basic,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Source {
    pub id: Uuid,
    pub kind: SourceKind,
    pub display_name: String,
    pub location: String,
    pub auth: AuthScheme,
    pub credential_ref: Option<String>,
    pub allowed_origins: Vec<String>,
    pub development_loopback: bool,
    pub last_sync_at: Option<i64>,
    pub status: String,
    pub created_at: i64,
    #[serde(default)]
    pub favorite: bool,
}
impl Source {
    pub fn new(kind: SourceKind, name: &str, location: &str) -> Self {
        Self {
            id: Uuid::new_v4(),
            kind,
            display_name: name.into(),
            location: location.into(),
            auth: AuthScheme::None,
            credential_ref: None,
            allowed_origins: vec![],
            development_loopback: false,
            last_sync_at: None,
            status: "Não testado".into(),
            created_at: now(),
            favorite: false,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Requirements {
    #[serde(default)]
    pub webgpu: bool,
    #[serde(default)]
    pub shared_array_buffer: bool,
    #[serde(default)]
    pub wasm_memory64: bool,
    #[serde(default)]
    pub min_memory_gib: Option<f64>,
    #[serde(flatten)]
    pub unknown: BTreeMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Game {
    pub id: Uuid,
    pub source_id: Uuid,
    pub external_id: Option<String>,
    pub title: String,
    pub description: String,
    pub entry_uri: String,
    pub cover_uri: Option<String>,
    pub version: Option<String>,
    pub requirements: Requirements,
    pub compat_status: String,
    pub favorite: bool,
    pub play_count: u32,
    pub last_played_at: Option<i64>,
    pub created_at: i64,
    pub local_bundle_path: Option<PathBuf>,
    pub profile: GameProfile,
}
impl Game {
    pub fn new(source: &Source, title: &str, entry: &str) -> Self {
        Self {
            id: Uuid::new_v4(),
            source_id: source.id,
            external_id: None,
            title: title.into(),
            description: String::new(),
            entry_uri: entry.into(),
            cover_uri: None,
            version: None,
            requirements: Requirements::default(),
            compat_status: "Não testado".into(),
            favorite: false,
            play_count: 0,
            last_played_at: None,
            created_at: now(),
            local_bundle_path: None,
            profile: GameProfile::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GameProfile {
    pub fullscreen: bool,
    pub muted: bool,
    pub safe_mode: bool,
}
impl Default for GameProfile {
    fn default() -> Self {
        Self {
            fullscreen: true,
            muted: false,
            safe_mode: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    pub cache_limit_mib: u64,
    pub filter: String,
    pub sort: String,
    pub fullscreen: bool,
    pub telemetry: bool,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            cache_limit_mib: 64,
            filter: "Todos".into(),
            sort: "manual".into(),
            fullscreen: false,
            telemetry: false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SessionState {
    Idle,
    Resolving,
    Checking,
    Preparing,
    Loading,
    Running,
    Paused,
    Stopping,
    Stopped,
    Failure,
    Crashed,
    Cleanup,
}
impl SessionState {
    pub fn transition(&mut self, next: Self) -> anyhow::Result<()> {
        use SessionState::*;
        let valid = matches!(
            (*self, next),
            (Idle, Resolving)
                | (Resolving, Checking)
                | (Checking, Preparing)
                | (Preparing, Loading)
                | (Loading, Running)
                | (Running, Paused)
                | (Paused, Running)
                | (Stopping, Stopped)
                | (Stopped, Idle)
                | (Crashed, Cleanup)
                | (Failure, Cleanup)
                | (Cleanup, Idle)
        ) || (matches!(next, Failure | Crashed | Stopping)
            && !matches!(*self, Idle | Stopped | Cleanup));
        anyhow::ensure!(valid, "Transição de sessão inválida: {self:?} → {next:?}");
        *self = next;
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub id: Uuid,
    pub game_id: Uuid,
    pub start_at: i64,
    pub end_at: Option<i64>,
    pub state: SessionState,
    pub reason: Option<String>,
    pub metrics: serde_json::Value,
}
impl Session {
    pub fn new(game_id: Uuid) -> Self {
        Self {
            id: Uuid::new_v4(),
            game_id,
            start_at: now(),
            end_at: None,
            state: SessionState::Idle,
            reason: None,
            metrics: serde_json::json!({}),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Paths {
    pub root: PathBuf,
    pub database: PathBuf,
    pub cache: PathBuf,
    pub profiles: PathBuf,
    pub bundles: PathBuf,
    pub logs: PathBuf,
}
impl Paths {
    pub fn at(root: PathBuf) -> anyhow::Result<Self> {
        let paths = Self {
            database: root.join("data/wgr.sqlite"),
            cache: root.join("cache"),
            profiles: root.join("profiles"),
            bundles: root.join("bundles"),
            logs: root.join("logs"),
            root,
        };
        for p in [
            &paths.cache,
            &paths.profiles,
            &paths.bundles,
            &paths.logs,
            &paths.root.join("data"),
        ] {
            std::fs::create_dir_all(p)?;
        }
        Ok(paths)
    }
}

#[cfg(test)]
mod tests {
    use super::SessionState;
    #[test]
    fn session_never_reports_running_without_loading() {
        let mut state = SessionState::Idle;
        assert!(state.transition(SessionState::Running).is_err());
        for next in [
            SessionState::Resolving,
            SessionState::Checking,
            SessionState::Preparing,
            SessionState::Loading,
            SessionState::Running,
            SessionState::Stopping,
            SessionState::Stopped,
            SessionState::Idle,
        ] {
            state.transition(next).expect("transição");
        }
    }
}
