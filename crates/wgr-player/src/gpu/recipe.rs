//! Receitas de pipeline por conteúdo, transcritas de `pipelineRecipe` e
//! `recipeKey` do `wgpu_worker.js`.
//!
//! A chave é o JSON `[vs, ps, el, st, tp, sf, b, d, r, rt, n, ds, ro]`, o mesmo
//! usado pelo seed `/shaders/pipelines.json`. Assim o player nativo pode
//! compilar os pipelines adiantado com o mesmo arquivo que o navegador usa.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// Elemento do layout de entrada: semântica, índice, formato, slot, offset,
/// classe e taxa de instância (os sete campos do worker web).
pub type Element = (u32, u32, u32, u32, u32, u32, u32);

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PipelineRecipe {
    pub vs: String,
    pub ps: String,
    pub el: Vec<Element>,
    pub st: Vec<u32>,
    pub tp: u32,
    pub sf: String,
    pub b: Option<Vec<u32>>,
    pub d: Option<Vec<u32>>,
    pub r: Option<Vec<u32>>,
    pub rt: Vec<Option<String>>,
    pub n: u32,
    pub ds: String,
    pub ro: u32,
}

impl PipelineRecipe {
    /// Mesma ordem de campos de `recipeKey`; serve como chave estável do cache
    /// e como linha do seed.
    pub fn key(&self) -> String {
        serde_json::to_string(&(
            &self.vs, &self.ps, &self.el, &self.st, self.tp, &self.sf, &self.b, &self.d, &self.r,
            &self.rt, self.n, &self.ds, self.ro,
        ))
        .expect("receita sempre serializável")
    }

    pub fn has_depth(&self) -> bool {
        self.ro & 1 != 0
    }

    pub fn has_stencil(&self) -> bool {
        self.ro & 2 != 0
    }
}

/// Lê o seed `pipelines.json` (uma lista de receitas).
pub fn parse_seed(json: &str) -> Result<Vec<PipelineRecipe>> {
    let recipes: Vec<PipelineRecipe> =
        serde_json::from_str(json).context("Seed de pipelines inválido")?;
    anyhow::ensure!(recipes.len() <= 100_000, "Seed de pipelines excessivo");
    Ok(recipes)
}

/// Lê o cache local em JSONL (uma receita por linha), ignorando linhas vazias.
pub fn parse_jsonl(text: &str) -> Result<Vec<PipelineRecipe>> {
    let mut recipes = Vec::new();
    for line in text.lines().filter(|line| !line.trim().is_empty()) {
        recipes.push(serde_json::from_str(line).context("Receita local inválida")?);
    }
    Ok(recipes)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"{"vs":"028921879df627f5","ps":"943b0ad9542193ff","el":[[2013849738,0,6,0,0,0,0],[1009949074,0,6,0,12,0,0]],"st":[36],"tp":4,"sf":"","b":[0,0,1,5,6,1,5,6,1,15,0,2,1,1,2,1],"d":[0,0,8,0,65535,1,1,1,8,1,1,1,8],"r":[3,3,1,0,0,0,1,1,0,0],"rt":["bgra8unorm"],"n":1,"ds":"","ro":0}"#;

    #[test]
    fn parses_the_same_shape_as_the_web_seed() {
        let recipe: PipelineRecipe = serde_json::from_str(SAMPLE).unwrap();
        assert_eq!(recipe.vs, "028921879df627f5");
        assert_eq!(recipe.el.len(), 2);
        assert_eq!(recipe.st, vec![36]);
        assert_eq!(recipe.rt, vec![Some("bgra8unorm".into())]);
        assert!(!recipe.has_depth());
    }

    #[test]
    fn key_uses_the_recipe_field_order() {
        let recipe: PipelineRecipe = serde_json::from_str(SAMPLE).unwrap();
        let key = recipe.key();
        assert!(key.starts_with(r#"["028921879df627f5","943b0ad9542193ff","#));
        assert!(key.ends_with(r#",1,"",0]"#));
        let round_trip: PipelineRecipe = serde_json::from_str(SAMPLE).unwrap();
        assert_eq!(round_trip.key(), key);
    }

    #[test]
    fn reads_a_jsonl_cache_with_blank_lines() {
        let text = format!("{SAMPLE}\n\n{SAMPLE}\n");
        assert_eq!(parse_jsonl(&text).unwrap().len(), 2);
    }

    #[test]
    fn seed_parsing_accepts_a_list() {
        let seed = format!("[{SAMPLE},{SAMPLE}]");
        assert_eq!(parse_seed(&seed).unwrap().len(), 2);
    }
}
