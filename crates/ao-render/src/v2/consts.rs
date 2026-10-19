//! The per-frame constants every v2 pass reads (WGSL mirror: `struct FC` in `common.wgsl`; keep both in sync).
use glam::Mat4;

#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub(crate) struct FrameConsts {
    pub view: [[f32; 4]; 4],
    /// Jittered projection x view (what the G-buffer pass rasterised with).
    pub view_proj: [[f32; 4]; 4],
    /// Inverse of `view_proj`: reconstructs world positions from the (jittered) depth buffer.
    pub inv_view_proj: [[f32; 4]; 4],
    /// Unjittered current / previous projection x view, for temporal reprojection.
    pub view_proj_unjit: [[f32; 4]; 4],
    pub prev_view_proj_unjit: [[f32; 4]; 4],
    /// xyz = camera position, w = far plane.
    pub eye: [f32; 4],
    /// width, height, 1/width, 1/height of the full-resolution targets.
    pub viewport: [f32; 4],
    /// near, far, tan(fovy / 2), aspect.
    pub proj: [f32; 4],
    /// xy = this frame's projection jitter in pixels, z = frame counter, w = seconds.
    pub jitter: [f32; 4],
    /// Toward the sun; w = 1 when the sun lights (and shadows) the world.
    pub sun_dir: [f32; 4],
    /// Linear radiance.
    pub sun_color: [f32; 4],
    /// Moon A, the shadowed night key: toward the moon, w = 1 when active.
    pub moon_dir: [f32; 4],
    pub moon_color: [f32; 4],
    /// Linear hemisphere ambient: up-facing sky tint / down-facing ground bounce.
    pub sky_ambient: [f32; 4],
    pub ground_ambient: [f32; 4],
    /// Native fog colour (linear) and `start, end` of the native linear fog; zw unused.
    pub fog_color: [f32; 4],
    pub fog: [f32; 4],
    /// exposure, saturation, contrast, vignette.
    pub grade: [f32; 4],
    /// x = frame counter, y = bit0 AO texture valid, z/w reserved.
    pub flags: [u32; 4],
}

impl FrameConsts {
    /// Applies a sub-pixel jitter to `proj` (pixels, +x right, +y down on screen).
    pub(crate) fn jittered(proj: Mat4, jitter: [f32; 2], size: (u32, u32)) -> Mat4 {
        let mut m = proj;
        m.z_axis.x -= 2.0 * jitter[0] / size.0 as f32;
        m.z_axis.y += 2.0 * jitter[1] / size.1 as f32;
        m
    }
}

/// Halton(2, 3) sequence element `index` (1-based) centred on zero: the TAA jitter in pixels.
pub(crate) fn halton_jitter(index: u32) -> [f32; 2] {
    fn halton(mut i: u32, base: u32) -> f32 {
        let (mut f, mut r) = (1.0, 0.0);
        while i > 0 { f /= base as f32; r += f * (i % base) as f32; i /= base; }
        r
    }
    let i = index % 8 + 1;
    [halton(i, 2) - 0.5, halton(i, 3) - 0.5]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_is_16_byte_aligned_and_stable() {
        assert_eq!(std::mem::size_of::<FrameConsts>() % 16, 0);
        assert_eq!(std::mem::size_of::<FrameConsts>(), 5 * 64 + 13 * 16 + 16);
    }

    #[test]
    fn halton_stays_inside_the_pixel_and_repeats_every_eight_frames() {
        for i in 0..16 {
            let j = halton_jitter(i);
            assert!(j.iter().all(|c| c.abs() < 0.5), "{j:?}");
            assert_eq!(j, halton_jitter(i % 8));
        }
    }
}
