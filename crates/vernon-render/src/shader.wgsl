// Vernon viewport shader (Apache-2.0).
//
// One 256-byte uniform block per draw item, bound with a dynamic offset.
// Colours arrive in linear RGB and are written gamma-encoded, because the
// offscreen target is Rgba8Unorm and egui samples user textures as sRGB-
// encoded data.

struct Uniforms {
    view_proj: mat4x4<f32>,
    model: mat4x4<f32>,
    // Inverse-transpose of `model` (upper 3x3 used).
    normal_mat: mat4x4<f32>,
    // Linear RGB wire colour; alpha unused (see viewport.w).
    wire_color: vec4<f32>,
    // Headlight: xyz = eye position (w = 1, perspective) or the direction
    // towards the eye (w = 0, orthographic).
    eye: vec4<f32>,
    // xy = target size in px, z = point size in px, w = edge alpha.
    viewport: vec4<f32>,
    // x = use vertex colour for lines/points (0/1), y = ambient term.
    params: vec4<f32>,
};

@group(0) @binding(0) var<uniform> u: Uniforms;

fn gamma_from_linear(c_in: vec3<f32>) -> vec3<f32> {
    let c = clamp(c_in, vec3<f32>(0.0), vec3<f32>(1.0));
    let lower = c * 12.92;
    let higher = 1.055 * pow(c, vec3<f32>(1.0 / 2.4)) - vec3<f32>(0.055);
    return select(higher, lower, c < vec3<f32>(0.0031308));
}

// ---- shaded triangles -------------------------------------------------

struct MeshIn {
    @location(0) pos: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) color: vec3<f32>,
};

struct MeshOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) world: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) color: vec3<f32>,
};

@vertex
fn vs_mesh(v: MeshIn) -> MeshOut {
    let world = u.model * vec4<f32>(v.pos, 1.0);
    var o: MeshOut;
    o.clip = u.view_proj * world;
    o.world = world.xyz;
    o.normal = (u.normal_mat * vec4<f32>(v.normal, 0.0)).xyz;
    o.color = v.color;
    return o;
}

@fragment
fn fs_mesh(f: MeshOut) -> @location(0) vec4<f32> {
    let to_eye = u.eye.xyz - f.world * u.eye.w;
    let l = to_eye / max(length(to_eye), 1e-20);
    var diffuse = 1.0;
    var spec = 0.0;
    let len = length(f.normal);
    if (len > 1e-12) {
        // Two-sided headlight.
        let ndl = abs(dot(f.normal / len, l));
        diffuse = ndl;
        spec = pow(ndl, 48.0) * 0.12;
    }
    let ambient = u.params.y;
    let c = f.color * (ambient + (1.0 - ambient) * diffuse) + vec3<f32>(spec);
    return vec4<f32>(gamma_from_linear(c), 1.0);
}

// ---- lines and points -------------------------------------------------

struct LineIn {
    @location(0) pos: vec3<f32>,
    @location(1) color: vec3<f32>,
};

struct FlatOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) color: vec4<f32>,
};

// Polygon edges: always the wire colour, alpha from viewport.w.
@vertex
fn vs_edge(v: LineIn) -> FlatOut {
    var o: FlatOut;
    o.clip = u.view_proj * (u.model * vec4<f32>(v.pos, 1.0));
    o.color = vec4<f32>(u.wire_color.rgb, u.viewport.w);
    return o;
}

// Polylines and grid: vertex colour when params.x = 1, else wire colour.
@vertex
fn vs_curve(v: LineIn) -> FlatOut {
    var o: FlatOut;
    o.clip = u.view_proj * (u.model * vec4<f32>(v.pos, 1.0));
    o.color = vec4<f32>(mix(u.wire_color.rgb, v.color, u.params.x), 1.0);
    return o;
}

// Points: one instance per point, expanded to a screen-aligned quad of
// viewport.z pixels.
@vertex
fn vs_point(@builtin(vertex_index) vi: u32, v: LineIn) -> FlatOut {
    var corners = array<vec2<f32>, 6>(
        vec2<f32>(-1.0, -1.0),
        vec2<f32>(1.0, -1.0),
        vec2<f32>(1.0, 1.0),
        vec2<f32>(-1.0, -1.0),
        vec2<f32>(1.0, 1.0),
        vec2<f32>(-1.0, 1.0),
    );
    var clip = u.view_proj * (u.model * vec4<f32>(v.pos, 1.0));
    let px = max(u.viewport.z, 1.0) / max(u.viewport.xy, vec2<f32>(1.0));
    clip = vec4<f32>(clip.xy + corners[vi % 6u] * px * clip.w, clip.zw);
    var o: FlatOut;
    o.clip = clip;
    o.color = vec4<f32>(mix(u.wire_color.rgb, v.color, u.params.x), 1.0);
    return o;
}

@fragment
fn fs_flat(f: FlatOut) -> @location(0) vec4<f32> {
    return vec4<f32>(gamma_from_linear(f.color.rgb), f.color.a);
}
