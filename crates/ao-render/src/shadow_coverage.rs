//! Light-volume admission is independent of camera visibility; only actors need pixel admission.
use glam::{Mat4, Vec3, Vec4};

pub(crate) fn admits_sphere(volumes: &[[Vec4; 6]], center: Vec3, radius: f32) -> bool {
    volumes.iter().any(|planes| planes.iter().all(|plane| plane.dot(center.extend(1.0)) >= -radius))
}

/// Expand depth only for loaded casters overlapping the cascade's XY footprint.
/// The snapped receiver texel grid, splits and horizon strength stay untouched.
pub(crate) fn include_caster_depth(matrix: Mat4, casters: impl Iterator<Item = (Vec3, f32)>) -> Mat4 {
    let rows = matrix.transpose();
    let mut low = 0.0f32;
    let mut high = 1.0f32;
    for (center, radius) in casters {
        if !center.is_finite() || !radius.is_finite() || radius < 0.0 { continue; }
        let clip = matrix * center.extend(1.0);
        let rx = rows.x_axis.truncate().length() * radius;
        let ry = rows.y_axis.truncate().length() * radius;
        if clip.x.abs() > clip.w + rx || clip.y.abs() > clip.w + ry { continue; }
        let rz = rows.z_axis.truncate().length() * radius;
        low = low.min(clip.z - rz);
        high = high.max(clip.z + rz);
    }
    let mut expanded = rows;
    expanded.z_axis = (rows.z_axis - low * rows.w_axis) / (high - low);
    expanded.transpose()
}

pub(crate) fn receiver_covers(uniform: &crate::sun_shadows::ShadowUniform, point: Vec3) -> bool {
    let depth = (point - Vec3::from_slice(&uniform.eye[..3])).dot(Vec3::from_slice(&uniform.forward[..3]));
    uniform.forward[3] > 0.0 && depth >= uniform.splits[3]
        && depth < uniform.splits[2].min(uniform.eye[3])
        && uniform.matrices.iter().any(|matrix| {
            let clip = Mat4::from_cols_array_2d(matrix) * point.extend(1.0);
            clip.x.abs() <= clip.w && clip.y.abs() <= clip.w && clip.z >= 0.0 && clip.z <= clip.w
        })
}

/// Native geometry is removed as one draw, so its complete receiver must fit.
/// A bounding sphere is deliberately conservative at cascade/fog boundaries.
pub(crate) fn receiver_covers_sphere(uniform: &crate::sun_shadows::ShadowUniform, center: Vec3, radius: f32) -> bool {
    if !center.is_finite() || !radius.is_finite() || radius < 0.0 || !receiver_covers(uniform, center) { return false; }
    let depth = (center - Vec3::from_slice(&uniform.eye[..3])).dot(Vec3::from_slice(&uniform.forward[..3]));
    depth - radius >= uniform.splits[3] && depth + radius < uniform.splits[2].min(uniform.eye[3])
        && uniform.matrices.iter().any(|matrix| {
            crate::frustum_planes(&Mat4::from_cols_array_2d(matrix)).iter()
                .all(|plane| plane.dot(center.extend(1.0)) >= radius)
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{frustum_culls_sphere, frustum_planes};

    #[test]
    fn off_view_static_and_terrain_casters_enter_light_volume() {
        let camera = Mat4::perspective_rh(0.8, 1.0, 0.1, 40.0);
        let light = Mat4::orthographic_rh(-20.0, 20.0, -20.0, 20.0, 0.1, 80.0);
        let caster = Vec3::new(10.0, 0.0, -5.0);
        assert!(frustum_culls_sphere(&frustum_planes(&camera), caster, 1.0));
        assert!(admits_sphere(&[frustum_planes(&light)], caster, 1.0));
        assert!(!admits_sphere(&[frustum_planes(&light)], Vec3::new(100.0, 0.0, -5.0), 1.0));
        // A loaded mountain upstream of the original depth range still occludes
        // receivers. It must not change the horizontal texel grid.
        let mountain = Vec3::new(0.0, 0.0, -300.0);
        assert!(!admits_sphere(&[frustum_planes(&light)], mountain, 10.0));
        let expanded = include_caster_depth(light, [(mountain, 10.0)].into_iter());
        assert!(admits_sphere(&[frustum_planes(&expanded)], mountain, 10.0));
        assert_eq!(light.transpose().x_axis, expanded.transpose().x_axis);
        assert_eq!(light.transpose().y_axis, expanded.transpose().y_axis);
    }

    #[test]
    fn replacement_requires_live_receiver_coverage() {
        let mut frame = crate::sun_shadows::CascadeFrame::new(Vec3::ZERO, -Vec3::Z, Vec3::Y,
            0.8, 1.0, 0.1, 180.0, Vec3::new(1.0, 1.0, 0.0), 1024).unwrap();
        assert!(receiver_covers(&frame.uniform, Vec3::new(0.0, 0.0, -10.0)));
        assert!(!receiver_covers(&frame.uniform, Vec3::new(0.0, 0.0, -181.0)));
        frame.uniform.forward[3] = 0.0;
        assert!(!receiver_covers(&frame.uniform, Vec3::new(0.0, 0.0, -10.0)));
    }

    #[test]
    fn covered_actor_does_not_replace_blob_outside_receiver_boundary() {
        let frame = crate::sun_shadows::CascadeFrame::new(Vec3::ZERO, -Vec3::Z, Vec3::Y,
            0.8, 1.0, 0.1, 180.0, Vec3::new(1.0, 1.0, 0.0), 1024).unwrap();
        let actor = Vec3::new(0.0, 0.0, -10.0);
        assert!(receiver_covers(&frame.uniform, actor));
        assert!(receiver_covers_sphere(&frame.uniform, actor, 0.5), "covered blob control");
        assert!(!receiver_covers_sphere(&frame.uniform, Vec3::new(0.0, 0.0, -181.0), 0.5),
            "actor coverage cannot replace a terrain receiver beyond the cascades");
        assert!(!receiver_covers_sphere(&frame.uniform, Vec3::new(0.0, 0.0, -179.5), 1.0),
            "partial blob coverage must keep the native draw");
        let mut fog_limited = frame.uniform;
        fog_limited.eye[3] = 20.0;
        assert!(receiver_covers(&fog_limited, actor));
        assert!(!receiver_covers_sphere(&fog_limited, Vec3::new(0.0, 0.0, -21.0), 0.5),
            "actor coverage cannot replace a terrain receiver beyond the fog receiver limit");
    }
}
