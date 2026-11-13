// 2D lights, added into one layer of the light buffer (see render2d.rs).

struct Camera {
    view_proj: mat4x4<f32>,
    screen: vec4<f32>,
};

@group(0) @binding(0) var<uniform> camera: Camera;

// A point light: a quad around it, lit by distance from the centre.

struct PointInstance {
    @location(0) position: vec2<f32>,
    @location(1) radii: vec2<f32>,      // inner, outer
    @location(2) color: vec4<f32>,      // rgb already times intensity
    @location(3) falloff: f32,
};

struct PointOut {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) offset: vec2<f32>,
    @location(1) @interpolate(flat) radii: vec2<f32>,
    @location(2) @interpolate(flat) color: vec4<f32>,
    @location(3) @interpolate(flat) falloff: f32,
};

@vertex
fn vs_point(@builtin(vertex_index) vertex_index: u32, light: PointInstance) -> PointOut {
    var corners = array<vec2<f32>, 6>(
        vec2<f32>(-1.0, -1.0),
        vec2<f32>(1.0, -1.0),
        vec2<f32>(1.0, 1.0),
        vec2<f32>(-1.0, -1.0),
        vec2<f32>(1.0, 1.0),
        vec2<f32>(-1.0, 1.0),
    );
    let offset = corners[vertex_index] * light.radii.y;
    var out: PointOut;
    out.clip_position = camera.view_proj * vec4<f32>(light.position + offset, 0.0, 1.0);
    out.offset = offset;
    out.radii = light.radii;
    out.color = light.color;
    out.falloff = light.falloff;
    return out;
}

@fragment
fn fs_point(in: PointOut) -> @location(0) vec4<f32> {
    return vec4<f32>(fs_point_color(in), 1.0);
}

fn fs_point_color(in: PointOut) -> vec3<f32> {
    let distance = length(in.offset);
    let span = max(in.radii.y - in.radii.x, 0.0001);
    let t = clamp((in.radii.y - distance) / span, 0.0, 1.0);
    // Unity's falloff strength: 0 lights the whole circle evenly up to its edge,
    // higher values fade it out sooner.
    let attenuation = pow(t, 1.0 + 3.0 * in.falloff);
    return in.color.rgb * attenuation;
}

// A polygon light: its triangles, one flat colour.

struct PolygonVertex {
    @location(0) position: vec2<f32>,
    @location(1) color: vec4<f32>,
};

struct PolygonOut {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) color: vec4<f32>,
};

@vertex
fn vs_polygon(vertex: PolygonVertex) -> PolygonOut {
    var out: PolygonOut;
    out.clip_position = camera.view_proj * vec4<f32>(vertex.position, 0.0, 1.0);
    out.color = vertex.color;
    return out;
}

@fragment
fn fs_polygon(in: PolygonOut) -> @location(0) vec4<f32> {
    return vec4<f32>(in.color.rgb, 1.0);
}

// Shadows. A light walls shadow is drawn after its shadow mask: 1 where the
// walls hide it. Its colour's alpha says how strongly they do.

@group(1) @binding(0) var shadow_mask: texture_2d<f32>;
@group(1) @binding(1) var shadow_sampler: sampler;

fn lit_fraction(position: vec4<f32>, strength: f32) -> f32 {
    let uv = position.xy / vec2<f32>(textureDimensions(shadow_mask));
    let hidden = textureSampleLevel(shadow_mask, shadow_sampler, uv, 0.0).r;
    return 1.0 - strength * hidden;
}

@fragment
fn fs_point_shadowed(in: PointOut) -> @location(0) vec4<f32> {
    let lit = fs_point_color(in) * lit_fraction(in.clip_position, in.color.a);
    return vec4<f32>(lit, 1.0);
}

@fragment
fn fs_polygon_shadowed(in: PolygonOut) -> @location(0) vec4<f32> {
    return vec4<f32>(in.color.rgb * lit_fraction(in.clip_position, in.color.a), 1.0);
}

// The mask: the shapes walls throw away from a light.
@fragment
fn fs_shadow_mask(in: PolygonOut) -> @location(0) vec4<f32> {
    return vec4<f32>(1.0, 1.0, 1.0, 1.0);
}
