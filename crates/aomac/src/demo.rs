//! Synthetic scene proving the renderer independent of the format slices.

use ao_scene::{Blend, Instance, Mesh, Scene, Submesh, Texture, TextureKey, Vertex};
use glam::{Mat4, Quat, Vec3};

fn key(id: u32) -> TextureKey {
    TextureKey { rdb_type: 0, id }
}

fn tex(n: u32, f: impl Fn(u32, u32) -> [u8; 4]) -> Texture {
    let rgba = (0..n * n).flat_map(|i| f(i % n, i / n)).collect();
    Texture { width: n, height: n, rgba }
}

fn quad(corners: [[f32; 3]; 4], normal: [f32; 3], uv_max: f32) -> Vec<Vertex> {
    let uv = [[0.0, uv_max], [uv_max, uv_max], [uv_max, 0.0], [0.0, 0.0]];
    (0..4).map(|i| Vertex { pos: corners[i], normal, uv: uv[i], ..Default::default() }).collect()
}

fn cube() -> Mesh {
    let mut vertices = vec![];
    let faces: [([f32; 3], [f32; 3], [f32; 3]); 6] = [
        ([1., 0., 0.], [0., 1., 0.], [0., 0., 1.]),
        ([-1., 0., 0.], [0., 1., 0.], [0., 0., -1.]),
        ([0., 1., 0.], [0., 0., 1.], [1., 0., 0.]),
        ([0., -1., 0.], [0., 0., -1.], [1., 0., 0.]),
        ([0., 0., 1.], [0., 1., 0.], [-1., 0., 0.]),
        ([0., 0., -1.], [0., 1., 0.], [1., 0., 0.]),
    ];
    for (n, u, v) in faces {
        let p = |a: f32, b: f32| -> [f32; 3] { std::array::from_fn(|i| 0.5 * (n[i] + a * u[i] + b * v[i])) };
        vertices.extend(quad([p(-1., -1.), p(-1., 1.), p(1., 1.), p(1., -1.)], n, 1.0));
    }
    let indices = (0..6u32).flat_map(|f| [0, 1, 2, 0, 2, 3].map(|i| f * 4 + i)).collect();
    Mesh { vertices, submeshes: vec![Submesh::new(indices, None)] }
}

pub fn scene(count: usize) -> Scene {
    let mut s = Scene::default();
    // Textures: 0 = ground checker, 1..=8 cube materials, 100 = alpha fence.
    s.textures.insert(key(0), tex(256, |x, y| if (x / 32 + y / 32) % 2 == 0 { [90, 130, 70, 255] } else { [70, 105, 55, 255] }));
    for m in 0..8u32 {
        let hue = |k: u32| ((m * 83 + k * 40) % 200 + 40) as u8;
        s.textures.insert(
            key(1 + m),
            tex(128, move |x, y| match m % 4 {
                0 => if y % 32 < 3 || (x + if (y / 32) % 2 == 0 { 0 } else { 32 }) % 64 < 3 { [200, 200, 190, 255] } else { [hue(0), hue(1) / 3, 40, 255] },
                1 => if (x + y) / 16 % 2 == 0 { [hue(0), hue(1), hue(2), 255] } else { [30, 30, 30, 255] },
                2 => { let h = (x.wrapping_mul(7919) ^ y.wrapping_mul(104729)) % 60; [hue(0).saturating_sub(h as u8), hue(1).saturating_sub(h as u8), hue(2), 255] }
                _ => [hue(0), (y * 2) as u8, hue(2), 255],
            }),
        );
    }
    s.textures.insert(
        key(100),
        tex(128, |x, y| {
            let bar = x % 32 < 8 || y % 64 < 6;
            if bar { [150, 100, 50, 255] } else { [0, 0, 0, 0] }
        }),
    );

    let ground = Mesh {
        vertices: quad([[-200., 0., 200.], [200., 0., 200.], [200., 0., -200.], [-200., 0., -200.]], [0., 1., 0.], 100.0),
        submeshes: vec![Submesh::new(vec![0, 1, 2, 0, 2, 3], Some(key(0)))],
    };
    let fence = Mesh {
        vertices: quad([[-2., 0., 0.], [2., 0., 0.], [2., 3., 0.], [-2., 3., 0.]], [0., 0., 1.], 1.0),
        submeshes: vec![Submesh { blend: Blend::AlphaTest, two_sided: true, ..Submesh::new(vec![0, 1, 2, 0, 2, 3], Some(key(100))) }],
    };
    s.meshes.push(ground);
    s.meshes.push(fence);
    for m in 0..8u32 {
        let mut c = cube();
        c.submeshes[0].texture = Some(key(1 + m));
        s.meshes.push(c);
    }
    // Blend test: translucent blue glass, additive glow with a vertex-colour fade, cubes behind/between.
    let mut glass = Mesh { vertices: quad([[-3., 0.5, 0.], [3., 0.5, 0.], [3., 4., 0.], [-3., 4., 0.]], [0., 0., 1.], 1.0), submeshes: vec![] };
    glass.submeshes.push(Submesh { blend: Blend::AlphaBlend, base_color: [0.2, 0.5, 1.0, 0.45], ..Submesh::new(vec![0, 1, 2, 0, 2, 3], None) });
    let mut glow = Mesh { vertices: quad([[-2., 0.5, 0.], [2., 0.5, 0.], [2., 3.5, 0.], [-2., 3.5, 0.]], [0., 0., 1.], 1.0), submeshes: vec![] };
    glow.vertices[2].color = [1.0, 1.0, 1.0, 0.0];
    glow.vertices[3].color = [1.0, 1.0, 1.0, 0.0];
    glow.submeshes.push(Submesh { blend: Blend::Additive, base_color: [1.0, 0.45, 0.1, 1.0], ..Submesh::new(vec![0, 1, 2, 0, 2, 3], None) });
    s.meshes.push(glass); // 10
    s.meshes.push(glow); // 11
    let at = |x: f32, y: f32, z: f32| Mat4::from_translation(Vec3::new(x, y, z)).to_cols_array_2d();
    s.instances.push(Instance { mesh: 0, transform: ao_scene::IDENTITY });
    s.instances.push(Instance { mesh: 2, transform: Mat4::from_scale_rotation_translation(Vec3::splat(2.0), Quat::from_rotation_y(0.5), Vec3::new(-1., 1., 8.)).to_cols_array_2d() });
    s.instances.push(Instance { mesh: 11, transform: at(1.0, 0., 10.5) }); // glow behind glass
    s.instances.push(Instance { mesh: 10, transform: at(0., 0., 12.) });
    s.instances.push(Instance { mesh: 11, transform: at(-1.5, 0., 13.5) }); // glow in front of glass
    s.instances.push(Instance { mesh: 10, transform: at(2., 0., 14.5) }); // second glass, nearer
    for i in 0..5 {
        let t = Mat4::from_rotation_translation(Quat::from_rotation_y(i as f32 * 0.4), Vec3::new(-8. + i as f32 * 4.5, 0., 6.));
        s.instances.push(Instance { mesh: 1, transform: t.to_cols_array_2d() });
    }
    let mut rng = 0x2545F491u32;
    let mut rnd = move || {
        rng = rng.wrapping_mul(1664525).wrapping_add(1013904223);
        (rng >> 8) as f32 / (1u32 << 24) as f32
    };
    // Spread grows with sqrt(count) so density stays roughly constant when scaled up.
    let half = 150.0 * (count as f32 / 600.0).sqrt().max(1.0);
    for _ in 0..count {
        let sc = 0.5 + rnd() * 2.5;
        let t = Mat4::from_scale_rotation_translation(
            Vec3::splat(sc),
            Quat::from_rotation_y(rnd() * 6.28),
            Vec3::new((rnd() - 0.5) * 2.0 * half, sc / 2.0, (rnd() - 0.5) * 2.0 * half),
        );
        s.instances.push(Instance { mesh: 2 + (rnd() * 8.0) as usize % 8, transform: t.to_cols_array_2d() });
    }
    // Self-illumination: a uniformly emissive cube and a glow-mask cube (window grid lit by alpha), plus a sky box.
    s.textures.insert(key(101), tex(128, |x, y| if x % 32 > 8 && y % 32 > 8 { [255, 230, 140, 255] } else { [60, 60, 70, 0] }));
    s.textures.insert(key(102), tex(64, |_, y| { let t = y as f32 / 63.0; [(20.0 + 60.0 * t) as u8, (25.0 + 70.0 * t) as u8, (70.0 + 90.0 * t) as u8, 255] }));
    let mut em = cube();
    em.submeshes[0].base_color = [0.1, 0.5, 0.2, 1.0];
    em.submeshes[0].emissive = [0.0, 1.0, 0.3];
    let mut gm = cube();
    gm.submeshes[0].texture = Some(key(101));
    gm.submeshes[0].glow_mask = true;
    let mut sky = cube();
    sky.submeshes[0].texture = Some(key(102));
    let (em_i, gm_i, sky_i) = (s.meshes.len(), s.meshes.len() + 1, s.meshes.len() + 2);
    s.meshes.extend([em, gm, sky]);
    let scaled = |sc: f32, p: Vec3| Mat4::from_scale_rotation_translation(Vec3::splat(sc), Quat::from_rotation_y(0.3), p).to_cols_array_2d();
    s.instances.push(Instance { mesh: em_i, transform: scaled(2.0, Vec3::new(-6., 1., 17.)) });
    s.instances.push(Instance { mesh: gm_i, transform: scaled(2.5, Vec3::new(6., 1.25, 17.)) });
    s.sky.push(Instance { mesh: sky_i, transform: scaled(400.0, Vec3::ZERO) });
    for (p, c) in [([-9., 1.5, 20.], [3.0, 0.4, 0.3]), ([-2., 1.5, 21.], [0.3, 1.2, 3.0]), ([3., 1.5, 20.], [2.5, 2.2, 0.4]), ([9., 1.5, 21.], [0.6, 3.0, 0.8])] {
        s.lights.push(ao_scene::Light { pos: p, color: c, range: 9.0 });
    }
    // Dusk, so the emissive/glow/lit surfaces and point lights stand out.
    s.environment = Some(ao_scene::Environment {
        sky_color: [0.02, 0.03, 0.08],
        fog_color: [0.03, 0.04, 0.09],
        fog_start: 150.0,
        fog_end: 600.0,
        ambient: [0.06, 0.07, 0.12],
        sun_color: [0.12, 0.1, 0.12],
        sun_dir: [0.4, 0.8, 0.3],
    });
    s.spawn = Some([0.0, 4.0, 22.0]);
    s.spawn_look_at = Some([0.0, 2.0, 10.0]);
    s
}
