//! Empacotamento de constant buffers, transcrito de `packUniforms` do
//! `wgpu_worker.js`. Todos os `var<uniform>` de um grupo viram um único
//! `cbpack_t` no binding 0, com um offset dinâmico por desenho. O binding 15
//! (constantes inline) fica de fora.

use regex::Regex;
use std::sync::LazyLock;

static UNIFORM_DECL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"@group\((\d+)u?\)\s*@binding\((\d+)u?\)\s*var<uniform>\s+(\w+)\s*:\s*(\w+)\s*;")
        .unwrap()
});
static ARRAY_N: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"array<[^>]*>,\s*(\d+)u?>").unwrap());
static FIRST_FN: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\n(?:@\w+|fn)\s").unwrap());

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackMember {
    pub binding: u32,
    pub size: u64,
    pub offset: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pack {
    pub members: Vec<PackMember>,
    pub size: u64,
}

/// Reescreve o WGSL e devolve o layout do buffer empacotado.
pub fn pack_uniforms(text: &str) -> (String, Option<Pack>) {
    struct Member {
        binding: u32,
        name: String,
        ty: String,
        size: u64,
    }

    let mut members: Vec<Member> = Vec::new();
    let mut group = 0u32;
    let out = UNIFORM_DECL.replace_all(text, |c: &regex::Captures| {
        let binding: u32 = c[2].parse().unwrap_or(0);
        if binding == 15 {
            return c[0].to_string();
        }
        group = c[1].parse().unwrap_or(0);
        let ty = c[4].to_string();
        let size = Regex::new(&format!(
            r"struct\s+{}\s*\{{([^}}]*)\}}",
            regex::escape(&ty)
        ))
        .ok()
        .and_then(|st| st.captures(text).map(|s| s[1].to_string()))
        .and_then(|body| {
            ARRAY_N
                .captures(&body)
                .map(|a| a[1].parse::<u64>().unwrap_or(1) * 16)
        })
        .unwrap_or(16);
        members.push(Member {
            binding,
            name: c[3].to_string(),
            ty,
            size,
        });
        String::new()
    });
    if members.is_empty() {
        return (text.to_string(), None);
    }
    let mut out = out.into_owned();
    members.sort_by_key(|m| m.binding);
    let mut offset = 0;
    let mut pack_members = Vec::with_capacity(members.len());
    for member in &members {
        pack_members.push(PackMember {
            binding: member.binding,
            size: member.size,
            offset,
        });
        offset += member.size;
    }
    for member in &members {
        let re = Regex::new(&format!(r"\b{}\b", regex::escape(&member.name))).unwrap();
        out = re
            .replace_all(&out, format!("cbpack_v.{}", member.name).as_str())
            .into_owned();
    }
    let packed = format!(
        "struct cbpack_t {{\n{}\n}}\n@group({}u) @binding(0u) var<uniform> cbpack_v : cbpack_t;\n",
        members
            .iter()
            .map(|member| format!("  {} : {},", member.name, member.ty))
            .collect::<Vec<_>>()
            .join("\n"),
        group
    );
    let out = match FIRST_FN.find(&out) {
        Some(at) => format!("{}\n{}{}", &out[..at.start()], packed, &out[at.start()..]),
        None => format!("{out}{packed}"),
    };
    (
        out,
        Some(Pack {
            members: pack_members,
            size: offset,
        }),
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BindingKind {
    Uniform,
    Storage,
    ReadOnlyStorage,
    Sampler,
    SamplerComparison,
    Texture,
    StorageTexture,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Binding {
    pub group: u32,
    pub binding: u32,
    pub name: String,
    pub kind: BindingKind,
    pub dim: String,
    pub sample: String,
    pub multisampled: bool,
    pub format: Option<String>,
    pub access: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShaderInfo {
    pub bindings: Vec<Binding>,
    pub outputs: Vec<u32>,
}

static DECL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"@group\((\d+)u?\)\s*@binding\((\d+)u?\)\s*var(?:<([^>]*)>)?\s+(\w+)\s*:\s*([^;]+);",
    )
    .unwrap()
});
static TEXTURE_TYPE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^texture_(\w+?)(?:<([^>]*)>)?$").unwrap());
static FRAGMENT_MAIN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"@fragment\s*fn\s+main\s*\((?:[^()]|\([^)]*\))*\)\s*->\s*([^{]+)\{").unwrap()
});
static LOCATION: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"@location\((\d+)u?\)").unwrap());

fn normalize_dim(dim: &str) -> String {
    match dim {
        "2d" => "2d".into(),
        "2d_array" => "2d-array".into(),
        "3d" => "3d".into(),
        "cube" => "cube".into(),
        "cube_array" => "cube-array".into(),
        "1d" => "1d".into(),
        other => other.into(),
    }
}

/// Interpreta bindings e saídas do fragment shader, como `parseWgsl`.
pub fn parse_wgsl(text: &str) -> ShaderInfo {
    let mut bindings = Vec::new();
    for captures in DECL.captures_iter(text) {
        let group = captures[1].parse().unwrap_or(0);
        let binding = captures[2].parse().unwrap_or(0);
        let space = captures
            .get(3)
            .map(|value| value.as_str().replace(char::is_whitespace, ""))
            .unwrap_or_default();
        let name = captures[4].to_string();
        let ty = captures[5].trim().to_string();
        let mut entry = Binding {
            group,
            binding,
            name,
            kind: BindingKind::Texture,
            dim: "2d".into(),
            sample: "float".into(),
            multisampled: false,
            format: None,
            access: None,
        };
        if space == "uniform" {
            entry.kind = BindingKind::Uniform;
        } else if space.starts_with("storage") {
            entry.kind = if space.contains("read_write") {
                BindingKind::Storage
            } else {
                BindingKind::ReadOnlyStorage
            };
        } else if ty == "sampler" {
            entry.kind = BindingKind::Sampler;
        } else if ty == "sampler_comparison" {
            entry.kind = BindingKind::SamplerComparison;
        } else {
            let texture = TEXTURE_TYPE.captures(&ty);
            let (name, argument) = texture
                .as_ref()
                .map(|captures| {
                    (
                        captures[1].to_string(),
                        captures
                            .get(2)
                            .map(|value| value.as_str().to_string())
                            .unwrap_or_else(|| "f32".into()),
                    )
                })
                .unwrap_or(("2d".into(), "f32".into()));
            if let Some(storage) = name.strip_prefix("storage_") {
                entry.kind = BindingKind::StorageTexture;
                entry.dim = normalize_dim(storage);
                entry.format = Some(argument.split(',').next().unwrap_or("").trim().to_string());
                entry.access = Some(if argument.contains("read_write") {
                    "read-write".into()
                } else if argument.contains("read") {
                    "read-only".into()
                } else {
                    "write-only".into()
                });
            } else if let Some(depth) = name.strip_prefix("depth_") {
                entry.dim = normalize_dim(depth);
                entry.sample = "depth".into();
            } else if let Some(rest) = name.strip_prefix("multisampled_") {
                entry.dim = normalize_dim(rest);
                entry.multisampled = true;
                entry.sample = "unfilterable-float".into();
            } else {
                entry.dim = normalize_dim(&name);
                entry.sample = match argument.as_str() {
                    "u32" => "uint".into(),
                    "i32" => "sint".into(),
                    _ => "float".into(),
                };
            }
            if entry.dim == "depth_2d" {
                entry.dim = "2d".into();
            }
        }
        bindings.push(entry);
    }
    let mut outputs = Vec::new();
    if let Some(signature) = FRAGMENT_MAIN.captures(text) {
        let result = signature[1].trim();
        if let Some(location) = LOCATION.captures(result) {
            outputs.push(location[1].parse().unwrap_or(0));
        } else if let Some(body) = Regex::new(&format!(
            r"struct\s+{}\s*\{{([^}}]*)\}}",
            regex::escape(result)
        ))
        .ok()
        .and_then(|structure| structure.captures(text))
        {
            for location in LOCATION.captures_iter(&body[1]) {
                outputs.push(location[1].parse().unwrap_or(0));
            }
        }
    }
    ShaderInfo { bindings, outputs }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHADER: &str = r#"
struct cb17_struct { tint : array<vec4<f32>, 17u>, }
struct cb4_struct { params : array<vec4<f32>, 4u>, }
@group(1u) @binding(2u) var<uniform> cb2_2 : cb17_struct;
@group(1u) @binding(5u) var<uniform> cb5_5 : cb4_struct;
fn main() {
  let a = cb2_2.tint[0];
  let b = cb5_5.params[1];
}
"#;

    #[test]
    fn packs_every_uniform_into_binding_zero_and_preserves_offsets() {
        let (packed, pack) = pack_uniforms(SHADER);
        let pack = pack.expect("uniform pack");
        assert_eq!(
            pack.members,
            vec![
                PackMember {
                    binding: 2,
                    size: 17 * 16,
                    offset: 0
                },
                PackMember {
                    binding: 5,
                    size: 4 * 16,
                    offset: 17 * 16
                },
            ]
        );
        assert_eq!(pack.size, 21 * 16);
        assert!(packed.contains("cbpack_v.cb2_2"));
        assert!(packed.contains("cbpack_v.cb5_5"));
        assert!(!packed.contains("var<uniform> cb2_2"));
        assert!(packed.contains("@group(1u) @binding(0u) var<uniform> cbpack_v"));
    }

    #[test]
    fn keeps_inline_constants_at_binding_fifteen() {
        let shader = r#"
struct cb1 { x : array<vec4<f32>, 1u>, }
@group(0u) @binding(15u) var<uniform> inline_cb : cb1;
fn main() { let v = inline_cb.x[0]; }
"#;
        let (packed, pack) = pack_uniforms(shader);
        assert!(pack.is_none());
        assert!(packed.contains("var<uniform> inline_cb"));
    }

    #[test]
    fn leaves_shaders_without_uniforms_untouched() {
        let shader = "fn main() {}\n";
        let (packed, pack) = pack_uniforms(shader);
        assert!(pack.is_none());
        assert_eq!(packed, shader);
    }

    const BINDINGS: &str = r#"
struct cbpack_t { a : vec4<f32>, }
@group(1u) @binding(0u) var<uniform> cbpack_v : cbpack_t;
@group(1u) @binding(16u) var s0 : sampler;
@group(1u) @binding(17u) var sc : sampler_comparison;
@group(1u) @binding(32u) var t0 : texture_2d<f32>;
@group(1u) @binding(33u) var t1 : texture_2d_array<u32>;
@group(1u) @binding(34u) var t2 : texture_depth_2d;
@group(1u) @binding(35u) var t3 : texture_multisampled_2d<f32>;
@group(1u) @binding(36u) var t4 : texture_storage_2d<rgba8unorm, write>;
@group(0u) @binding(0u) var<storage, read_write> sb : array<u32>;
@group(0u) @binding(1u) var<storage, read> rb : array<u32>;
struct FragmentOutput { @location(0u) color : vec4<f32>, @location(1u) normal : vec4<f32>, }
@fragment fn main(@location(1u) uv : vec4<f32>) -> FragmentOutput {
  return FragmentOutput(vec4<f32>(1.0), vec4<f32>(0.0));
}
"#;

    #[test]
    fn parses_bindings_with_the_same_kinds_and_dimensions_as_the_worker() {
        let info = parse_wgsl(BINDINGS);
        assert_eq!(info.bindings.len(), 10);
        let by_binding = |binding: u32| {
            info.bindings
                .iter()
                .find(|entry| entry.binding == binding)
                .unwrap()
                .clone()
        };
        assert_eq!(by_binding(0).kind, BindingKind::Uniform);
        assert_eq!(by_binding(16).kind, BindingKind::Sampler);
        assert_eq!(by_binding(17).kind, BindingKind::SamplerComparison);
        let texture = by_binding(32);
        assert_eq!(texture.kind, BindingKind::Texture);
        assert_eq!(texture.dim, "2d");
        assert_eq!(texture.sample, "float");
        let array = by_binding(33);
        assert_eq!(array.dim, "2d-array");
        assert_eq!(array.sample, "uint");
        let depth = by_binding(34);
        assert_eq!(depth.sample, "depth");
        let multisampled = by_binding(35);
        assert!(multisampled.multisampled);
        assert_eq!(multisampled.sample, "unfilterable-float");
        let storage = by_binding(36);
        assert_eq!(storage.kind, BindingKind::StorageTexture);
        assert_eq!(storage.format.as_deref(), Some("rgba8unorm"));
        assert_eq!(storage.access.as_deref(), Some("write-only"));
        let read_write = info
            .bindings
            .iter()
            .find(|entry| entry.group == 0 && entry.binding == 0)
            .unwrap();
        assert_eq!(read_write.kind, BindingKind::Storage);
        let read_only = info
            .bindings
            .iter()
            .find(|entry| entry.group == 0 && entry.binding == 1)
            .unwrap();
        assert_eq!(read_only.kind, BindingKind::ReadOnlyStorage);
    }

    #[test]
    fn parses_fragment_outputs_from_the_returned_struct() {
        let info = parse_wgsl(BINDINGS);
        assert_eq!(info.outputs, vec![0, 1]);
    }

    #[test]
    fn parses_a_direct_location_return() {
        let info =
            parse_wgsl("@fragment fn main() -> @location(0u) vec4<f32> { return vec4<f32>(1.0); }");
        assert_eq!(info.outputs, vec![0]);
    }
}
