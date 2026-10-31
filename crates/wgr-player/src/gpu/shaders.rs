//! Índice e packs de shaders, transcritos de `loadShaderIndex`, `packBytes` e
//! `packShader` do `wgpu_worker.js`.
//!
//! O índice lista os packs e, por hash, o intervalo `[pack, offset, bytes do
//! WGSL, bytes das constantes]`. Os packs são arquivos concatenados; este
//! módulo só extrai o texto, sem rede.

use anyhow::{Context, Result};
use serde::Deserialize;
use std::collections::HashMap;

#[derive(Debug, Clone, Deserialize)]
pub struct PackInfo {
    pub hashes: u32,
    pub bytes: u64,
    pub file: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ShaderRecord {
    pub stage: String,
    pub effect: String,
    pub program: String,
    #[serde(default)]
    pub ok: bool,
    /// `[pack, offset, bytes do WGSL, bytes das constantes]`.
    #[serde(default)]
    pub p: Vec<u32>,
    #[serde(default)]
    pub consts: bool,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub stage_failed: Option<String>,
}

impl ShaderRecord {
    pub fn label(&self) -> String {
        format!("{}::{}", self.effect, self.program)
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct ShaderIndex {
    #[serde(rename = "_packs", default)]
    pub packs: Vec<PackInfo>,
    #[serde(flatten)]
    pub shaders: HashMap<String, ShaderRecord>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShaderSource {
    pub text: String,
    pub constants: Option<String>,
}

impl ShaderIndex {
    pub fn parse(json: &str) -> Result<Self> {
        let index: Self = serde_json::from_str(json).context("Índice de shaders inválido")?;
        anyhow::ensure!(
            index.shaders.len() <= 200_000,
            "Índice de shaders excessivo"
        );
        Ok(index)
    }

    pub fn record(&self, hash: &str) -> Option<&ShaderRecord> {
        self.shaders.get(hash)
    }

    pub fn pack_file(&self, index: u32) -> Option<&str> {
        self.packs
            .get(index as usize)
            .map(|pack| pack.file.as_str())
    }

    /// Extrai o WGSL e as constantes de um pack já baixado.
    pub fn source_from_pack(&self, hash: &str, pack: &[u8]) -> Result<Option<ShaderSource>> {
        let Some(record) = self.record(hash) else {
            return Ok(None);
        };
        if !record.ok {
            return Ok(None);
        }
        if record.p.len() < 4 {
            return Ok(None);
        }
        let offset = record.p[1] as usize;
        let text_bytes = record.p[2] as usize;
        let const_bytes = record.p[3] as usize;
        let end = offset
            .checked_add(text_bytes)
            .and_then(|value| value.checked_add(const_bytes))
            .context("Intervalo de shader inválido")?;
        anyhow::ensure!(end <= pack.len(), "Pack de shader truncado");
        let text = std::str::from_utf8(&pack[offset..offset + text_bytes])
            .context("WGSL do pack não é UTF-8")?
            .to_string();
        let constants = if const_bytes > 0 {
            Some(
                std::str::from_utf8(&pack[offset + text_bytes..end])
                    .context("Constantes do pack não são UTF-8")?
                    .to_string(),
            )
        } else {
            None
        };
        Ok(Some(ShaderSource { text, constants }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const INDEX: &str = r#"{
      "_packs":[{"hashes":2,"bytes":64,"file":"pack0.test.bin"}],
      "aaaaaaaaaaaaaaaa":{"stage":"vs","effect":"ped","program":"VS_Ped","ok":true,"p":[0,4,10,0]},
      "bbbbbbbbbbbbbbbb":{"stage":"ps","effect":"ped","program":"PS_Ped","ok":true,"consts":true,"p":[0,14,5,7]},
      "cccccccccccccccc":{"stage":"ps","effect":"bad","program":"PS_Bad","ok":false,"error":"untranslated"}
    }"#;

    #[test]
    fn parses_packs_and_records() {
        let index = ShaderIndex::parse(INDEX).unwrap();
        assert_eq!(index.packs.len(), 1);
        assert_eq!(index.pack_file(0), Some("pack0.test.bin"));
        assert_eq!(index.pack_file(9), None);
        let record = index.record("aaaaaaaaaaaaaaaa").unwrap();
        assert!(record.ok);
        assert_eq!(record.stage, "vs");
        assert_eq!(record.label(), "ped::VS_Ped");
        assert_eq!(record.p, vec![0, 4, 10, 0]);
    }

    #[test]
    fn extracts_the_wgsl_and_constants_at_the_recorded_interval() {
        let index = ShaderIndex::parse(INDEX).unwrap();
        let mut pack = vec![b'#'; 64];
        pack[4..14].copy_from_slice(b"fn main(){");
        pack[14..19].copy_from_slice(b"[1,2]");
        pack[19..26].copy_from_slice(b"{\"a\":1}");
        let source = index
            .source_from_pack("aaaaaaaaaaaaaaaa", &pack)
            .unwrap()
            .unwrap();
        assert_eq!(source.text, "fn main(){");
        assert_eq!(source.constants, None);
        let with_consts = index
            .source_from_pack("bbbbbbbbbbbbbbbb", &pack)
            .unwrap()
            .unwrap();
        assert_eq!(with_consts.text, "[1,2]");
        assert_eq!(with_consts.constants.as_deref(), Some("{\"a\":1}"));
    }

    #[test]
    fn rejects_untranslated_or_truncated_shaders() {
        let index = ShaderIndex::parse(INDEX).unwrap();
        let pack = vec![0u8; 8];
        assert!(
            index
                .source_from_pack("cccccccccccccccc", &pack)
                .unwrap()
                .is_none()
        );
        assert!(index.source_from_pack("aaaaaaaaaaaaaaaa", &pack).is_err());
        assert!(
            index
                .source_from_pack("dddddddddddddddd", &pack)
                .unwrap()
                .is_none()
        );
    }
}
