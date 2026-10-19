//! Lamp tests: CPU gating/revocation/cluster assignment (a Rust mirror of lamps_cluster.wgsl + lamps_build.wgsl), and one GPU
//! test that runs the real build shaders and compares them to that mirror.
use super::*;
use wgpu::util::DeviceExt;

fn packed(pos: [f32; 3], range: f32, linear: [f32; 3]) -> [[f32; 4]; 4] {
    let g = linear.map(|c| c.powf(1.0 / 2.2));
    [[pos[0], pos[1], pos[2], range], [g[0], g[1], g[2], 2.0], [-1.0, 0.0, 0.0, 0.0], [0.0; 4]]
}

fn view_proj(eye: Vec3) -> Mat4 {
    Mat4::perspective_rh(1.0, 1.6, 0.3, 200.0) * Mat4::look_to_rh(eye, -Vec3::Z, Vec3::Y)
}

fn run(lights: &[[[f32; 4]; 4]], zones: &[Option<u32>], owners: &[Option<u32>], zone_active: impl Fn(u32) -> bool, owner_slot: impl Fn(u32) -> Option<u32>) -> Vec<LampRec> {
    let flat: Vec<[f32; 4]> = lights.iter().flatten().copied().collect();
    let mut out = Vec::new();
    gather(&mut out, &Source { packed: &flat, zones, owners }, zone_active, owner_slot, view_proj(Vec3::ZERO), Vec3::ZERO);
    out
}

#[test]
fn gather_applies_zone_gate_range_validity_and_gain() {
    let on = packed([0.0, 1.0, -5.0], 10.0, [1.0, 0.5, 0.25]);
    let lights = [on, packed([1.0, 1.0, -5.0], 10.0, [1.0; 3]), packed([2.0, 1.0, -5.0], 0.0, [1.0; 3]),
        packed([f32::NAN, 1.0, -5.0], 10.0, [1.0; 3]), packed([3.0, 1.0, -5.0], 10.0, [0.0; 3])];
    let zones = [None, Some(7), None, None, None];
    let kept = run(&lights, &zones, &[], |zone| zone != 7, |_| None);
    assert_eq!(kept.len(), 1, "zone-gated, zero-range, NaN and black lights must be dropped");
    assert_eq!(kept[0].pos_range, on[0]);
    assert!((kept[0].color_cone[0] - LAMP_GAIN).abs() < 1e-5 && (kept[0].color_cone[1] - 0.5 * LAMP_GAIN).abs() < 1e-5, "{:?}", kept[0].color_cone);
    assert_eq!(kept[0].color_cone[3], 2.0, "no-cone marker survives");
    assert_eq!(run(&lights, &zones, &[], |_| true, |_| None).len(), 2, "an active zone keeps its lamp");
}

#[test]
fn gather_culls_by_view_frustum_and_reach() {
    let behind_far = packed([0.0, 0.0, 30.0], 5.0, [1.0; 3]);
    let behind_reaching = packed([0.0, 0.0, 4.0], 12.0, [1.0; 3]); // its sphere crosses the near plane into view
    let beyond_far_cap = packed([0.0, 0.0, -900.0], 20.0, [1.0; 3]);
    let kept = run(&[behind_far, behind_reaching, beyond_far_cap], &[], &[], |_| true, |_| None);
    assert_eq!(kept.iter().map(|l| l.pos_range[2]).collect::<Vec<_>>(), [4.0]);
}

#[test]
fn carried_light_of_a_hidden_actor_is_absent_exactly_like_the_actor() {
    let owned = packed([0.0, 1.0, -5.0], 10.0, [1.0, 0.0, 0.0]);
    let free = packed([1.0, 1.0, -5.0], 10.0, [0.0, 1.0, 0.0]);
    let admitted = |id: u32| (id == 7).then_some(3);
    let with_hidden = run(&[owned, free], &[], &[Some(9), None], |_| true, admitted);
    let without = run(&[free], &[], &[None], |_| true, admitted);
    assert_eq!(with_hidden, without, "an inadmissible owner must leave the list byte-identical to its absence");
    let shown = run(&[owned, free], &[], &[Some(7), None], |_| true, admitted);
    assert_eq!(shown.len(), 2);
    assert_eq!((shown[0].owner(), shown[1].owner()), (3, 0), "carried lights keep the owner's interaction slot, statics 0");
}

#[test]
fn keep_nearest_caps_at_max_lamps_and_drops_the_farthest() {
    let mut lamps: Vec<LampRec> = (0..MAX_LAMPS + 10).map(|i| LampRec { pos_range: [0.0, 0.0, -(i as f32) - 1.0, 5.0], ..bytemuck::Zeroable::zeroed() }).collect();
    keep_nearest(&mut lamps, Vec3::ZERO);
    assert_eq!(lamps.len(), MAX_LAMPS);
    assert!(lamps.iter().all(|l| -l.pos_range[2] <= MAX_LAMPS as f32), "a farther lamp displaced a nearer one");
}

// ---- Rust mirror of lamps_cluster.wgsl / lamps_build.wgsl ---------------------------------------------------------------

#[derive(Clone, Copy)]
struct Cam { near: f32, far: f32, tan: f32, aspect: f32, size: (f32, f32) }
const CAM: Cam = Cam { near: 0.3, far: 200.0, tan: 0.6, aspect: 1.6, size: (1920.0, 1200.0) };

fn near(c: Cam) -> f32 { c.near.max(0.05) }
fn far(c: Cam) -> f32 { c.far.min(FAR_CAP).max(near(c) * 2.0) }
fn boundary(c: Cam, k: u32) -> f32 { near(c) * (far(c) / near(c)).powf(k as f32 / 24.0) }
fn slice(c: Cam, depth: f32) -> u32 { ((depth.max(near(c)) / near(c)).ln() / (far(c) / near(c)).ln() * 24.0).max(0.0) as u32 }

fn cluster_box(c: Cam, idx: u32) -> (Vec3, Vec3) {
    let (tx, ty, tz) = (idx % 16, (idx / 16) % 9, idx / 144);
    let (px, py) = (2.0 / c.size.0, 2.0 / c.size.1);
    let x = [-1.0 + 2.0 * tx as f32 / 16.0 - px, -1.0 + 2.0 * (tx + 1) as f32 / 16.0 + px];
    let y = [1.0 - 2.0 * (ty + 1) as f32 / 9.0 - py, 1.0 - 2.0 * ty as f32 / 9.0 + py];
    let d = [boundary(c, tz) * 0.999, boundary(c, tz + 1) * 1.001];
    let (mut lo, mut hi) = (Vec3::splat(3.0e38), Vec3::splat(-3.0e38));
    for k in 0..8 {
        let depth = d[(k >> 2) & 1];
        let p = Vec3::new(x[k & 1] * c.tan * c.aspect * depth, y[(k >> 1) & 1] * c.tan * depth, -depth);
        lo = lo.min(p);
        hi = hi.max(p);
    }
    (lo, hi)
}

/// `lights`: (view-space centre, range, luminance); range <= 0 = revoked. Mirrors `build` for one cluster.
fn assign(c: Cam, lights: &[(Vec3, f32, f32)], idx: u32) -> Vec<usize> {
    let (lo, hi) = cluster_box(c, idx);
    let mut best: Vec<(f32, usize)> = Vec::new();
    for (i, &(p, range, lum)) in lights.iter().enumerate() {
        if range <= 0.0 { continue; }
        let d2 = p.clamp(lo, hi).distance_squared(p);
        if d2 > range * range { continue; }
        let s = lum * range / (d2 + 1.0);
        let at = best.iter().position(|&(other, _)| other < s).unwrap_or(best.len());
        if at >= 8 { continue; }
        best.insert(at, (s, i));
        best.truncate(8);
    }
    best.into_iter().map(|(_, i)| i).collect()
}

fn cluster_of(c: Cam, p: Vec3) -> Option<u32> {
    let depth = -p.z;
    if depth < near(c) || depth >= far(c) { return None; }
    let (nx, ny) = (p.x / (c.tan * c.aspect * depth), p.y / (c.tan * depth));
    if nx.abs() >= 1.0 || ny.abs() >= 1.0 { return None; }
    let tx = ((nx * 0.5 + 0.5) * 16.0) as u32;
    let ty = ((0.5 - ny * 0.5) * 9.0) as u32;
    Some((slice(c, depth) * 9 + ty) * 16 + tx)
}

struct Lcg(u32);
impl Lcg {
    fn next(&mut self) -> f32 { self.0 = self.0.wrapping_mul(1664525).wrapping_add(1013904223); (self.0 >> 8) as f32 / (1u32 << 24) as f32 }
}

#[test]
fn light_lands_in_every_cluster_its_sphere_reaches_and_in_none_beyond() {
    let mut rng = Lcg(7);
    for _ in 0..24 {
        let p = Vec3::new((rng.next() - 0.5) * 60.0, (rng.next() - 0.5) * 30.0, -(0.5 + rng.next() * 120.0));
        let range = 0.5 + rng.next() * 25.0;
        let lights = [(p, range, 1.0)];
        let listed: Vec<bool> = (0..CLUSTERS).map(|idx| assign(CAM, &lights, idx).contains(&0)).collect();
        // Completeness: any point of the sphere that is on screen is in a cluster that lists the light.
        for _ in 0..400 {
            let dir = Vec3::new(rng.next() - 0.5, rng.next() - 0.5, rng.next() - 0.5).normalize_or_zero();
            let q = p + dir * range * rng.next().cbrt();
            if let Some(idx) = cluster_of(CAM, q) { assert!(listed[idx as usize], "point {q:?} of the sphere ({p:?}, {range}) is in cluster {idx}, which misses the light"); }
        }
        // Tightness: a listed cluster's volume really is within reach (box distance <= range) and clusters far from the
        // sphere never list it.
        for idx in 0..CLUSTERS {
            let (lo, hi) = cluster_box(CAM, idx);
            let reach = p.clamp(lo, hi).distance(p);
            assert_eq!(listed[idx as usize], reach <= range, "cluster {idx} reach {reach} range {range}");
        }
    }
}

#[test]
fn a_tiny_light_lands_in_its_own_cluster_only_up_to_the_padding() {
    let p = Vec3::new(1.0, -0.5, -12.0);
    let lights = [(p, 0.01, 1.0)];
    let own = cluster_of(CAM, p).unwrap();
    let listed: Vec<u32> = (0..CLUSTERS).filter(|&idx| !assign(CAM, &lights, idx).is_empty()).collect();
    assert!(listed.contains(&own) && listed.len() <= 8, "{listed:?} vs {own}");
    let behind = [(Vec3::new(0.0, 0.0, 50.0), 20.0, 1.0)];
    assert!((0..CLUSTERS).all(|idx| assign(CAM, &behind, idx).is_empty()), "a light wholly behind the camera reaches no cluster");
}

#[test]
fn a_full_cluster_keeps_the_eight_most_influential_in_order_and_skips_revoked() {
    let mut lights: Vec<(Vec3, f32, f32)> = (0..12).map(|i| (Vec3::new(0.0, 0.0, -10.0), 8.0, 1.0 + i as f32)).collect();
    lights[11].1 = -1.0; // revoked brightest
    let idx = cluster_of(CAM, Vec3::new(0.0, 0.0, -10.0)).unwrap();
    assert_eq!(assign(CAM, &lights, idx), [10, 9, 8, 7, 6, 5, 4, 3]);
}

#[test]
fn wgsl_cluster_constants_match_the_rust_mirror() {
    let source = include_str!("lamps_cluster.wgsl");
    for line in ["const CLUSTER_X = 16u;", "const CLUSTER_Y = 9u;", "const CLUSTER_Z = 24u;", "const CLUSTER_COUNT = 3456u;",
        "const CLUSTER_MAX = 8u;", "const CLUSTER_STRIDE = 9u;", "const CLUSTER_FAR_CAP = 400.0;"] {
        assert!(source.contains(line), "{line}");
    }
    assert_eq!((CLUSTERS, CLUSTER_STRIDE, FAR_CAP), (3456, 9, 400.0));
}

// ---- GPU: the real shaders against the mirror ------------------------------------------------------------------------

fn read<T: bytemuck::Pod>(device: &wgpu::Device, queue: &wgpu::Queue, buffer: &wgpu::Buffer, len: usize) -> Vec<T> {
    let size = (len * std::mem::size_of::<T>()) as u64;
    let readback = device.create_buffer(&wgpu::BufferDescriptor { label: None, size, usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
    let mut enc = device.create_command_encoder(&Default::default());
    enc.copy_buffer_to_buffer(buffer, 0, &readback, 0, size);
    queue.submit([enc.finish()]);
    let (tx, rx) = std::sync::mpsc::channel();
    readback.slice(..).map_async(wgpu::MapMode::Read, move |r| tx.send(r).unwrap());
    device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
    rx.recv().unwrap().unwrap();
    let out = bytemuck::cast_slice(&readback.slice(..).get_mapped_range()).to_vec();
    out
}

#[test]
fn gpu_build_matches_the_mirror_revokes_hidden_owners_and_finds_cube_slots() {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let renderer = crate::Renderer::new(&instance, None).expect("lamp build test requires a GPU");
    let (device, queue) = (&renderer.device, &renderer.queue);
    let build = Build::new(device);
    // Identity view: view space == world space.
    let mut fc: crate::v2::consts::FrameConsts = bytemuck::Zeroable::zeroed();
    fc.view = Mat4::IDENTITY.to_cols_array_2d();
    fc.viewport = [CAM.size.0, CAM.size.1, 1.0 / CAM.size.0, 1.0 / CAM.size.1];
    fc.proj = [CAM.near, CAM.far, CAM.tan, CAM.aspect];
    let mut rng = Lcg(99);
    let mut recs: Vec<LampRec> = (0..40).map(|i| LampRec {
        pos_range: [(rng.next() - 0.5) * 40.0, (rng.next() - 0.5) * 20.0, -(1.0 + rng.next() * 60.0), 2.0 + rng.next() * 14.0],
        color_cone: [0.2 + rng.next(), 0.2 + rng.next(), 0.2 + rng.next(), 2.0],
        atten: [-1.0, 0.0, 0.0, 0.0],
        axis_owner: [0.0, 0.0, 0.0, f32::from_bits(if i == 5 { 2 } else if i == 6 { 3 } else { 0 })],
    }).collect();
    recs[7].pos_range[..3].copy_from_slice(&[4.0, 1.0, -9.0]);
    // Owner slot 2 admitted, slot 3 not; array shorter than slot 9000 to cover the bounds check.
    let visibility = [0u32, 0, 1, 0];
    let mut meta = vec![[0.0f32; 4]; 32 + 96 * 9];
    meta[1] = [1.0, 0.0, 0.0, 0.0]; // cube slot 1 live...
    meta[16 + 1] = recs[7].pos_range; // ...holding lamp 7
    meta[3] = [1.0, 0.0, 0.0, 0.0]; // slot 3 live but holding a light nobody has
    meta[16 + 3] = [9.0, 9.0, 9.0, 9.0];
    let init = |label, contents: &[u8], usage| device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: Some(label), contents, usage });
    let ssbo = wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC;
    let consts = init("fc", bytemuck::bytes_of(&fc), wgpu::BufferUsages::UNIFORM);
    let params = init("params", bytemuck::cast_slice(&[recs.len() as u32, 0, 0, 0]), wgpu::BufferUsages::UNIFORM);
    let lights = init("lights", bytemuck::cast_slice(&recs), ssbo);
    let view_lights = device.create_buffer(&wgpu::BufferDescriptor { label: None, size: recs.len() as u64 * 16, usage: ssbo, mapped_at_creation: false });
    let status = device.create_buffer(&wgpu::BufferDescriptor { label: None, size: recs.len() as u64 * 4, usage: ssbo, mapped_at_creation: false });
    let owner = init("owner", bytemuck::cast_slice(&visibility), wgpu::BufferUsages::STORAGE);
    let cube = init("cube", bytemuck::cast_slice(&meta), wgpu::BufferUsages::STORAGE);
    let clusters = device.create_buffer(&wgpu::BufferDescriptor { label: None, size: u64::from(CLUSTERS * CLUSTER_STRIDE) * 4, usage: ssbo, mapped_at_creation: false });
    let bind = device.create_bind_group(&wgpu::BindGroupDescriptor { label: None, layout: &build.layout, entries: &[
        wgpu::BindGroupEntry { binding: 0, resource: consts.as_entire_binding() }, wgpu::BindGroupEntry { binding: 1, resource: params.as_entire_binding() },
        wgpu::BindGroupEntry { binding: 2, resource: lights.as_entire_binding() }, wgpu::BindGroupEntry { binding: 3, resource: view_lights.as_entire_binding() },
        wgpu::BindGroupEntry { binding: 4, resource: status.as_entire_binding() }, wgpu::BindGroupEntry { binding: 5, resource: owner.as_entire_binding() },
        wgpu::BindGroupEntry { binding: 6, resource: cube.as_entire_binding() }, wgpu::BindGroupEntry { binding: 7, resource: clusters.as_entire_binding() },
    ] });
    let mut enc = device.create_command_encoder(&Default::default());
    build.encode(&mut enc, &bind, recs.len() as u32, None);
    queue.submit([enc.finish()]);

    let gpu: Vec<u32> = read(device, queue, &clusters, (CLUSTERS * CLUSTER_STRIDE) as usize);
    let status: Vec<u32> = read(device, queue, &status, recs.len());
    // Revoked: owner slot 3 is not admitted. Admitted owner (slot 2) and unowned lamps stay.
    let mirror: Vec<(Vec3, f32, f32)> = recs.iter().map(|r| {
        let revoked = r.owner() != 0 && visibility.get(r.owner() as usize).is_none_or(|&v| v == 0);
        (Vec3::from_slice(&r.pos_range[..3]), if revoked { -1.0 } else { r.pos_range[3] }, 0.2126 * r.color_cone[0] + 0.7152 * r.color_cone[1] + 0.0722 * r.color_cone[2])
    }).collect();
    let mut lit = 0;
    for idx in 0..CLUSTERS {
        let expected = assign(CAM, &mirror, idx);
        let at = (idx * CLUSTER_STRIDE) as usize;
        let got: Vec<usize> = gpu[at + 1..at + 1 + gpu[at] as usize].iter().map(|&i| i as usize).collect();
        assert_eq!(got, expected, "cluster {idx}");
        assert!(!got.contains(&6), "light 6 belongs to a hidden actor and must light nothing");
        lit += usize::from(got.contains(&5));
    }
    assert!(lit > 0, "light 5 (admitted owner) never reached any cluster; the revocation test would be vacuous");
    assert_eq!(status[7], 2, "lamp 7 sits in cube slot 1 (status = slot + 1)");
    assert_eq!(status[0], 0, "a lamp without a cube has no tap");
    assert_eq!(status[6], 0, "revoked lamps report nothing");
}
