// Original procedural expanding wavefronts; no borrowed artwork.
// Caller supplies only exact current-frame native liquid pixels with roof proof.
fn rain_ring_normal(world: vec3<f32>, time: f32, amount: f32) -> vec3<f32> {
    let cell = floor(world.xz * 1.7);
    var slope = vec2<f32>(0.0);
    for (var z = -1; z <= 1; z++) {
        for (var x = -1; x <= 1; x++) {
            let site = cell + vec2<f32>(f32(x), f32(z));
            let seed = fract(sin(dot(site, vec2<f32>(127.1, 311.7))) * 43758.5453);
            let centre = (site + vec2<f32>(seed, fract(seed * 17.13))) / 1.7;
            let age = fract(time * 1.6 + seed);
            let delta = world.xz - centre;
            let radius = length(delta);
            let front = radius - age * 0.46;
            let envelope = exp(-front * front / 0.0007) * (1.0 - age);
            slope += delta / max(radius, 0.001) * sin(front * 150.0) * envelope;
        }
    }
    return vec3<f32>(slope.x, 0.0, slope.y) * (0.12 * amount);
}
