//! Shadow draws reuse the admitted, current-frame actor parts and skinned buffers.
use super::*;
use crate::sun_shadows::SunShadows;

impl Renderer {
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

    pub(crate) fn draw_actor_shadows(&self, pass: &mut wgpu::RenderPass, shadows: &SunShadows, primary: bool) {
        let Some(instances) = self.act.bufs.get(self.frame) else { return; };
        pass.set_vertex_buffer(1, instances.slice(..));
        pass.set_vertex_buffer(2, shadows.actor_ids_buffer.slice(..));
        for (index, item) in self.act.items.iter().enumerate() {
            // Faded parts may occlude, but never emit an additional shadow.
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
