// Skinned, textured model shader.
//
// Skinning is evaluated on the CPU into joint matrices (see `scene.rs`); this
// stage only does the four-matrix weighted sum. The lighting is a deliberate
// two-light studio setup: a key plus a dim fill and a hemisphere ambient, which
// reads well on flat-shaded low-poly geometry and needs no light assets.

struct Camera {
    view_proj: mat4x4<f32>,
};

struct Object {
    model: mat4x4<f32>,
    base_color: vec4<f32>,
    // x: alpha mask enabled, yzw: reserved
    params: vec4<f32>,
    joints: array<mat4x4<f32>, 128>,
};

@group(0) @binding(0) var<uniform> camera: Camera;

@group(1) @binding(0) var<uniform> object: Object;
@group(1) @binding(1) var base_color_texture: texture_2d<f32>;
@group(1) @binding(2) var base_color_sampler: sampler;

struct VertexInput {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) joints: vec4<f32>,
    @location(4) weights: vec4<f32>,
};

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) normal: vec3<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) world_position: vec3<f32>,
};

@vertex
fn vs_main(in: VertexInput) -> VertexOutput {
    // Indices are clamped on the CPU to MAX_JOINTS, so the lookup is in range.
    let skin =
        object.joints[u32(in.joints.x)] * in.weights.x +
        object.joints[u32(in.joints.y)] * in.weights.y +
        object.joints[u32(in.joints.z)] * in.weights.z +
        object.joints[u32(in.joints.w)] * in.weights.w;

    let world = object.model * (skin * vec4<f32>(in.position, 1.0));
    let normal = (object.model * (skin * vec4<f32>(in.normal, 0.0))).xyz;

    var out: VertexOutput;
    out.clip_position = camera.view_proj * world;
    out.normal = normal;
    out.uv = in.uv;
    out.world_position = world.xyz;
    return out;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let sampled = textureSample(base_color_texture, base_color_sampler, in.uv);
    let base = sampled * object.base_color;

    // Cut-out foliage and the like: MASK materials discard sub-half alpha.
    if (object.params.x > 0.5 && base.a < 0.5) {
        discard;
    }

    let normal = normalize(in.normal);
    let key_direction = normalize(vec3<f32>(0.45, 0.80, 0.55));
    let fill_direction = normalize(vec3<f32>(-0.60, 0.25, -0.50));

    let key = max(dot(normal, key_direction), 0.0);
    let fill = max(dot(normal, fill_direction), 0.0) * 0.35;
    let ambient = 0.32 + 0.18 * (normal.y * 0.5 + 0.5);

    let color = base.rgb * (ambient + key * 0.75 + fill);
    return vec4<f32>(color, 1.0);
}
