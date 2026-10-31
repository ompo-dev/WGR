//! Montagem da linha de comando da engine, transcrita de `play.html` e de
//! `main.rs` do host nativo. O objetivo é que a engine receba exatamente as
//! mesmas capacidades e o mesmo tamanho de canvas que no web port.

use anyhow::{Context, Result};
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct Port {
    pub format: String,
    pub id: String,
    pub abi: String,
    pub binary: PortBinary,
    pub assets: PortAssets,
    pub shaders: PortShaders,
    pub entry: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PortBinary {
    pub url: String,
    pub sha256: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PortAssets {
    pub manifest: String,
    pub base: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PortShaders {
    pub base: String,
}

impl Port {
    pub fn parse(json: &str) -> Result<Self> {
        let port: Port = serde_json::from_str(json).context("Descritor do port inválido")?;
        anyhow::ensure!(port.format == "wgr.port.v1", "Formato de port desconhecido");
        anyhow::ensure!(
            port.assets.base.starts_with("https://"),
            "Origem de assets sem HTTPS"
        );
        anyhow::ensure!(
            port.shaders.base.starts_with("https://"),
            "Origem de shaders sem HTTPS"
        );
        Ok(port)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameLimit {
    Thirty,
    Sixty,
    Uncapped,
}

impl FrameLimit {
    fn argument(self) -> &'static str {
        match self {
            Self::Thirty => "-frameLimit=2",
            Self::Sixty => "-frameLimit=1",
            Self::Uncapped => "-frameLimit=0",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartMode {
    Menu,
    Story,
    Sandbox,
}

impl StartMode {
    fn word(self) -> i32 {
        match self {
            Self::Menu => 0,
            Self::Story => 1,
            Self::Sandbox => 2,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Launch {
    /// Buffer interno em pixels físicos, como o `wantSize()` do play.html.
    pub width: u32,
    pub height: u32,
    pub frame_limit: FrameLimit,
    pub start_mode: StartMode,
    /// Núcleos informados à engine; o web só limita quando são dois ou menos.
    pub cores: usize,
    /// Perfil de pouca memória do play.html.
    pub low_memory: bool,
    /// `?shadows=1` do play.html: mantém sombras no nível mínimo.
    pub shadows: bool,
    /// `?verbose=1`: não silencia os canais de script e replay.
    pub verbose: bool,
}

impl Launch {
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            frame_limit: FrameLimit::Sixty,
            start_mode: StartMode::Sandbox,
            cores: 0,
            low_memory: false,
            shadows: false,
            verbose: false,
        }
    }
}

const LOW_MEMORY: &[&str] = &[
    "-textureQuality=0",
    "-pedVariety=0",
    "-vehicleVariety=0",
    "-reflectionQuality=0",
    "-particleQuality=0",
    "-grassQuality=0",
    "-cityDensity=0",
    "-lodScale=0",
    "-pedLodBias=0",
    "-vehicleLodBias=0",
];

pub fn engine_args(launch: &Launch) -> Vec<String> {
    let mut args: Vec<String> = [
        "./this.program",
        "-rootdir=/game/",
        "-nodisplaycalibration",
        "-noSocialClub",
        "-pc:nosocialclub",
        "-nonetwork",
        "-output",
        "-wgpu",
        "-fastscriptcore",
        "-forceResolution",
    ]
    .into_iter()
    .map(str::to_string)
    .collect();
    args.push(format!("-width={}", launch.width));
    args.push(format!("-height={}", launch.height));
    if !launch.verbose {
        args.push("-Script_all=warning".into());
        args.push("-replay_all=warning".into());
    }
    args.push(launch.frame_limit.argument().into());
    if launch.cores > 0 && launch.cores <= 2 {
        args.push(format!("-numCores={}", launch.cores));
    }
    if launch.low_memory {
        args.extend(LOW_MEMORY.iter().map(|arg| (*arg).to_string()));
        args.push(if launch.shadows {
            "-shadowQuality=0".into()
        } else {
            "-shadowQuality=-1".into()
        });
    }
    if launch.start_mode != StartMode::Menu {
        args.push(format!("-wasmstartchoice={}", launch.start_mode.word()));
    }
    args
}

#[cfg(test)]
mod tests {
    use super::*;

    const PORT: &str = include_str!("../../../ports/fiveplay-gta5.port.json");

    #[test]
    fn parses_the_bundled_port_descriptor() {
        let port = Port::parse(PORT).unwrap();
        assert_eq!(port.format, "wgr.port.v1");
        assert_eq!(port.assets.base, "https://fiveplay.lol/data/");
        assert!(port.binary.url.starts_with("https://cdn.fiveplay.lol/"));
    }

    #[test]
    fn passes_the_same_canvas_and_capabilities_as_the_web_port() {
        let mut launch = Launch::new(1920, 1200);
        launch.frame_limit = FrameLimit::Uncapped;
        launch.cores = 2;
        let args = engine_args(&launch);
        for expected in [
            "-forceResolution",
            "-width=1920",
            "-height=1200",
            "-wgpu",
            "-fastscriptcore",
            "-frameLimit=0",
            "-numCores=2",
            "-wasmstartchoice=2",
        ] {
            assert!(args.iter().any(|arg| arg == expected), "faltou {expected}");
        }
        assert!(!args.iter().any(|arg| arg == "-shadowQuality=-1"));
    }

    #[test]
    fn low_memory_profile_matches_the_web_page() {
        let mut launch = Launch::new(1280, 720);
        launch.low_memory = true;
        launch.verbose = true;
        let args = engine_args(&launch);
        assert!(args.iter().any(|arg| arg == "-shadowQuality=-1"));
        assert!(args.iter().any(|arg| arg == "-textureQuality=0"));
        assert!(!args.iter().any(|arg| arg == "-Script_all=warning"));
    }
}
