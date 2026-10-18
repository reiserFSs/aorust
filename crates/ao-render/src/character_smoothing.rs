//! One opt-in subdivision level; native vertices and CPU selection geometry stay intact.
use ao_scene::{Mesh, Submesh, Vertex};
use glam::Vec3;
use std::collections::HashMap;

/// `samples[k]` generates `vertices[original_count + k]` from original endpoints.
/// The high bit of the second endpoint means linear (crease); mask it before indexing.
/// Skin these endpoints first, then interpolate: CAT's distinct influence-local positions
/// cannot be represented by averaging their two bone slots.
pub(super) struct Subdivision {
    pub vertices: Vec<Vertex>,
    pub submeshes: Vec<Submesh>,
    pub samples: Vec<[u32; 2]>,
}

const LINEAR: u32 = 1 << 31;
const CREASE_COS: f32 = 0.5; // Preserve authored normal or face turns of 60 degrees and above.

struct Edge {
    count: usize,
    submesh: usize,
    face: Vec3,
    smooth: bool,
    midpoint: Option<u32>,
}

fn key(a: u32, b: u32) -> (u32, u32) {
    if a < b { (a, b) } else { (b, a) }
}

fn finite_vertex(mut vertex: Vertex) -> Vertex {
    for value in vertex.pos.iter_mut().chain(vertex.normal.iter_mut())
        .chain(vertex.uv.iter_mut()).chain(vertex.color.iter_mut())
    {
        if !value.is_finite() { *value = 0.0; }
    }
    vertex
}

fn face(vertices: &[Vertex], triangle: &[u32]) -> Option<Vec3> {
    let [a, b, c] = [*vertices.get(triangle[0] as usize)?,
        *vertices.get(triangle[1] as usize)?, *vertices.get(triangle[2] as usize)?];
    if !a.pos.into_iter().chain(b.pos).chain(c.pos).all(f32::is_finite) {
        return None;
    }
    Some((Vec3::from_array(b.pos) - Vec3::from_array(a.pos))
        .cross(Vec3::from_array(c.pos) - Vec3::from_array(a.pos)).normalize_or_zero())
}

fn midpoint(a: Vertex, b: Vertex, smooth: bool) -> Vertex {
    let pa = Vec3::from_array(a.pos);
    let pb = Vec3::from_array(b.pos);
    let na = Vec3::from_array(a.normal).normalize_or_zero();
    let nb = Vec3::from_array(b.normal).normalize_or_zero();
    let linear = pa * 0.5 + pb * 0.5;
    let projected = linear - (na * (linear - pa).dot(na) + nb * (linear - pb).dot(nb)) * 0.5;
    let pos = if smooth && projected.is_finite() { projected } else { linear };
    let normal = (na + nb).normalize_or_zero();
    finite_vertex(Vertex {
        pos: pos.to_array(),
        normal: normal.to_array(),
        uv: std::array::from_fn(|i| a.uv[i] * 0.5 + b.uv[i] * 0.5),
        color: std::array::from_fn(|i| a.color[i] * 0.5 + b.color[i] * 0.5),
    })
}

pub(super) fn subdivide(mesh: &Mesh) -> Subdivision {
    let mut result = Subdivision {
        vertices: mesh.vertices.iter().copied().map(finite_vertex).collect(),
        submeshes: Vec::with_capacity(mesh.submeshes.len()),
        samples: Vec::new(),
    };
    // The endpoint flag reserves one index bit. Such meshes cannot be uploaded by
    // this path; retaining the native topology is preferable to aliasing indices.
    if mesh.vertices.len() >= LINEAR as usize {
        result.submeshes.clone_from(&mesh.submeshes);
        return result;
    }
    let mut edges = HashMap::<(u32, u32), Edge>::new();
    for (submesh, part) in mesh.submeshes.iter().enumerate() {
        for triangle in part.indices.as_chunks::<3>().0 {
            let Some(normal) = face(&mesh.vertices, triangle) else { continue };
            for (a, b) in [(triangle[0], triangle[1]), (triangle[1], triangle[2]), (triangle[2], triangle[0])] {
                let edge = edges.entry(key(a, b)).or_insert(Edge {
                    count: 0, submesh, face: normal, smooth: true, midpoint: None,
                });
                edge.count += 1;
                edge.smooth &= submesh == edge.submesh && normal.length_squared() > 0.0
                    && edge.face.dot(normal) > CREASE_COS;
            }
        }
    }
    for part in &mesh.submeshes {
        let mut output = part.clone();
        output.indices.clear();
        output.indices.reserve(part.indices.len().saturating_mul(4));
        for triangle in part.indices.as_chunks::<3>().0 {
            if face(&mesh.vertices, triangle).is_none() { continue; }
            let [a, b, c] = [triangle[0], triangle[1], triangle[2]];
            let mut midpoints = [0; 3];
            for (slot, (u, v)) in [(a, b), (b, c), (c, a)].into_iter().enumerate() {
                let endpoints = key(u, v);
                let edge = edges.get_mut(&endpoints).expect("validated triangle edge");
                midpoints[slot] = if let Some(index) = edge.midpoint { index } else {
                    let va = result.vertices[endpoints.0 as usize];
                    let vb = result.vertices[endpoints.1 as usize];
                    let na = Vec3::from_array(va.normal).normalize_or_zero();
                    let nb = Vec3::from_array(vb.normal).normalize_or_zero();
                    // Index identity, not spatial welding, preserves duplicated normal/UV seams.
                    // Boundary, material, non-manifold and sharp edges remain exactly linear.
                    let smooth = edge.count == 2 && edge.smooth && na.dot(nb) > CREASE_COS;
                    let index = result.vertices.len() as u32;
                    result.vertices.push(midpoint(va, vb, smooth));
                    result.samples.push([endpoints.0, endpoints.1 | if smooth { 0 } else { LINEAR }]);
                    edge.midpoint = Some(index);
                    index
                };
            }
            let [ab, bc, ca] = midpoints;
            output.indices.extend_from_slice(&[a, ab, ca, ab, b, bc, ca, bc, c, ab, bc, ca]);
        }
        result.submeshes.push(output);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn patch() -> Mesh {
        let mut vertices: Vec<_> = [[-1.0, 0.0, 0.0], [1.0, 0.0, 0.0],
            [0.0, 1.0, 0.0], [0.0, -1.0, 0.0]].into_iter()
            .map(|pos| Vertex { pos, normal: [0.0, 0.0, 1.0], ..Vertex::default() }).collect();
        vertices[0].normal = [-0.3, 0.0, 1.0];
        vertices[1].normal = [0.3, 0.0, 1.0];
        vertices[0].uv = [0.0, 0.2];
        vertices[1].uv = [1.0, 0.8];
        vertices[0].color = [0.0; 4];
        Mesh { vertices, submeshes: vec![Submesh::new(vec![0, 1, 2, 1, 0, 3], None)] }
    }

    fn sample(result: &Subdivision, a: u32, b: u32) -> (Vertex, bool) {
        let index = result.samples.iter().position(|s| s[0] == a && s[1] & !LINEAR == b).unwrap();
        (result.vertices[result.vertices.len() - result.samples.len() + index], result.samples[index][1] & LINEAR != 0)
    }

    #[test]
    fn one_level_preserves_originals_materials_winding_and_attributes() {
        let mesh = patch();
        let result = subdivide(&mesh);
        assert_eq!(&result.vertices[..mesh.vertices.len()], mesh.vertices.as_slice());
        assert_eq!(result.vertices.len(), 9);
        assert_eq!(result.samples.len(), 5);
        assert_eq!(result.submeshes[0].indices.len(), 24);
        let mut original_material = mesh.submeshes[0].clone();
        original_material.indices = result.submeshes[0].indices.clone();
        assert_eq!(result.submeshes[0], original_material);
        let (middle, linear) = sample(&result, 0, 1);
        assert!(!linear);
        assert!(middle.pos[2] > 0.0);
        assert_eq!(middle.uv, [0.5, 0.5]);
        assert_eq!(middle.color, [0.5; 4]);
        assert!((Vec3::from_array(middle.normal).length() - 1.0).abs() < 1e-6);
        for triangle in result.submeshes[0].indices.as_chunks::<3>().0 {
            assert!(face(&result.vertices, triangle).unwrap().z > 0.0);
        }
        let (boundary, linear) = sample(&result, 0, 2);
        assert!(linear);
        assert_eq!(boundary.pos, [-0.5, 0.5, 0.0]);
    }

    #[test]
    fn material_normal_face_and_nonmanifold_creases_stay_linear() {
        let mut materials = patch();
        materials.submeshes[0].indices.truncate(3);
        materials.submeshes.push(Submesh::new(vec![1, 0, 3], None));
        assert!(sample(&subdivide(&materials), 0, 1).1);
        let mut normals = patch();
        normals.vertices[1].normal = [1.0, 0.0, 0.0];
        assert!(sample(&subdivide(&normals), 0, 1).1);
        let mut faces = patch();
        faces.vertices[3].pos = [0.0, 0.0, 1.0];
        assert!(sample(&subdivide(&faces), 0, 1).1);
        let mut nonmanifold = patch();
        nonmanifold.submeshes[0].indices.extend_from_slice(&[1, 0, 3]);
        assert!(sample(&subdivide(&nonmanifold), 0, 1).1);
    }

    #[test]
    fn duplicated_positions_never_weld_across_authored_normals() {
        let mut mesh = patch();
        for source in [1, 0] {
            let mut vertex = mesh.vertices[source];
            vertex.normal = [0.0, 1.0, 0.0];
            mesh.vertices.push(vertex);
        }
        mesh.submeshes[0].indices = vec![0, 1, 2, 4, 5, 3];
        let result = subdivide(&mesh);
        assert!(sample(&result, 0, 1).1);
        assert!(sample(&result, 4, 5).1);
        assert_eq!(sample(&result, 0, 1).0.normal, [0.0, 0.0, 1.0]);
        assert_eq!(sample(&result, 4, 5).0.normal, [0.0, 1.0, 0.0]);
    }

    #[test]
    fn malformed_and_degenerate_geometry_is_finite_and_nonpanicking() {
        let mut mesh = patch();
        mesh.submeshes[0].indices.extend_from_slice(&[0, 1, u32::MAX, 0, 0, 0, 1]);
        let result = subdivide(&mesh);
        assert_eq!(result.submeshes[0].indices.len(), 36);
        mesh.vertices[0].pos[0] = f32::NAN;
        mesh.vertices[1].normal = [f32::INFINITY; 3];
        mesh.vertices[2].uv[0] = f32::NEG_INFINITY;
        mesh.vertices[3].color[0] = f32::NAN;
        let result = subdivide(&mesh);
        for vertex in &result.vertices {
            assert!(vertex.pos.into_iter().chain(vertex.normal).chain(vertex.uv).chain(vertex.color).all(f32::is_finite));
        }
        assert!(result.submeshes[0].indices.iter().all(|&i| (i as usize) < result.vertices.len()));
        assert!(subdivide(&Mesh::default()).samples.is_empty());
    }
}
