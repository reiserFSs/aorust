//! Native-weather sky preparation and post-world atmosphere; all light energy is shared.
use crate::{EnhancedSettings, Renderer};
use glam::{Mat4, Vec3};

impl Renderer {
    pub(super) fn release_disabled_atmosphere(&mut self, settings: EnhancedSettings) {
        if !atmosphere_enabled(settings) {
            if let Some(pass) = self.atmosphere.as_mut() { pass.release_targets(); }
        }
        if !settings.volumetric_clouds && !settings.cloud_shadows && !settings.cloud_gap_shafts {
            if let Some(pass) = self.volumetric_clouds.as_mut() { pass.release_targets(); }
        } else if !settings.volumetric_clouds {
            if let Some(pass) = self.volumetric_clouds.as_mut() { pass.release_history(); }
        } else if !settings.cloud_shadows && !settings.cloud_gap_shafts {
            if let Some(pass) = self.volumetric_clouds.as_mut() { pass.release_shadow_field(); }
        }
        if !settings.sky_discs {
            if let Some(pass) = self.sky_discs.as_mut() { pass.release_targets(); }
            self.sky_disc_transforms.clear();
        }
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

    pub(super) fn prepare_atmosphere_stack(&mut self, encoder: &mut wgpu::CommandEncoder, vp: Mat4, eye: Vec3, far: f32, env: ao_scene::Environment) {
        let settings = self.enhanced_settings;
        if !atmosphere_enabled(settings) && !settings.volumetric_clouds && !settings.cloud_shadows && !settings.sky_discs { return; }
        let Some(frame) = self.enhanced_frame.as_ref() else { return; };
        let moon = if native_sun_contributes(env) { None } else { self.enhanced_moonlight() };
        self.atmosphere_native_layers.clear();
        self.atmosphere_native_layers.extend(self.sky.clouds.iter().map(|cloud| {
            let mut layer = cloud.layer.clone();
            if let Some((direction, gamma_rgb)) = moon {
                layer.light_direction = direction;
                layer.light = gamma_rgb;
            }
            layer
        }));
        self.atmosphere_layers.clear();
        self.atmosphere_layers.extend(self.atmosphere_native_layers.iter().map(|layer| crate::volumetric_clouds::density_layer(layer, eye, far, self.time)));
        let clouds = self.volumetric_clouds.as_mut().expect("startup cloud pipelines");
        if settings.volumetric_clouds || settings.cloud_shadows {
            clouds.prepare(&self.device, &self.queue, encoder, &frame.resolved_depth,
                frame.sun.as_ref(), frame.water_cloud.as_ref(), frame.size, vp, eye,
                &self.atmosphere_native_layers, self.time, far, settings.volumetric_clouds,
                settings.cloud_shadows || settings.cloud_gap_shafts, env);
        } else if settings.cloud_gap_shafts {
            clouds.prepare_cloud_field(&self.device, &self.queue, encoder, eye, &self.atmosphere_native_layers, self.time, far);
        }
    }

    pub(super) fn encode_atmosphere_stack(&mut self, encoder: &mut wgpu::CommandEncoder, vp: Mat4, eye: Vec3, far: f32, env: ao_scene::Environment) {
        let settings = self.enhanced_settings;
        if !atmosphere_enabled(settings) && !settings.cloud_shadows && !settings.sky_discs { return; }
        let (effect_env, moon_air) = effect_environment(env, self.enhanced_moonlight(), self.enhanced_shadow_moonlight(), settings);
        let Some(frame) = self.enhanced_frame.as_ref() else { return; };
        let cloud_field = self.volumetric_clouds.as_ref().and_then(|clouds| clouds.cloud_field());
        let directional_mask = frame.sun.as_ref().expect("atmosphere requests admitted-world coverage");
        if atmosphere_enabled(settings) {
            let pass = self.atmosphere.as_mut().expect("startup atmosphere pipelines");
            pass.encode(&self.device, &self.queue, encoder, &frame.resolved_depth, directional_mask, &frame.colour,
                frame.size, vp, eye, far, effect_env, moon_air, self.fog.as_ref(), self.atmosphere_base_height,
                crate::atmosphere::AtmosphereSwitches {
                    height_haze: settings.height_haze, land_haze: settings.land_haze, valley_mist: settings.valley_mist,
                    volumetric_shafts: settings.volumetric_shafts, cloud_gap_shafts: settings.cloud_gap_shafts,
                }, self.sun_shadows.as_ref().map(|shadows| &shadows.receiver), &self.atmosphere_layers, env.sun_dir[1], cloud_field.as_ref());
        }
        if settings.sky_discs {
            let pass = self.sky_discs.as_mut().expect("startup sky disc pipelines");
            pass.encode(&self.device, &self.queue, encoder, &frame.resolved_depth, directional_mask, &frame.colour, frame.size, vp, eye,
                &self.sky_disc_transforms, &self.sky_views, self.time, self.sky.pose.daylight,
                &self.atmosphere_layers, &self.sky_disc_sources);
        }
        if settings.cloud_shadows {
            self.volumetric_clouds.as_ref().expect("startup cloud pipelines").composite_world(encoder, &frame.colour);
        }
    }
}

fn atmosphere_enabled(settings: EnhancedSettings) -> bool {
    settings.height_haze || settings.land_haze || settings.valley_mist || settings.volumetric_shafts || settings.cloud_gap_shafts
}

fn native_sun_contributes(env: ao_scene::Environment) -> bool {
    env.sun_dir[1] > 0.0 && env.sun_color.into_iter().any(|value| value > 0.0)
}

pub(super) fn effect_environment(mut env: ao_scene::Environment, general_moon: Option<([f32; 3], [f32; 3])>,
    mapped_moon: Option<([f32; 3], [f32; 3])>, settings: EnhancedSettings) -> (ao_scene::Environment, bool) {
    if !native_sun_contributes(env) {
        // Exact encoded Moon maps win; gap-only rays may use the same shared
        // general Moon source without requiring an unrelated cascade switch.
        let general_moon = if settings.volumetric_shafts && !settings.cloud_gap_shafts { None } else { general_moon };
        if let Some((direction, gamma_rgb)) = mapped_moon.or(general_moon) {
            env.sun_dir = direction;
            env.sun_color = gamma_rgb.map(crate::srgb_to_linear);
            return (env, true);
        }
        env.sun_color = [0.0; 3];
    }
    (env, false)
}
