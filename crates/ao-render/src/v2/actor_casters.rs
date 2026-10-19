//! Enhanced v2 actor shadow/admission draws. A child module of `actors` only because it reads the private actor item list;
//! all policy lives in `v2/admission.rs` and `v2/shadows.rs`.
use super::*;

impl Renderer {
    /// Per actor item (the instance index of the actor buffer): its actor's interaction slot, i.e. the 1-based index of the first
    /// item of that actor. Zero-alpha actors have no items, so they have no slot and nothing to admit.
    pub(crate) fn v2_item_slots(&self, out: &mut Vec<u32>) {
        crate::v2::item_slots(self.act.items.iter().map(|item| item.actor), out);
    }

    pub(crate) fn v2_has_actor_items(&self) -> bool { !self.act.items.is_empty() }

    /// Draws every actor part that can occlude (`ids == true`: also faded parts) or cast (`ids == false`) through `pipe_for`,
    /// whose argument is the native actor pipeline slot. `slots` is the per-item slot stream bound at vertex buffer 2.
    /// Native blob shadows, projection effects and blended priorities never take part (same rule as v1's primary pass).
    pub(crate) fn v2_draw_actor_parts<'a>(&self, pass: &mut wgpu::RenderPass<'_>, slots: &wgpu::Buffer, ids: bool,
        pipe_for: &dyn Fn(usize) -> Option<&'a wgpu::RenderPipeline>) {
        let Some(instances) = self.act.bufs.get(self.frame) else { return };
        pass.set_vertex_buffer(1, instances.slice(..));
        pass.set_vertex_buffer(2, slots.slice(..));
        for (index, item) in self.act.items.iter().enumerate() {
            if item.native_shadow_source.is_some() || (!ids && item.faded) || !matches!(item.rendering_effect, 0 | 2) { continue; }
            let model = &self.act.models[&item.model];
            let Some(mesh) = &model.meshes[item.mesh] else { continue };
            let own = (item.mesh == 0).then(|| self.act.actors.get(&item.actor)?.vb.as_ref()).flatten();
            pass.set_vertex_buffer(0, own.unwrap_or(&mesh.vb).slice(..));
            pass.set_index_buffer(mesh.ib.slice(..), wgpu::IndexFormat::Uint32);
            for draw in &mesh.draws {
                let priority = actor_priority(item.priority.or(visual_priority(draw, actor_visual_key(item, draw), &self.drawn_visuals)),
                    item.faded, item.rendering_effect, blended_pipe(draw.pipe));
                if !matches!(priority, 0..=7 | 10) { continue; }
                let slot = mesh_effect_pipe(if item.faded { faded_pipe(draw.pipe) } else { draw.pipe }, item.rendering_effect, draw.projection_pass);
                let Some(pipeline) = pipe_for(slot) else { continue };
                pass.set_pipeline(pipeline);
                pass.set_bind_group(1, &model.mats[draw.mat], &[]);
                pass.draw_indexed(draw.first_index..draw.first_index + draw.count, 0, index as u32..index as u32 + 1);
            }
        }
    }
}
