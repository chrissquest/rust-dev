// Instanced sprite shader: one quad per instance, no vertex buffer for the corners.
//
// The quad is built from the vertex index. Each instance carries a full 3D
// position, so the same data works under a straight-down or a tilted camera.
// Texels with no alpha are discarded so they never write depth.

struct Camera {
    view_proj: mat4x4<f32>,
};

@group(0) @binding(0) var<uniform> camera: Camera;

@group(1) @binding(0) var sheet: texture_2d<f32>;
@group(1) @binding(1) var sheet_sampler: sampler;

struct Instance {
    @location(0) position: vec3<f32>,
    @location(1) rotation: f32,
    @location(2) size: vec2<f32>,
    @location(3) pivot: vec2<f32>,
    // u0, v0 (top-left), u1, v1 (bottom-right)
    @location(4) uv: vec4<f32>,
    @location(5) color: vec4<f32>,
};

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
    // The instance's UV rectangle as (min, max), for clamping in the fragment stage.
    @location(2) @interpolate(flat) uv_bounds: vec4<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vertex_index: u32, instance: Instance) -> VertexOutput {
    // Two counter-clockwise triangles covering the unit square, (0, 0) bottom-left.
    var corners = array<vec2<f32>, 6>(
        vec2<f32>(0.0, 0.0),
        vec2<f32>(1.0, 0.0),
        vec2<f32>(1.0, 1.0),
        vec2<f32>(0.0, 0.0),
        vec2<f32>(1.0, 1.0),
        vec2<f32>(0.0, 1.0),
    );
    let corner = corners[vertex_index];

    let local = (corner - instance.pivot) * instance.size;
    let s = sin(instance.rotation);
    let c = cos(instance.rotation);
    let rotated = vec2<f32>(local.x * c - local.y * s, local.x * s + local.y * c);

    var out: VertexOutput;
    out.clip_position = camera.view_proj
        * vec4<f32>(instance.position.xy + rotated, instance.position.z, 1.0);
    // The quad's bottom edge shows the rectangle's bottom edge (v1).
    out.uv = vec2<f32>(
        mix(instance.uv.x, instance.uv.z, corner.x),
        mix(instance.uv.w, instance.uv.y, corner.y),
    );
    out.color = instance.color;
    out.uv_bounds = vec4<f32>(
        min(instance.uv.xy, instance.uv.zw),
        max(instance.uv.xy, instance.uv.zw),
    );
    return out;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    // Keep the sample a hair inside the rectangle: at the exact edge, rounding can
    // pick the neighbouring cell of the sheet and draw a seam between tiles.
    let inset = 0.01 / vec2<f32>(textureDimensions(sheet));
    let uv = clamp(in.uv, in.uv_bounds.xy + inset, in.uv_bounds.zw - inset);

    let texel = textureSample(sheet, sheet_sampler, uv) * in.color;
    if (texel.a < 0.004) {
        discard;
    }
    return texel;
}
