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
}
