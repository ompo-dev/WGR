// GPU-only fallback for DXGI_R16G16B16A16_UNORM render targets.
// Browser Dawn exposes texture-formats-tier1, whereas the native wgpu 30
// device cannot render directly into rgba16unorm. The render target is
// rgba32float; each float below is the normalized value of one 16-bit word
// in a BC1 compressed block. Quantize exactly, two words per u32.
struct PackInfo {
    src_origin: vec4<u32>,
    layout: vec4<u32>,
};
@group(0) @binding(0) var source: texture_2d<f32>;
@group(0) @binding(1) var<storage, read_write> blocks: array<u32>;
@group(0) @binding(2) var<uniform> info: PackInfo;

@compute @workgroup_size(8, 8, 1)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    if (gid.x >= info.layout.x || gid.y >= info.layout.y) {
        return;
    }
    let xy = vec2<i32>(info.src_origin.xy + gid.xy);
    let value = textureLoad(source, xy, 0);
    let words = vec4<u32>(floor(clamp(value, vec4<f32>(0.0), vec4<f32>(1.0)) * 65535.0 + 0.5));
    let offset = gid.y * info.layout.z + gid.x * 2u;
    blocks[offset] = words.x | (words.y << 16u);
    blocks[offset + 1u] = words.z | (words.w << 16u);
}
