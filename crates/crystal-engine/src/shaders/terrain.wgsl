// Pixel-art splat terrain.
//
// Every decision is made once per *texture texel* (the world position snapped to
// `texels_per_metre`), and the output is one layer's texel colour, never a mix.
// That keeps borders crisp pixel steps and every pixel on the palette, while the
// splat map, the texels' own brightness ("height") and two noises decide where
// the steps go. `mock_transitions.py` in the art repo is the CPU reference.

struct Camera {
    view_proj: mat4x4<f32>,
};

struct TerrainParams {
    // min_x, min_z, size_x, size_z
    rect: vec4<f32>,
    // texels_per_metre, tile_metres, wobble, wobble_period
    blend: vec4<f32>,
    // grain, contact_shadow, layer_count, unused
    extra: vec4<f32>,
    // per layer, two vec4s each covering layers 0-3 and 4-7
    gains: array<vec4<f32>, 2>,
    slope_lo: array<vec4<f32>, 2>,
    // slope_hi <= slope_lo turns the slope term off for that layer
    slope_hi: array<vec4<f32>, 2>,
};

@group(0) @binding(0) var<uniform> camera: Camera;

@group(1) @binding(0) var<uniform> terrain: TerrainParams;
@group(1) @binding(1) var layer_textures: texture_2d_array<f32>;
@group(1) @binding(2) var layer_sampler: sampler;
@group(1) @binding(3) var splat_textures: texture_2d_array<f32>;
@group(1) @binding(4) var splat_sampler: sampler;

struct VertexInput {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
};

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) normal: vec3<f32>,
    @location(1) world_position: vec3<f32>,
};

@vertex
fn vs_main(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    out.clip_position = camera.view_proj * vec4<f32>(in.position, 1.0);
    out.normal = in.normal;
    out.world_position = in.position;
    return out;
}

fn hash(cell: vec2<i32>) -> f32 {
    var h: u32 = u32(cell.x) * 374761393u + u32(cell.y) * 668265263u;
    h = (h ^ (h >> 13u)) * 1274126177u;
    return f32(h & 0xFFFFu) / 65535.0;
}

fn value_noise(p: vec2<f32>) -> f32 {
    let i = floor(p);
    var f = p - i;
    f = f * f * (3.0 - 2.0 * f);
    let c = vec2<i32>(i);
    let a = hash(c);
    let b = hash(c + vec2<i32>(1, 0));
    let d = hash(c + vec2<i32>(0, 1));
    let e = hash(c + vec2<i32>(1, 1));
    return mix(mix(a, b, f.x), mix(d, e, f.x), f.y);
}

struct Pick {
    layer: i32,
    score: f32,
    color: vec3<f32>,
};

// The winning layer for one texel cell (integer texel coordinates in world XZ).
fn pick(cell: vec2<f32>, slope: f32) -> Pick {
    let texels_per_metre = terrain.blend.x;
    let tile_metres = terrain.blend.y;
    let wobble = terrain.blend.z;
    let period = terrain.blend.w;
    let grain = terrain.extra.x;
    let count = i32(terrain.extra.z);

    let t = (cell + vec2<f32>(0.5)) / texels_per_metre;
    let splat_uv = (t - terrain.rect.xy) / terrain.rect.zw;
    let w0 = textureSampleLevel(splat_textures, splat_sampler, splat_uv, 0, 0.0);
    let w1 = textureSampleLevel(splat_textures, splat_sampler, splat_uv, 1, 0.0);
    let layer_uv = t / tile_metres;
    let texel_noise = hash(vec2<i32>(cell)) - 0.5;

    var best = Pick(0, -1.0e9, vec3<f32>(0.0));
    for (var k = 0; k < count; k++) {
        let color = textureSampleLevel(layer_textures, layer_sampler, layer_uv, k, 0.0).rgb;
        // Brightness in sRGB terms, like an artist would judge it.
        let height = dot(pow(color, vec3<f32>(1.0 / 2.2)), vec3<f32>(0.3, 0.59, 0.11));

        var weight = select(w1[k % 4], w0[k % 4], k < 4);
        // Indexing the uniform directly: naga refuses runtime indices into array values.
        let lo = terrain.slope_lo[k / 4][k % 4];
        let hi = terrain.slope_hi[k / 4][k % 4];
        if (hi > lo) {
            weight += smoothstep(lo, hi, slope);
        }

        let offset = vec2<f32>(17.3, -9.1) * f32(k);
        let meander = value_noise((t + offset) / period) - 0.5;
        let score = weight
            + terrain.gains[k / 4][k % 4] * (height - 0.5)
            + wobble * meander
            + grain * texel_noise;
        if (score > best.score) {
            best = Pick(k, score, color);
        }
    }
    return best;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let normal = normalize(in.normal);
    let slope = 1.0 - normal.y;
    let cell = floor(in.world_position.xz * terrain.blend.x);

    let here = pick(cell, slope);

    // Contact shadow: a higher layer on the light's side (+X / +Z) shades this texel.
    var shade = 1.0;
    let toward_z = pick(cell + vec2<f32>(0.0, 1.0), slope);
    let toward_x = pick(cell + vec2<f32>(1.0, 0.0), slope);
    if ((toward_z.layer != here.layer && toward_z.score > here.score) ||
        (toward_x.layer != here.layer && toward_x.score > here.score)) {
        shade = terrain.extra.y;
    }

    // Same studio lighting as model.wgsl, so props and ground sit together.
    let key_direction = normalize(vec3<f32>(0.45, 0.80, 0.55));
    let fill_direction = normalize(vec3<f32>(-0.60, 0.25, -0.50));
    let key = max(dot(normal, key_direction), 0.0);
    let fill = max(dot(normal, fill_direction), 0.0) * 0.35;
    let ambient = 0.32 + 0.18 * (normal.y * 0.5 + 0.5);

    let color = here.color * shade * (ambient + key * 0.75 + fill);
    return vec4<f32>(color, 1.0);
}
