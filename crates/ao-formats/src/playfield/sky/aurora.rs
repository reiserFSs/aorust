//! `e_GloomySky` (the Shadowlands aurora, `Tweak_Shadowlands_Aurora_*.txt`): tweak variables -> [`GloomyParams`] (the
//! binding table of `FUN_1004412e`, DisplaySystem @0x1004412e) and the dome mesh of `FUN_10046ccc` (@0x10046ccc).
//! The colour map simulation is `ao_scene::aurora` (`FUN_10047300`).

use super::script::{self, Ctx, Obj};
use ao_scene::aurora::{Curve, GloomyParams};
use ao_scene::Vertex;

/// The client's literal `3.14` (double @0x1008c380) used for the azimuth of a vertex.
#[allow(clippy::approx_constant)]
const AZIMUTH_PI: f32 = 3.14;

/// Reads the tweak variables the visual binds (missing ones are 0 like unbound FXS variables; the three size/step floats
/// that are divisors default to 1 to keep the evaluation finite).
pub fn params(o: &Obj, ctx: &Ctx) -> Option<GloomyParams> {
    let f = |n: &str| script::float(o, ctx, n, 0.0);
    let flag = |n: &str| f(n) != 0.0;
    let (width, height) = (f("ColorMapWidth") as usize, f("ColorMapHeight") as usize);
    // the map is a vertex grid indexed with u16 in the client
    if width < 2 || height < 2 || width.checked_mul(height).is_none_or(|n| n > 65_535) {
        return None;
    }
    let n = (f("NumberOfCurves") as i32).clamp(1, 9) as usize;
    let curves = (1..=n)
        .map(|c| {
            let arr = |what: &str, default: f32| -> [f32; 4] { std::array::from_fn(|k| script::float(o, ctx, &format!("Curve{c}AnimCurve{}{what}", k + 1), default)) };
            Curve {
                chance: f(&format!("Curve{c}ChanceToChangeColor")) as i32,
                main_color: f(&format!("Curve{c}MainColor")) as u32,
                rand_color: f(&format!("Curve{c}RandColor")) as u32,
                anim_speed: f(&format!("Curve{c}AnimSpeed")),
                intensity: f(&format!("Curve{c}Intensity")),
                use_anim: std::array::from_fn(|k| flag(&format!("Curve{c}UseAnimCurve{}", k + 1))),
                speed: arr("Speed", 0.0),
                repeats: arr("Repeats", 0.0),
                size: arr("Size", 1.0),
                displacement: arr("Displacement", 0.0),
            }
        })
        .collect();
    Some(GloomyParams {
        width,
        height,
        clear_each_frame: flag("ClearColorsEachFrame"),
        curve_step: f("CurveDrawingStep"),
        stop_feedback: flag("ColorSmoothStopFeedback"),
        drag: std::array::from_fn(|k| flag(&format!("ColorSmoothDrag{}", k + 1))),
        blur: std::array::from_fn(|k| flag(&format!("ColorSmoothBlur{}", k + 1))),
        div2_fade: std::array::from_fn(|k| flag(&format!("ColorSmoothDiv2Fade{}", k + 1))),
        clear_upper: flag("ColorSmoothClearUpper"),
        subtract_fade: f("ColorSmoothSubtractFade") as u32,
        curves,
    })
}

/// Vertices (AO space, left handed, +Y up) and triangle indices of `FUN_10046ccc`. Vertex `b * W + a` (`a` along the
/// map width = down the dome from the vortex hole, `b` along the map height = azimuth); colours are filled in by the
/// simulation. The mesh is built from the object's own variables, then `Rotation` orients it.
pub fn mesh(o: &Obj, ctx: &Ctx, p: &GloomyParams) -> (Vec<Vertex>, Vec<u32>) {
    let f = |n: &str| script::float(o, ctx, n, 0.0);
    let (w, h) = (p.width, p.height);
    let (hole, height, below, rotation, radius) = (f("VortexHoleSize"), f("MeshHeight"), f("MeshBelowHorizon"), f("VortexRotation"), f("MeshXZRadius"));
    let curve_power = f("MeshBodyCurve") as u32;
    let mut vertices = vec![Vertex::default(); w * h];
    for a in 0..w {
        // t: 1 at the hole, 0 at the horizon, down to -MeshBelowHorizon at the last column
        let t = 1.0 - a as f32 * ((below + 1.0) / w as f32);
        let r = if t >= 0.0 {
            let mut t = t;
            for _ in 0..curve_power.min(64) {
                t *= t;
            }
            hole * t
        } else {
            -t / below
        };
        let s = 1.0 - r;
        let y = height * t;
        for b in 0..h {
            let angle = (b as f32 + b as f32) * AZIMUTH_PI / h as f32 + rotation * s;
            vertices[b * w + a] = Vertex {
                pos: [angle.sin() * s * radius, y * radius, angle.cos() * s * radius],
                uv: [a as f32 / w as f32 * 0.5, 0.5],
                ..Default::default()
            };
        }
    }
    // MeshTopToBottomFill closes the last column back onto the first (the vortex hole to the lowest point)
    let k = 1 - (f("MeshTopToBottomFill") as i32).clamp(0, 1) as usize;
    let mut indices = Vec::new();
    for a in 0..w - k {
        for b in 0..h {
            let v = |a: usize, b: usize| (b % h * w + a % w) as u32;
            let (v00, v01, v10, v11) = (v(a, b), v(a + 1, b), v(a, b + 1), v(a + 1, b + 1));
            if a & 1 == b & 1 {
                indices.extend([v00, v01, v10, v10, v01, v11]);
            } else {
                indices.extend([v00, v01, v11, v00, v11, v10]);
            }
        }
    }
    (vertices, indices)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::playfield::sky::script::parse_objects;

    fn aurora() -> Obj {
        let mut text = String::from("Object A\n{\n  Unsigned FXID: e_GloomySky\n  Unsigned ColorMapWidth 6u\n  Unsigned ColorMapHeight 4u\n  Float VortexHoleSize 0.5\n  Float MeshHeight 1.0\n  Unsigned MeshBodyCurve 1u\n  Float MeshBelowHorizon 0.5\n  Float VortexRotation 0.0\n  Float MeshXZRadius 10.0\n  Unsigned MeshTopToBottomFill e_No\n  Unsigned NumberOfCurves 2u\n  Unsigned ClearColorsEachFrame e_Yes\n  Float CurveDrawingStep 0.5\n");
        for c in 1..=2 {
            text += &format!("  Unsigned Curve{c}MainColor 0x0038d6\n  Float Curve{c}Intensity 0.5 * GAME.CurrentNightIntensity\n  Unsigned Curve{c}UseAnimCurve1 e_Yes\n  Float Curve{c}AnimCurve1Size 2.0\n");
        }
        text += "}\n";
        parse_objects(&text).remove(0)
    }

    #[test]
    fn overflowing_map_size_is_rejected() {
        let o = parse_objects("Object A\n{\n  Unsigned FXID: e_GloomySky\n  Unsigned ColorMapWidth 9223372036854775808u\n  Unsigned ColorMapHeight 2u\n}\n").remove(0);
        assert!(params(&o, &Ctx::at(0.0)).is_none());
    }

    #[test]
    fn params_follow_the_binding_table() {
        let ctx = Ctx { night: 0.5, ..Ctx::at(0.0) };
        let p = params(&aurora(), &ctx).unwrap();
        assert_eq!((p.width, p.height, p.curves.len()), (6, 4, 2));
        assert!(p.clear_each_frame && !p.clear_upper);
        assert_eq!(p.curves[0].main_color, 0x0038d6);
        assert_eq!(p.curves[1].intensity, 0.25, "night intensity scales the plot");
        assert_eq!((p.curves[0].use_anim, p.curves[0].size), ([true, false, false, false], [2.0, 1.0, 1.0, 1.0]));
    }

    #[test]
    fn mesh_is_a_vortex_dome_that_dips_below_the_horizon() {
        let (o, ctx) = (aurora(), Ctx::at(0.0));
        let p = params(&o, &ctx).unwrap();
        let (v, idx) = mesh(&o, &ctx, &p);
        assert_eq!(v.len(), 24);
        assert_eq!(idx.len(), (6 - 1) * 4 * 6);
        // column 0 is the top: hole radius 1 - 0.5 at height 1 * 10
        let top = v[0].pos;
        assert!((top[1] - 10.0).abs() < 1e-4 && (top[0].hypot(top[2]) - 5.0).abs() < 1e-4, "{top:?}");
        // the last column is below the horizon and smaller than the horizon ring
        let last = v[5].pos;
        assert!(last[1] < 0.0 && last[0].hypot(last[2]) < 10.0, "{last:?}");
        assert!(idx.iter().all(|&i| (i as usize) < v.len()));
    }
}
