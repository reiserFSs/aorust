//! The procedural Shadowlands aurora (`e_GloomySky`): a feedback colour map that is drawn on a dome mesh as vertex colours.
//!
//! Port of `FUN_10047300` (DisplaySystem @0x10047300, the per-frame `RViewPort_t` draw of the `GloomySky` visual); the
//! point plot is `FUN_10045a24` (@0x10045a24). The mesh itself is built by `ao_formats::playfield::sky` from
//! `FUN_10046ccc` (@0x10046ccc). Every pass below is the decompiled loop, in the client's order:
//!
//! 1. `time += dt`; `ClearColorsEachFrame` zeroes the `ColorMapWidth x ColorMapHeight` map of ARGB `u32`;
//! 2. for each of `NumberOfCurves` curves, `x` runs `0..W` in `CurveDrawingStep` steps and plots one point at
//!    `y = H/2 + H/2 * sum_k cos(Repeats_k * (x/W * pi * 0.5) + AnimSpeed * time * Speed_k + Displacement_k) / Size_k`;
//!    the colour is `RandColor` with chance `ChanceToChangeColor / 1000` (one `rand()` per plot), else `MainColor`;
//! 3. `ColorSmoothStopFeedback` clears column 0; `Drag1..3`, `Blur1/2`, `Drag4..6` smooth in place, `Div2Fade1` halves;
//! 4. the map is copied to the vertex colours (`ClearUpper` zeroes two vertices of every row of the copy);
//! 5. `Div2Fade2` halves the map and `SubtractFade` takes that much off every alpha (pixel cleared below) for the next frame.
//!
//! The client runs this once per rendered frame, so the feedback depends on its frame rate, which is not stored anywhere:
//! [`STEP`] is an assumed 30 Hz **[UNRESOLVED]**.

/// Seconds per simulation frame (assumed client frame rate, 30 Hz) **[UNRESOLVED]**.
pub const STEP: f32 = 1.0 / 30.0;

/// One `CurveN*` set of the tweak object.
#[derive(Clone, Debug, PartialEq)]
pub struct Curve {
    /// `CurveNChanceToChangeColor`: per mille chance of `rand_color`.
    pub chance: i32,
    /// `CurveNMainColor` / `CurveNRandColor` (`0xRRGGBB`, alpha byte 0).
    pub main_color: u32,
    pub rand_color: u32,
    pub anim_speed: f32,
    /// `CurveNIntensity`: alpha of the plot is `intensity * 255 * weight`.
    pub intensity: f32,
    /// `CurveNUseAnimCurveM`, `CurveNAnimCurveM{Speed,Repeats,Size,Displacement}`.
    pub use_anim: [bool; 4],
    pub speed: [f32; 4],
    pub repeats: [f32; 4],
    pub size: [f32; 4],
    pub displacement: [f32; 4],
}

#[derive(Clone, Debug, PartialEq)]
pub struct GloomyParams {
    /// `ColorMapWidth` x `ColorMapHeight`.
    pub width: usize,
    pub height: usize,
    pub clear_each_frame: bool,
    pub curve_step: f32,
    pub stop_feedback: bool,
    /// `ColorSmoothDrag1..6` (1-3 before the blur, 4-6 after it).
    pub drag: [bool; 6],
    pub blur: [bool; 2],
    /// `ColorSmoothDiv2Fade1` (before the vertex copy), `Div2Fade2` (after it).
    pub div2_fade: [bool; 2],
    pub clear_upper: bool,
    pub subtract_fade: u32,
    /// The first `NumberOfCurves` (1..=9) curves.
    pub curves: Vec<Curve>,
}

/// MSVCRT `rand()` with the default seed 1 (`srand` is never called before the aurora starts [INFERENCE]).
#[derive(Clone, Debug, PartialEq)]
struct Rand(u32);

impl Rand {
    fn next(&mut self) -> i32 {
        self.0 = self.0.wrapping_mul(214_013).wrapping_add(2_531_011);
        ((self.0 >> 16) & 0x7fff) as i32
    }
}

const HALF: u32 = 0x7f7f_7f7f;

fn avg(a: u32, b: u32) -> u32 {
    (a >> 1 & HALF) + (b >> 1 & HALF)
}

/// Simulation state of one aurora object.
#[derive(Clone, Debug, PartialEq)]
pub struct GloomySky {
    pub params: GloomyParams,
    /// Feedback map, `map[y * width + x]`, ARGB.
    map: Vec<u32>,
    /// Vertex colours (ARGB) of the last frame, vertex `y * width + x` (`FUN_10046ccc` order).
    vertex: Vec<u32>,
    time: f32,
    frames: u64,
    rng: Rand,
}

impl GloomySky {
    pub fn new(params: GloomyParams) -> Self {
        let n = params.width * params.height;
        // the vertex buffer starts out yellow (0xff, 0xff, 0x00 bytes, `FUN_10046ccc`) until the first frame copies the map
        GloomySky { params, map: vec![0; n], vertex: vec![0xffff_ff00; n], time: 0.0, frames: 0, rng: Rand(1) }
    }

    /// ARGB vertex colours of the last simulated frame.
    pub fn vertex_colors(&self) -> &[u32] {
        &self.vertex
    }

    /// Runs frames of [`STEP`] until `t` seconds are simulated (at most 1200 frames per call).
    pub fn advance_to(&mut self, t: f32) {
        let target = (t.max(0.0) / STEP) as u64;
        let from = self.frames.max(target.saturating_sub(1200));
        self.frames = from;
        while self.frames < target {
            self.step(STEP);
            self.frames += 1;
        }
    }

    /// One frame (`FUN_10047300` after the state blob setup).
    pub fn step(&mut self, dt: f32) {
        let (w, h) = (self.params.width, self.params.height);
        let n = w * h;
        if n == 0 {
            return;
        }
        self.time += dt;
        if self.params.clear_each_frame {
            self.map.fill(0);
        }
        self.plot_curves();
        let p = &self.params;
        let m = &mut self.map;
        if p.stop_feedback {
            for r in 0..h {
                m[r * w] = 0;
            }
        }
        let drag = |m: &mut [u32]| {
            for i in 0..n.saturating_sub(1) {
                m[i] = avg(m[i], m[i + 1]);
            }
        };
        let blur = |m: &mut [u32]| {
            for i in 0..n.saturating_sub(1) {
                let v = avg(m[i], m[i + 1]);
                m[i] = v;
                m[i + 1] = v;
            }
            for i in 0..(h - 1) * w {
                let v = avg(m[i + w], m[i]);
                m[i] = v;
                m[i + w] = v;
            }
        };
        for d in &p.drag[..3] {
            if *d {
                drag(m);
            }
        }
        for b in &p.blur {
            if *b {
                blur(m);
            }
        }
        for d in &p.drag[3..] {
            if *d {
                drag(m);
            }
        }
        let halve = |m: &mut [u32]| m.iter_mut().for_each(|v| *v = *v >> 1 & HALF);
        if p.div2_fade[0] {
            halve(m);
        }
        self.vertex.clone_from(m);
        if p.clear_upper {
            // the client indexes the second vertex with the map *height* (`y * width + height - 1`), not the width
            for r in 0..h {
                self.vertex[r * w] = 0;
                if let Some(v) = (r * w + h).checked_sub(1).and_then(|i| self.vertex.get_mut(i)) {
                    *v = 0;
                }
            }
        }
        if p.div2_fade[1] {
            halve(m);
        }
        let fade = p.subtract_fade;
        if fade != 0 {
            for v in m.iter_mut() {
                let a = *v >> 24;
                *v = if fade < a { (*v & 0x00ff_ffff) | ((a - fade) << 24) } else { 0 };
            }
        }
    }

    fn plot_curves(&mut self) {
        let (w, h) = (self.params.width as f32, self.params.height as f32);
        let pi = std::f32::consts::PI;
        let step = self.params.curve_step;
        for ci in 0..self.params.curves.len() {
            let c = self.params.curves[ci].clone();
            let mut x = 0.0f32;
            // `step <= 0` would never leave the loop (the client's own tweak comment: "never bigger than 1.0")
            while w > 0.0 && step > 0.0 && x < w {
                let u = x / w * pi * 0.5;
                let mut v = 0.0f32;
                for k in 0..4 {
                    if c.use_anim[k] {
                        let arg = c.repeats[k] * u + c.anim_speed * self.time * c.speed[k] + c.displacement[k];
                        let val = (arg as f64).cos() as f32 / c.size[k];
                        v = if k == 0 { val } else { val + v };
                    }
                }
                let y = h * 0.5 * v;
                let color = if self.rng.next() % 1000 < c.chance { c.rand_color } else { c.main_color };
                self.plot(x - (self.params.width >> 1) as f32, y, c.intensity, color);
                x += step;
            }
        }
    }

    /// `FUN_10045a24`: splats four texels around `(x, y)` (relative to the map centre), alpha `intensity * 255 *
    /// max(weight_x, weight_y)`, OR-ed into the map.
    fn plot(&mut self, x: f32, y: f32, intensity: f32, color: u32) {
        let (w, h) = (self.params.width as u32, self.params.height as u32);
        let fy = (h >> 1) as f32 + y;
        let fx = (w >> 1) as f32 + x;
        let (iy, ix) = ((fy as i32 as u32) % h, (fx as i32 as u32) % w);
        let (iy1, ix1) = ((iy + 1) % h, (ix + 1) % w);
        let (fy_frac, fx_frac) = ((fy as f64 - (fy as f64).floor()) as f32, (fx as f64 - (fx as f64).floor()) as f32);
        let (ay0, ax0) = (1.0 - fy_frac, 1.0 - fx_frac);
        let scaled = intensity * 255.0;
        let at = |wgt: f32| ((scaled * wgt) as i32 as u32).wrapping_shl(24).wrapping_add(color);
        let m = &mut self.map;
        let wi = w as usize;
        m[iy as usize * wi + ix as usize] |= at(ax0.max(ay0));
        m[iy as usize * wi + ix1 as usize] |= at(fx_frac.max(ay0));
        m[iy1 as usize * wi + ix as usize] |= at(ax0.max(fy_frac));
        m[iy1 as usize * wi + ix1 as usize] |= at(fx_frac.max(fy_frac));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn curve(chance: i32) -> Curve {
        Curve { chance, main_color: 0x0000ff, rand_color: 0xffff00, anim_speed: 1.0, intensity: 0.5, use_anim: [true, false, false, false], speed: [0.0; 4], repeats: [0.0; 4], size: [1.0; 4], displacement: [0.0; 4] }
    }

    fn params() -> GloomyParams {
        GloomyParams {
            width: 8,
            height: 4,
            clear_each_frame: true,
            curve_step: 0.5,
            stop_feedback: false,
            drag: [false; 6],
            blur: [false; 2],
            div2_fade: [false; 2],
            clear_upper: false,
            subtract_fade: 0,
            curves: vec![curve(0)],
        }
    }

    #[test]
    fn msvcrt_rand_sequence() {
        let mut r = Rand(1);
        // the CRT's first outputs for the default seed
        assert_eq!([r.next(), r.next(), r.next()], [41, 18467, 6334]);
    }

    #[test]
    fn plot_splats_four_texels_with_max_weight() {
        let mut s = GloomySky::new(params());
        // on a texel corner (fx = fy = 0): weights max(1-fx, 1-fy) = 1 for (iy, ix), max(fx, 1-fy) = 1, max(1-fx, fy) = 1, max(fx, fy) = 0
        s.plot(0.0, 0.0, 0.5, 0x0000ff);
        let m = &s.map;
        assert_eq!(m[2 * 8 + 4], (127 << 24) | 0xff);
        assert_eq!(m[2 * 8 + 5], (127 << 24) | 0xff);
        assert_eq!(m[3 * 8 + 4], (127 << 24) | 0xff);
        assert_eq!(m[3 * 8 + 5], 0xff);
        // between texels both fractions are 0.5: every texel gets half the intensity
        let mut s = GloomySky::new(params());
        s.plot(0.5, 0.5, 1.0, 0);
        assert_eq!(s.map[2 * 8 + 4] >> 24, 127);
        // OR, not add
        s.plot(0.5, 0.5, 1.0, 0);
        assert_eq!(s.map[2 * 8 + 4] >> 24, 127);
    }

    #[test]
    fn frame_copies_the_map_before_the_fade_takes_it_away() {
        let mut p = params();
        p.subtract_fade = 200;
        p.curves[0].intensity = 1.0;
        let mut s = GloomySky::new(p);
        s.step(STEP);
        assert!(s.vertex_colors().iter().any(|&v| v >> 24 > 0), "plot visible in the vertex copy");
        // 255 * weight <= 255, fade 200: only alphas above 200 survive for the next frame, as alpha - 200
        assert!(s.map.iter().all(|&v| v == 0 || (v >> 24) <= 55));
    }

    #[test]
    fn smoothing_passes_halve_and_average() {
        assert_eq!(avg(0x8000_00fe, 0x0000_0002), 0x4000_0080);
        let mut p = params();
        p.clear_each_frame = false;
        p.div2_fade = [true, false];
        p.curves.clear();
        let mut s = GloomySky::new(p);
        s.map[5] = 0x8040_20fe;
        s.step(STEP);
        assert_eq!(s.vertex_colors()[5], 0x4020_107f);
    }

    #[test]
    fn clear_upper_uses_the_height_like_the_client() {
        let mut p = params();
        p.clear_each_frame = false;
        p.clear_upper = true;
        p.curves.clear();
        let mut s = GloomySky::new(p);
        s.map.fill(0xffff_ffff);
        s.step(STEP);
        let v = s.vertex_colors();
        // row r: vertex r*8 and r*8 + 4 - 1
        assert!(v[0] == 0 && v[3] == 0 && v[8] == 0 && v[11] == 0 && v[16] == 0 && v[24] == 0 && v[27] == 0);
        assert_eq!(v[1], 0xffff_ffff);
    }

    #[test]
    fn simulation_is_deterministic_and_advances_with_time() {
        let mut a = GloomySky::new(params());
        let mut b = a.clone();
        a.advance_to(1.0);
        b.advance_to(0.5);
        b.advance_to(1.0);
        assert_eq!(a, b);
        assert!(a.frames >= 29);
    }
}
