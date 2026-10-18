//! Shadow draws reuse the admitted, current-frame actor parts and skinned buffers.
use super::*;
use crate::sun_shadows::SunShadows;

impl Renderer {
    /// Keep native blobs in the faithful path and whenever no usable sun replacement exists.
    pub(super) fn replaces_native_shadow(&self, source: u32, receiver: Option<(Vec3, f32)>) -> bool {
        if !self.enhanced_settings.sun_shadows { return false; }
        let Some(shadows) = self.sun_shadows.as_ref() else { return false; };
        // Off-view, disabled and fully fogged owners must not disclose themselves through a blob.
        let visible_source = self.act.items.iter().filter(|item| item.actor == source).any(|item| {
            let model = &self.act.models[&item.model];
            let Some(mesh) = model.meshes[item.mesh].as_ref() else { return false; };
            mesh.draws.iter().any(|draw| {
                let priority = actor_priority(item.priority.or(visual_priority(draw, actor_visual_key(item, draw), &self.drawn_visuals)),
                    item.faded, item.rendering_effect, blended_pipe(draw.pipe));
                let fog_exempt = matches!(item.rendering_effect, 2 | 4..=6)
                    || model.topology[item.mesh].1.iter().any(|part| !part.fog_enabled);
                matches!(priority, 0..=7 | 10) && (fog_exempt || item.fog_depth < shadows.native_fog_end)
            })
        });
        if !visible_source { return true; }
        if !shadows.sun_replacement_active { return false; }
        let Some(frame) = self.act.frames.iter().find(|frame| frame.id == source) else { return false; };
        let origin = Mat4::from_cols_array_2d(&frame.transform).w_axis.truncate();
        if !crate::shadow_coverage::receiver_covers(&shadows.coverage_uniform, origin) { return false; }
        let Some((center, radius)) = receiver else { return false; };
        if !crate::shadow_coverage::receiver_covers_sphere(&shadows.coverage_uniform, center, radius) { return false; }
        self.act.items.iter().filter(|item| item.actor == source && !item.faded
            && matches!(item.rendering_effect, 0 | 2)).any(|item| {
            let Some(mesh) = self.act.models[&item.model].meshes[item.mesh].as_ref() else { return false; };
            mesh.draws.iter().any(|draw| {
                let priority = actor_priority(item.priority.or(visual_priority(draw, actor_visual_key(item, draw), &self.drawn_visuals)),
                    item.faded, item.rendering_effect, blended_pipe(draw.pipe));
                let slot = mesh_effect_pipe(draw.pipe, item.rendering_effect, draw.projection_pass);
                matches!(priority, 0..=7 | 10)
                    && shadows.compiled.pipelines.get(&(true, slot, false)).is_some_and(Option::is_some)
            })
        })
    }

    pub(crate) fn prepare_actor_shadows(&self, shadows: &mut SunShadows) {
        shadows.prepare_actor_admission(&self.device, self.act.items.len());
        for (index, item) in self.act.items.iter().enumerate() {
            let group = *shadows.actor_groups.entry(item.actor).or_insert(index as u32 + 1);
            shadows.actor_ids.push(group);
        }
        if !shadows.actor_ids.is_empty() {
            self.queue.write_buffer(&shadows.actor_ids_buffer, 0, bytemuck::cast_slice(&shadows.actor_ids));
        }
    }

    /// Potential primary owner bounds only; admission always remains a current GPU decision.
    pub(crate) fn point_owner_capsules(&self, owners: &[u32], out: &mut Vec<[f32; 4]>) {
        out.clear();
        let mut frame_index = 0;
        for item in &self.act.items {
            if !owners.contains(&item.actor)
                || item.native_shadow_source.is_some() || !matches!(item.rendering_effect, 0 | 2) { continue; }
            while frame_index < self.act.frames.len() && self.act.frames[frame_index].id != item.actor {
                frame_index += 1;
            }
            let Some(frame) = self.act.frames.get(frame_index) else {
                out.push([0.0, 0.0, 0.0, f32::INFINITY]);
                break;
            };
            let model = &self.act.models[&item.model];
            let Some(mesh) = model.meshes[item.mesh].as_ref() else { continue; };
            if !mesh.draws.iter().any(|draw| {
                let priority = actor_priority(item.priority.or(visual_priority(draw, actor_visual_key(item, draw), &self.drawn_visuals)),
                    item.faded, item.rendering_effect, blended_pipe(draw.pipe));
                let slot = mesh_effect_pipe(if item.faded { faded_pipe(draw.pipe) } else { draw.pipe },
                    item.rendering_effect, draw.projection_pass);
                matches!(priority, 0..=7 | 10)
                    && self.shadow_pipelines.pipelines.get(&(true, slot, true)).is_some_and(Option::is_some)
            }) { continue; }
            let base = Mat4::from_cols_array_2d(&frame.transform);
            let transform = frame.parts.get(item.mesh).map_or(base, |part| base * Mat4::from_cols_array_2d(part));
            out.push(self.point_actor_bounds(item, mesh, transform));
        }
    }

    fn point_actor_bounds(&self, item: &Item, mesh: &ModelMesh, transform: Mat4) -> [f32; 4] {
        let actor = self.act.actors.get(&item.actor).filter(|_| item.mesh == 0);
        let skin_bounds = actor.filter(|a| a.vb.is_some()).and_then(|a| a.skin.as_ref().and(a.skin_bounds));
        let (center, radius) = if actor.is_some_and(|a| a.vb.is_some() && a.skin.is_none()) {
            // The retained native palette may extend beyond the authored bind bounds.
            let scale_bound = (transform.x_axis.truncate().length_squared()
                + transform.y_axis.truncate().length_squared() + transform.z_axis.truncate().length_squared()).sqrt();
            (transform.transform_point3(mesh.sphere.0), (mesh.sphere.1 + POSE_MARGIN) * scale_bound)
        } else {
            let (center, half) = skin_bounds.unwrap_or(mesh.bounds);
            let extent = transform.x_axis.truncate().abs() * half.x
                + transform.y_axis.truncate().abs() * half.y + transform.z_axis.truncate().abs() * half.z;
            (transform.transform_point3(center), extent.length())
        };
        if !center.is_finite() || !radius.is_finite() || radius < 0.0 {
            [0.0, 0.0, 0.0, f32::INFINITY]
        } else {
            [center.x, center.y, center.z, radius]
        }
    }

    /// Conservative CPU work signature, never a substitute for current GPU owner admission.
    pub(crate) fn point_owner_occluder_hash(&self, eye: Vec3, capsules: &[[f32; 4]]) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut hash = std::hash::DefaultHasher::new();
        let mut frame_index = 0;
        let mut item_count = 0u64;
        for item in &self.act.items {
            // Match the primary pass, including faded occluders that do not cast shadows.
            if item.native_shadow_source.is_some() || !matches!(item.rendering_effect, 0 | 2) { continue; }
            while frame_index < self.act.frames.len() && self.act.frames[frame_index].id != item.actor {
                frame_index += 1;
            }
            let Some(frame) = self.act.frames.get(frame_index) else { break; };
            let model = &self.act.models[&item.model];
            let Some(mesh) = model.meshes[item.mesh].as_ref() else { continue; };
            let base = Mat4::from_cols_array_2d(&frame.transform);
            let transform = frame.parts.get(item.mesh).map_or(base, |part| base * Mat4::from_cols_array_2d(part));
            if !crate::point_shadows::capsule_relevant(eye, self.point_actor_bounds(item, mesh, transform), capsules) { continue; }
            let mut draw_hash = std::hash::DefaultHasher::new();
            let mut draw_count = 0u64;
            let mut alpha_scroll = false;
            for draw in &mesh.draws {
                let priority = actor_priority(item.priority.or(visual_priority(draw, actor_visual_key(item, draw), &self.drawn_visuals)),
                    item.faded, item.rendering_effect, blended_pipe(draw.pipe));
                if !matches!(priority, 0..=7 | 10) { continue; }
                let slot = mesh_effect_pipe(if item.faded { faded_pipe(draw.pipe) } else { draw.pipe },
                    item.rendering_effect, draw.projection_pass);
                if !self.shadow_pipelines.pipelines.get(&(true, slot, true)).is_some_and(Option::is_some) { continue; }
                (draw.count, draw.first_index, draw.mat, slot, draw.node, priority).hash(&mut draw_hash);
                alpha_scroll |= mesh.submesh_mats.iter().zip(&model.topology[item.mesh].1).any(|((mat, _, _), part)| {
                    *mat == draw.mat && point_alpha_scroll(part)
                });
                draw_count += 1;
            }
            if draw_count == 0 { continue; }
            item_count += 1;
            (item.actor, item.model, item.mesh, model.point_cache_revision, draw_count, draw_hash.finish()).hash(&mut hash);
            hash.write(bytemuck::bytes_of(&frame.transform));
            frame.parts.get(item.mesh).is_some().hash(&mut hash);
            if let Some(part) = frame.parts.get(item.mesh) { hash.write(bytemuck::bytes_of(part)); }
            frame.alpha.to_bits().hash(&mut hash);
            frame.rendering_effect.hash(&mut hash);
            frame.part_uvs.get(item.mesh).map(|uv| uv.map(f32::to_bits)).hash(&mut hash);
            let material = frame.part_materials.get(item.mesh);
            material.and_then(|material| material.alpha).map(f32::to_bits).hash(&mut hash);
            // Use the actual retained GPU pose inputs, including when this frame submits None.
            // Neither pointer identities nor per-material repeated skin hashing are needed.
            if item.mesh == 0 {
                let actor = self.act.actors.get(&item.actor).filter(|actor| actor.vb.is_some());
                actor.is_some().hash(&mut hash);
                if let Some(actor) = actor {
                    actor.skin.is_some().hash(&mut hash);
                    if let Some(skin) = &actor.skin {
                        skin.len().hash(&mut hash);
                        hash.write(bytemuck::cast_slice(skin.as_slice()));
                    }
                    actor.pose.is_some().hash(&mut hash);
                    if let Some(native) = &actor.pose {
                        native.palette.len().hash(&mut hash);
                        hash.write(bytemuck::cast_slice(native.palette.as_slice()));
                    }
                }
            }
            if alpha_scroll {
                self.time.to_bits().hash(&mut hash);
            }
        }
        item_count.hash(&mut hash);
        hash.finish()
    }

    /// Append current actor draws; only the GPU's current owner proof admits them.
    pub(crate) fn point_actor_casters(&self, shadows: &SunShadows, out: &mut Vec<crate::point_shadows::CacheCaster>) {
        use std::hash::{Hash, Hasher};
        if self.act.bufs.get(self.frame).is_none() { return; }
        let mut frame_index = 0;
        for (index, item) in self.act.items.iter().enumerate() {
            if item.native_shadow_source.is_some() || item.faded || !matches!(item.rendering_effect, 0 | 2) { continue; }
            // prepare_actors emits meshes in submitted frame order, even when some are culled.
            while frame_index < self.act.frames.len() && self.act.frames[frame_index].id != item.actor {
                frame_index += 1;
            }
            let Some(frame) = self.act.frames.get(frame_index) else { break; };
            let model = &self.act.models[&item.model];
            let Some(mesh) = model.meshes[item.mesh].as_ref() else { continue; };
            let Some(&group) = shadows.actor_ids.get(index) else { continue; };
            let base = Mat4::from_cols_array_2d(&frame.transform);
            let transform = frame.parts.get(item.mesh).map_or(base, |part| base * Mat4::from_cols_array_2d(part));
            let actor = self.act.actors.get(&item.actor).filter(|_| item.mesh == 0);
            let [x, y, z, radius] = self.point_actor_bounds(item, mesh, transform);
            let center = Vec3::new(x, y, z);
            let material = frame.part_materials.get(item.mesh);
            let mut instance_transform = transform.to_cols_array_2d();
            instance_transform[0][3] = material.and_then(|m| m.alpha).unwrap_or(frame.alpha).min(1.0) - 1.0;
            let uv = frame.part_uvs.get(item.mesh).copied().unwrap_or([1.0, 1.0, 0.0, 0.0]);
            // Hash retained values, never Arc addresses, frame positions or ephemeral proof groups.
            // The potentially large CPU skin/native palette is hashed once per body, not per draw.
            let mut pose = std::hash::DefaultHasher::new();
            pose.write(bytemuck::bytes_of(&instance_transform));
            pose.write(bytemuck::bytes_of(&uv));
            model.point_cache_revision.hash(&mut pose);
            if let Some(actor) = actor.filter(|actor| actor.vb.is_some()) {
                actor.skin.is_some().hash(&mut pose);
                if let Some(skin) = &actor.skin {
                    skin.len().hash(&mut pose);
                    pose.write(bytemuck::cast_slice(skin.as_slice()));
                }
                actor.pose.is_some().hash(&mut pose);
                if let Some(native) = &actor.pose {
                    native.palette.len().hash(&mut pose);
                    pose.write(bytemuck::cast_slice(native.palette.as_slice()));
                }
            }
            for (draw_index, draw) in mesh.draws.iter().enumerate() {
                if !matches!(actor_priority(item.priority.or(visual_priority(draw, actor_visual_key(item, draw), &self.drawn_visuals)),
                    item.faded, item.rendering_effect, blended_pipe(draw.pipe)), 0..=7 | 10) { continue; }
                let slot = mesh_effect_pipe(draw.pipe, item.rendering_effect, draw.projection_pass);
                if !shadows.compiled.pipelines.get(&(true, slot, false)).is_some_and(Option::is_some) { continue; }
                let mut part = std::hash::DefaultHasher::new();
                (item.model, item.mesh, draw_index, draw.node, draw.mat, slot).hash(&mut part);
                let stable_part = part.finish() as u32;
                let mut draw_pose = pose.clone();
                (stable_part, draw.count, draw.first_index).hash(&mut draw_pose);
                if mesh.submesh_mats.iter().zip(&model.topology[item.mesh].1).any(|((mat, _, _), part)| {
                    *mat == draw.mat && point_alpha_scroll(part)
                }) {
                    self.time.to_bits().hash(&mut draw_pose);
                }
                out.push(crate::point_shadows::CacheCaster {
                    sphere: [center.x, center.y, center.z, radius],
                    draw: [draw.count, draw.first_index, index as u32, group],
                    key: [draw_pose.finish() as u32, item.actor, u32::from(frame.character), 1],
                    spare: [index as u32, draw_index as u32, 0, stable_part],
                });
            }
        }
    }

    /// Replay the descriptor order with the current native bindings and GPU-selected draws.
    pub(crate) fn draw_point_actors(&self, pass: &mut wgpu::RenderPass, shadows: &SunShadows,
        indirect: &wgpu::Buffer, base: u64, descriptors: &[crate::point_shadows::CacheCaster]) {
        let Some(instances) = self.act.bufs.get(self.frame) else { return; };
        let mut bound_item = None;
        for (global_index, descriptor) in descriptors.iter().enumerate() {
            if descriptor.key[3] != 1 { continue; }
            let item = &self.act.items[descriptor.spare[0] as usize];
            let model = &self.act.models[&item.model];
            let Some(mesh) = model.meshes[item.mesh].as_ref() else { continue; };
            let draw = &mesh.draws[descriptor.spare[1] as usize];
            let slot = mesh_effect_pipe(draw.pipe, item.rendering_effect, draw.projection_pass);
            let Some(Some(pipeline)) = shadows.compiled.pipelines.get(&(true, slot, false)) else { continue; };
            pass.set_pipeline(pipeline);
            pass.set_bind_group(1, &model.mats[draw.mat], &[]);
            if bound_item != Some(descriptor.spare[0]) {
                let own = (item.mesh == 0).then(|| self.act.actors.get(&item.actor)?.vb.as_ref()).flatten();
                pass.set_vertex_buffer(0, own.unwrap_or(&mesh.vb).slice(..));
                // Indirect first_instance stays zero on devices without INDIRECT_FIRST_INSTANCE.
                let first_instance = u64::from(descriptor.draw[2]);
                pass.set_vertex_buffer(1, instances.slice(first_instance * std::mem::size_of::<ActorInstance>() as u64..));
                pass.set_vertex_buffer(2, shadows.actor_ids_buffer.slice(first_instance * 4..));
                pass.set_index_buffer(mesh.ib.slice(..), wgpu::IndexFormat::Uint32);
                bound_item = Some(descriptor.spare[0]);
            }
            pass.draw_indexed_indirect(indirect, base + global_index as u64 * 20);
        }
    }

    pub(crate) fn draw_actor_shadows(&self, pass: &mut wgpu::RenderPass, shadows: &SunShadows, primary: bool) {
        let Some(instances) = self.act.bufs.get(self.frame) else { return; };
        pass.set_vertex_buffer(1, instances.slice(..));
        pass.set_vertex_buffer(2, shadows.actor_ids_buffer.slice(..));
        for (index, item) in self.act.items.iter().enumerate() {
            // Faded parts may occlude, but never emit an additional shadow.
            if item.native_shadow_source.is_some() { continue; }
            if (!primary && item.faded) || !matches!(item.rendering_effect, 0 | 2) { continue; }
            let model = &self.act.models[&item.model];
            let Some(mesh) = &model.meshes[item.mesh] else { continue; };
            let own = (item.mesh == 0).then(|| self.act.actors.get(&item.actor)?.vb.as_ref()).flatten();
            pass.set_vertex_buffer(0, own.unwrap_or(&mesh.vb).slice(..));
            pass.set_index_buffer(mesh.ib.slice(..), wgpu::IndexFormat::Uint32);
            for draw in &mesh.draws {
                if !matches!(actor_priority(item.priority.or(visual_priority(draw, actor_visual_key(item, draw), &self.drawn_visuals)), item.faded, item.rendering_effect, blended_pipe(draw.pipe)), 0..=7 | 10) { continue; }
                let slot = mesh_effect_pipe(if item.faded { faded_pipe(draw.pipe) } else { draw.pipe }, item.rendering_effect, draw.projection_pass);
                let Some(Some(pipeline)) = shadows.compiled.pipelines.get(&(true, slot, primary)) else { continue; };
                pass.set_pipeline(pipeline);
                pass.set_bind_group(1, &model.mats[draw.mat], &[]);
                pass.draw_indexed(draw.first_index..draw.first_index + draw.count, 0, index as u32..index as u32 + 1);
            }
        }
    }
}

/// Only sampled alpha can make primary/depth coverage change as the texture scrolls.
fn point_alpha_scroll(part: &ao_scene::Submesh) -> bool {
    (part.blend == Blend::AlphaTest || part.alpha_test.is_some() || part.sprite_alpha_test)
        && !(part.glow_mask && !part.prelit) && part.uv_scroll != [0.0; 2]
}
