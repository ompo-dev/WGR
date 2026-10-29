// WGR native presentation: fixed game buffer, resizable aspect-preserving surface.
@group(0) @binding(0) var src: texture_2d<f32>;
struct VertexOut {
    @builtin(position) position: vec4f,
    @location(0) uv: vec2f,
}
@vertex fn vs(@builtin(vertex_index) i: u32) -> VertexOut {
    let p = array<vec2f, 3>(vec2f(-1, -1), vec2f(3, -1), vec2f(-1, 3));
    return VertexOut(vec4f(p[i], 0, 1), vec2f(p[i].x * 0.5 + 0.5, 0.5 - p[i].y * 0.5));
}
fn pixel(p: vec2i, dims: vec2i) -> vec4f {
    return textureLoad(src, clamp(p, vec2i(0), dims - vec2i(1)), 0);
}
@fragment fn fs(v: VertexOut) -> @location(0) vec4f {
    let dims = vec2i(textureDimensions(src));
    let p = v.uv * vec2f(dims) - vec2f(0.5);
    let base = vec2i(floor(p));
    let f = fract(p);
    return mix(mix(pixel(base, dims), pixel(base + vec2i(1, 0), dims), f.x),
               mix(pixel(base + vec2i(0, 1), dims), pixel(base + vec2i(1), dims), f.x), f.y);
}
