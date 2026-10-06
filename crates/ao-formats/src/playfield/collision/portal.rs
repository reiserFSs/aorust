//! Teleportal polygons (`PortalArea_t`, `n3SurfaceResource_t +0x34`): the area of a zone surface the client's own character
//! walks into to start a zone change (`n3Zone_t::IsPosInTeleportal` N3 @0x1001a86a, docs/zone/world.md §10.2).
//!
//! The polygon is stored with every zone surface record it overlaps (rdb 1000013, see `examples/portal_survey`), and the test
//! only asks the zone that holds the position (`n3Dynel_t::GetZone`).

use std::collections::HashMap;

/// `FUN_1001b21a` rejects edges whose z extent is below this (f32 @N3 0x1003da14 = 1e-6).
const MIN_EDGE_DZ: f32 = 1e-6;

/// Polygons of the zones of one playfield.
#[derive(Default)]
pub(super) struct Portals {
    /// Zone index -> polygon (AO world coordinates, `y` up).
    pub zones: HashMap<u32, Vec<[f32; 3]>>,
}

impl Portals {
    /// `n3Zone_t::IsPosInTeleportal` for the zone `zone` and the AO world position `p` (`GetTeleportalArea` then `FUN_1001b21a`).
    /// A zone without a portal answers false.
    pub fn contains(&self, zone: u32, p: [f32; 3]) -> bool {
        self.zones
            .get(&zone)
            .is_some_and(|poly| polygon_contains(poly, p))
    }
}

/// `FUN_1001b21a` [N3 0x1001b21a]: crossing parity of the +x ray from `(p.x, p.z)` against the polygon edges in the x/z plane (`y`
/// is ignored). Edge `i -> i+1` (wrapping) counts when its endpoints lie on strictly opposite sides of `z = p.z`
/// (`(z0 - z) * (z1 - z) < 0`, f32 @0x1003cb08 = 0), its z extent is at least 1e-6, and the edge crosses `z` at an x greater than
/// `p.x`. The original then maps the point onto the polygon's `Path_t` for the distance along it, which the caller never reads.
pub(super) fn polygon_contains(poly: &[[f32; 3]], p: [f32; 3]) -> bool {
    let mut crossings = 0u32;
    for (i, a) in poly.iter().enumerate() {
        let b = &poly[(i + 1) % poly.len()];
        let (da, db) = (a[2] - p[2], b[2] - p[2]);
        let span = db - da;
        if db * da < 0.0 && span.abs() >= MIN_EDGE_DZ && p[0] < (a[0] * db - b[0] * da) / span {
            crossings += 1;
        }
    }
    crossings & 1 != 0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quad() -> Vec<[f32; 3]> {
        vec![
            [0.0, 5.0, 0.0],
            [10.0, 5.0, 0.0],
            [10.0, 5.0, 10.0],
            [0.0, 5.0, 10.0],
        ]
    }

    #[test]
    fn parity_in_the_xz_plane_ignores_height() {
        let q = quad();
        assert!(polygon_contains(&q, [5.0, -100.0, 5.0]));
        assert!(polygon_contains(&q, [0.5, 0.0, 9.5]));
        assert!(
            !polygon_contains(&q, [11.0, 5.0, 5.0]),
            "right of the polygon: no crossing at greater x"
        );
        assert!(!polygon_contains(&q, [5.0, 5.0, 10.5]));
        assert!(!polygon_contains(&q, [5.0, 5.0, -0.5]));
        assert!(!polygon_contains(&[], [0.0, 0.0, 0.0]));
    }

    #[test]
    fn concave_polygon_counts_both_arms() {
        // U shape opening towards +z: the notch (x 4..6, z 4..10) is outside
        let u = vec![
            [0.0, 0.0, 0.0],
            [10.0, 0.0, 0.0],
            [10.0, 0.0, 10.0],
            [6.0, 0.0, 10.0],
            [6.0, 0.0, 4.0],
            [4.0, 0.0, 4.0],
            [4.0, 0.0, 10.0],
            [0.0, 0.0, 10.0],
        ];
        assert!(polygon_contains(&u, [2.0, 0.0, 8.0]));
        assert!(polygon_contains(&u, [8.0, 0.0, 8.0]));
        assert!(!polygon_contains(&u, [5.0, 0.0, 8.0]));
        assert!(polygon_contains(&u, [5.0, 0.0, 2.0]));
    }

    #[test]
    fn only_the_asked_zone_answers() {
        let mut p = Portals::default();
        p.zones.insert(7, quad());
        assert!(p.contains(7, [5.0, 5.0, 5.0]));
        assert!(!p.contains(8, [5.0, 5.0, 5.0]));
    }
}
