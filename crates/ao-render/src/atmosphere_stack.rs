//! Enhanced atmosphere ordering; native world fog and primary admission have already run.
use crate::{EnhancedSettings, Renderer};
use glam::{Mat4, Vec3};

impl Renderer {
    pub(super) fn release_disabled_atmosphere(&mut self, settings: EnhancedSettings) {
        if !atmosphere_enabled(settings) { self.atmosphere = None; }
        if !settings.volumetric_clouds && !settings.cloud_shadows && !settings.cloud_gap_shafts { self.volumetric_clouds = None; }
        if !settings.sky_discs { self.sky_discs = None; self.sky_disc_transforms.clear(); }
    }

    pub(super) fn release_atmosphere_targets(&mut self) {
        if let Some(pass) = self.atmosphere.as_mut() { pass.release_targets(); }
        if let Some(pass) = self.volumetric_clouds.as_mut() { pass.release_targets(); }
        if let Some(pass) = self.sky_discs.as_mut() { pass.release_targets(); }
        self.atmosphere_layers.clear();
        self.atmosphere_native_layers.clear();
    }

    pub(super) fn atmosphere_target_bytes(&self) -> u64 {
        self.atmosphere.as_ref().map_or(0, crate::atmosphere::Atmosphere::target_bytes)
            + self.volumetric_clouds.as_ref().map_or(0, crate::volumetric_clouds::VolumetricClouds::target_bytes)
            + self.sky_discs.as_ref().map_or(0, crate::sky_discs::SkyDiscs::target_bytes)
    }

    pub(super) fn encode_atmosphere_stack(&mut self, encoder: &mut wgpu::CommandEncoder, vp: Mat4, eye: Vec3, far: f32, env: ao_scene::Environment) {
        let settings = self.enhanced_settings;
        if !atmosphere_enabled(settings) && !settings.volumetric_clouds && !settings.cloud_shadows && !settings.sky_discs { return; }
        let Some(frame) = self.enhanced_frame.as_ref() else { return; };
        self.atmosphere_native_layers.clear();
        self.atmosphere_native_layers.extend(self.sky.clouds.iter().map(|cloud| cloud.layer.clone()));
        self.atmosphere_layers.clear();
        self.atmosphere_layers.extend(self.atmosphere_native_layers.iter().map(|layer| crate::volumetric_clouds::density_layer(layer, eye, far, self.time)));
        if settings.cloud_gap_shafts {
            let clouds = self.volumetric_clouds.get_or_insert_with(|| crate::volumetric_clouds::VolumetricClouds::new(&self.device, self.format));
            clouds.prepare_cloud_field(&self.device, &self.queue, encoder, eye, &self.atmosphere_native_layers, self.time, far);
        }
        let cloud_field = self.volumetric_clouds.as_ref().and_then(|clouds| clouds.cloud_field());
        let directional_mask = frame.sun.as_ref().expect("atmosphere requests admitted-world coverage");
        if atmosphere_enabled(settings) {
            let pass = self.atmosphere.get_or_insert_with(|| crate::atmosphere::Atmosphere::new(&self.device, self.format));
            if !pass.is_ready() { pass.wait(); }
            pass.encode(&self.device, &self.queue, encoder, &frame.resolved_depth, directional_mask, &frame.colour,
                frame.size, vp, eye, far, env, self.fog.as_ref(), self.atmosphere_base_height,
                crate::atmosphere::AtmosphereSwitches {
                    height_haze: settings.height_haze, land_haze: settings.land_haze, valley_mist: settings.valley_mist,
                    volumetric_shafts: settings.volumetric_shafts, cloud_gap_shafts: settings.cloud_gap_shafts,
                }, self.sun_shadows.as_ref().map(|shadows| &shadows.receiver), &self.atmosphere_layers, env.sun_dir[1], cloud_field.as_ref());
        }
        if settings.sky_discs {
            let pass = self.sky_discs.get_or_insert_with(|| crate::sky_discs::SkyDiscs::new(&self.device, self.format));
            if !pass.is_ready() { pass.wait(); }
            pass.encode(&self.device, &self.queue, encoder, &frame.resolved_depth, directional_mask, &frame.colour, frame.size, vp, eye,
                &self.sky_disc_transforms, &self.sky_views, self.time, self.sky.pose.daylight,
                &self.atmosphere_layers, &self.sky_disc_sources);
        }
        if settings.volumetric_clouds || settings.cloud_shadows {
            let pass = self.volumetric_clouds.get_or_insert_with(|| crate::volumetric_clouds::VolumetricClouds::new(&self.device, self.format));
            if !pass.is_ready() { pass.wait(); }
            pass.encode(&self.device, &self.queue, encoder, &frame.resolved_depth, directional_mask, &frame.colour,
                frame.size, vp, eye, &self.atmosphere_native_layers, self.time, far, settings.volumetric_clouds, settings.cloud_shadows, env);
        }
    }
}

fn atmosphere_enabled(settings: EnhancedSettings) -> bool {
    settings.height_haze || settings.land_haze || settings.valley_mist || settings.volumetric_shafts || settings.cloud_gap_shafts
}
