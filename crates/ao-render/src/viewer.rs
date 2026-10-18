use crate::{default_view, Camera, Renderer, Targets, Vec3};
use crate::gpu_stats::{GpuStatsExt, TextureViewStatsExt};
use anyhow::Result;
use ao_scene::Scene;
use std::collections::HashSet;
use std::sync::Arc;
use std::time::{Duration, Instant};
use winit::application::ApplicationHandler;
use winit::dpi::PhysicalSize;
use winit::event::{DeviceEvent, DeviceId, ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{KeyCode, ModifiersState, PhysicalKey};
use winit::monitor::MonitorHandle;
use winit::platform::macos::WindowExtMacOS;
use winit::window::{CursorGrabMode, Fullscreen, Window, WindowId};

#[path = "raw_mouse.rs"]
mod raw_mouse;
#[path = "window_prefs.rs"]
mod window_prefs;
#[path = "frame_diagnostics.rs"]
mod frame_diagnostics;
use frame_diagnostics::FrameDiagnostics;

/// What a [`Frontend`] can ask of the window/renderer between frames.
pub struct Host {
    /// Camera used for the next frame. Written back from free-fly controls while `fly`.
    pub camera: Camera,
    /// Exact camera membership from native collision; `None` uses finite source-liquid geometry.
    pub camera_underwater: Option<bool>,
    /// WASD + right-mouse-look free-fly controls (the viewer's default).
    pub fly: bool,
    /// Close the window after this frame.
    pub quit: bool,
    scene: Option<Scene>,
    repose: Option<Scene>,
    scene_generation: u64,
    /// Lens for the next frame (per-frame FOV changes of camera paths); `None` keeps the scene's lens.
    pub lens: Option<ao_scene::Lens>,
    /// Fog density multiplier of the `FogMode` pref ([`ao_scene::fog_mode_density_scale`]); `None` leaves it as it is; drained by the viewer.
    pub fog_density_scale: Option<f32>,
    /// Startup color-depth quality, native compression preference, and aomac Enhanced "Better Mip Filtering" (not retail).
    pub texture_policy: Option<(i32, bool, bool)>,
    /// Texture anisotropic filtering clamp (aomac Enhanced option, audit item 2; retail default 1x). `None` leaves it as it is.
    pub anisotropy: Option<u16>,
    /// Colour/depth MSAA sample count (aomac Enhanced option; not retail, default 1 = off). `None` leaves it as it is;
    /// downward-clamped to what the adapter supports by [`Renderer::set_msaa_samples`].
    pub msaa_samples: Option<u32>,
    /// Independent opt-in graphics effects and quality knobs; drained by both viewer frontends.
    /// Immediate fixture/evidence setting; waits for background compilation of an unseen combination.
    pub enhanced_settings: Option<crate::EnhancedSettings>,
    /// Live option request: keep drawing the current variant until the worker finishes.
    pub enhanced_switch: Option<crate::EnhancedSettings>,
    /// Session-only master switch; never rewrites individual feature preferences.
    pub enhanced_suspended: bool,
    /// Session-only GPU overlay and slow CPU-wall frame logging, both off by default.
    pub effect_gpu_overlay: bool,
    pub slow_frame_log: bool,
    /// Prior completed rolling GPU samples, refreshed before each frontend frame.
    pub effect_gpu_costs: Vec<crate::EffectGpuCost>,
    pub effect_gpu_error: Option<String>,
    enhanced_requested: Option<crate::EnhancedSettings>,
    enhanced_runtime_suspended: bool,
    effect_gpu_profiling_suppressed: bool,
    /// Root containing the user's day/night LUTs and optional per-playfield overrides.
    pub grade_lut_root: Option<std::path::PathBuf>,
    /// Actual own-character scene position, supplied every frame; never inferred from actor admission.
    pub post_focus: Option<[f32; 3]>,
    /// Interface scale (aomac Enhanced option, audit item 2): `None`/`Some(0)` = Auto (today's whole backing-scale
    /// rounding), `Some(1..=3)` = a fixed GUI-pixel scale, honoured regardless of window size ([`gui_scale`]).
    /// Drained every frame by the viewer; the very first frame (before the window exists) reads
    /// [`Frontend::interface_scale`] instead, so the login screen is never a frame late.
    pub interface_scale: Option<u8>,
    /// Skip the Spaces fullscreen animation and hide the menu bar/dock on fullscreen entry (aomac Enhanced option,
    /// audit item 6; winit `WindowExtMacOS::set_borderless_game`). Not drained: read whenever fullscreen is toggled.
    pub borderless_game: bool,
    /// Trackpad pinch gesture zooms the third-person camera (aomac Enhanced option, audit item 8; opt-in, not retail).
    /// Not drained: read whenever a `WindowEvent::PinchGesture` arrives.
    pub pinch_zoom: bool,
    /// Live character preference controlling authored `GAME.GameShowSpaceship` instances.
    pub spaceships_shown: bool,
    pub smooth_animations: bool,
    pub previous_visible_actors: HashSet<u32>,
    /// Live native liquid subdivision preference, not a liquid visibility switch.
    pub realistic_water: bool,
    /// EnvPrefs15/16, consumed by both the real viewer and offscreen harness.
    pub cloud_preferences: Option<(bool, bool)>,
    /// Native terrain quality / full-quality-radius DValues; consumed on the next frame.
    pub ground_detail: Option<(i32, i32)>,
    pub ground_rendering: i32,
    pub ground_textures: Option<std::collections::HashMap<ao_scene::TextureKey, ao_scene::Texture>>,
    /// Character preference, observed live by DisplaySystem (0x10078f75).
    pub wait_for_vert_sync: Option<bool>,
    /// DisplaySystem 0x100787ec / Randy 0x10042f76: synchronize the previous rendered frame at Flip.
    pub mouse_lag_fix: bool,
    /// Frontend request: capture the cursor and deliver [`GameInput::MouseMotion`] (mouse-look); applied after each frame/input.
    pub look: bool,
    /// Actor models to upload before the next frame (`key`, model scene), see [`Renderer::add_actor_model`]; drained by the viewer.
    /// Shared ownership keeps decoded mesh/texture arrays in place until the upload is drained.
    pub actor_models: Vec<(u64, std::sync::Arc<Scene>)>,
    /// Dynamic geometry replacements retaining previously uploaded textures; drained after model uploads.
    pub actor_model_updates: Vec<(u64, std::sync::Arc<Scene>)>,
    /// Model keys to retire before uploads and actor frames; drained by the viewer. See [`Renderer::remove_actor_model`].
    pub actor_model_removals: Vec<u64>,
    /// Destroyed actor ID ranges (`first`, `count`), not visibility culls; retired when the following frame replaces the final submission.
    pub actor_visual_retirements: Vec<(u32, u32)>,
    /// Destroyed logical actor ranges (`first`, `count`), reset before fresh frames, including same-ID replacements.
    pub actor_visual_resets: Vec<(u32, u32)>,
    /// The actors to draw this frame ([`ao_scene::ActorFrame`]); push every frame, the viewer drains it. Actors that are not pushed are forgotten.
    pub actors: Vec<ao_scene::ActorFrame>,
    /// Per-frame moving lights (worn/attached item lights, e.g. a Light Bar's embedded `RLight_t`, tracked to its
    /// attractor bone): push every frame ([`Renderer::set_dynamic_lights`]), the viewer applies and replaces them
    /// alongside `actors`. Not pushed = no dynamic lights this frame (an empty push, not a missing one, since the
    /// previous frame's lights must not linger once their source actor leaves view/range).
    pub dynamic_lights: Vec<ao_scene::Light>,
    /// Forget all actor models and actors before applying `actor_models` (a new zone).
    pub clear_actors: bool,
    /// Replace the live sky ([`LiveSky`]; `Some(None)` = static sky) of the windowed viewer before the next frame; drained by the viewer.
    pub live_sky: Option<Option<LiveSky>>,
    /// Set the live sky clock (game day time, seconds) of the running [`LiveSky`] (a server time resync); drained by the viewer.
    pub sky_clock: Option<f32>,
    /// Keyboard modifiers as of the last input event (what the original's `InputConfig_t::IsShiftDown` / `IsCtrlDown` read for mouse clicks).
    pub mods: ao_gui::Modifiers,
    /// Frontend request: hide the OS cursor because the frontend draws the game's own pointer in its draw list; applied after each frame/input.
    pub hide_cursor: bool,
    /// One-shot cursor position in GUI pixels (native split-count selection); applied only to the focused, uncaptured owned window.
    pub pointer_warp: Option<(f32, f32)>,
    /// Current weather wind in AO world coordinates (`GCComputeWind`, Gamecode 0x100cd993).
    pub effect_wind: [f32; 3],
    /// Live native rain/storm channels and static collision capability, in scene coordinates.
    pub weather_input: crate::WeatherInput,
    /// Current-frame character candidates; final body/visibility admission occurs in the renderer.
    pub grass_benders: Vec<crate::GrassBender>,
    /// Optional native surface capability, retained only while grass is enabled.
    pub grass_exposure: Option<crate::GrassExposure>,
    /// Current native weather wind for grass (zero when the current zone has no weather).
    pub grass_wind: [f32; 3],
    /// Current server-synchronized GameDayTime (0..6480 seconds); absent outside a game zone.
    pub effect_day_time: Option<f32>,
    /// Server-synchronized integer game seconds in the current day; absent until GameTime arrives.
    pub effect_game_seconds: Option<i32>,
    /// Actual active camera vehicle, not the preferred third-person setting.
    pub effect_first_person: bool,
    /// Native n3EngineClient +0x7c camera position, passed to VisualEnvFX +0x14.
    pub effect_environment_center: Option<[f32; 3]>,
    /// Native VisualEnvFX environment oscillator channels +0x90/+0x94, carried in X/Z.
    pub effect_environment_wind: Option<[f32; 3]>,
    /// RDB playfield resource tilemap id and flags, not the network playfield instance.
    pub effect_playfield: Option<(u32, u32)>,
    /// Native Light +0x104: ray direction, opposite the environment's vector towards the sun.
    pub effect_sun_direction: Option<[f32; 3]>,
    /// `n3Playfield_t::IsDungeon` from the loaded zone layout.
    pub effect_dungeon: Option<bool>,
    /// Native viewport sprites; submit each frame, drained with actors.
    pub screen_layers: Vec<super::ScreenLayer>,
    /// Authored textures for viewport layers; drained before screen rendering.
    pub screen_textures: Vec<(ao_scene::TextureKey,ao_scene::Texture)>,
    /// Ordered native depth-only clear visuals; submit each frame.
    pub viewport_depth_clears:Vec<super::ViewportDepthClear>,
    /// Shared native VisualFog state, also mutated by effect constructors/destructors.
    pub native_fog: Option<super::SharedNativeFog>,
    native_fog_preference: Option<i32>,
    /// Frontend request: write the current frame (world + GUI) as a PNG to this path once, off the render thread
    /// (aomac's F12 screenshot); `None` = no request. Drained by the viewer/`Offscreen`.
    pub screenshot: Option<std::path::PathBuf>,
    /// Informational, filled by the viewer before every `Frontend::frame` call (opposite direction from most
    /// `Host` fields): physical pixels per GUI pixel of the real window, 1 in `Offscreen` unless a test sets it.
    /// Lets the aomac Enhanced "Anti-aliased Text" option (not retail, audit item 13) rasterize TrueType glyphs
    /// at the real backing scale through `ao_gui::Gui::set_text_scale`.
    pub window_scale: u32,
}

fn present_mode(wait: bool) -> wgpu::PresentMode {
    if wait { wgpu::PresentMode::AutoVsync } else { wgpu::PresentMode::AutoNoVsync }
}

impl Host {
    /// Latest requested settings, with live options taking precedence over immediate fixture settings.
    pub fn enhanced(&self) -> Option<crate::EnhancedSettings> {
        if self.enhanced_suspended { Some(crate::EnhancedSettings::default()) }
        else { self.enhanced_switch.or(self.enhanced_settings).or(self.enhanced_requested) }
    }

    fn take_enhanced_requests(&mut self) -> (Option<crate::EnhancedSettings>, Option<crate::EnhancedSettings>) {
        let immediate = self.enhanced_settings.take();
        let live = self.enhanced_switch.take();
        if let Some(settings) = live.or(immediate) { self.enhanced_requested = Some(settings); }
        let changed = self.enhanced_runtime_suspended != self.enhanced_suspended;
        self.enhanced_runtime_suspended = self.enhanced_suspended;
        if self.enhanced_suspended {
            (None, Some(crate::EnhancedSettings::default()))
        } else if changed {
            (None, self.enhanced_requested)
        } else { (immediate, live) }
    }

    fn apply_world_options(&mut self, renderer: &mut Renderer) {
        let (immediate, live) = self.take_enhanced_requests();
        renderer.set_world_options(self.msaa_samples.take(), immediate, live);
        self.configure_gpu_diagnostics(renderer);
    }

    fn configure_gpu_diagnostics(&mut self, renderer: &mut Renderer) {
        let enabled = (self.effect_gpu_overlay || self.slow_frame_log) && !self.effect_gpu_profiling_suppressed;
        match renderer.set_effect_gpu_profiling(enabled) {
            Ok(()) => self.effect_gpu_error = None,
            Err(error) => self.effect_gpu_error = Some(error.to_string()),
        }
    }

    fn refresh_gpu_diagnostics(&mut self, renderer: &mut Renderer) {
        self.configure_gpu_diagnostics(renderer);
        if self.effect_gpu_profiling_suppressed { return; }
        if self.effect_gpu_overlay || self.slow_frame_log {
            renderer.poll_effect_gpu_costs();
            self.effect_gpu_costs.clear();
            self.effect_gpu_costs.extend_from_slice(renderer.effect_gpu_costs());
        } else { self.effect_gpu_costs.clear(); }
    }

    pub fn bind_native_fog(&mut self,state:super::SharedNativeFog) {
        if self.native_fog.as_ref().is_some_and(|old|std::rc::Rc::ptr_eq(old,&state)){return;}
        if let Some(mode)=self.native_fog_preference {let mut fog=state.get();fog.set_mode(mode);state.set(fog);}
        self.native_fog=Some(state);
    }
    pub fn set_native_fog_preference(&mut self,mode:i32) {
        if self.native_fog_preference==Some(mode){return;}
        self.native_fog_preference=Some(mode);
        if let Some(state)=&self.native_fog {let mut fog=state.get();fog.set_mode(mode);state.set(fog);}
        else {self.fog_density_scale=Some(ao_scene::fog_mode_density_scale(mode));}
    }
    /// Hands the queued actor models and the frame's actors to the renderer.
    fn apply_actors(&mut self, r: &mut Renderer) {
        r.set_grade_lut_source(self.grade_lut_root.as_deref(), self.effect_playfield.map(|(id, _)| id));
        if std::mem::take(&mut self.clear_actors) {
            r.clear_actors();
        }
        for key in self.actor_model_removals.drain(..) {
            r.remove_actor_model(key);
        }
        r.queue_actor_visual_retirements(&mut self.actor_visual_retirements);
        r.retire_actor_visuals(&self.actor_visual_resets);
        self.actor_visual_resets.clear();
        for (key, scene) in self.actor_models.drain(..) {
            r.add_actor_model(key, &scene);
        }
        for (key, scene) in self.actor_model_updates.drain(..) {
            r.update_actor_model(key, &scene);
        }
        r.set_actors(std::mem::take(&mut self.actors));
        r.set_post_focus(self.post_focus.take());
        r.set_enhanced_grass_inputs(&mut self.grass_benders, [self.grass_wind[0], self.grass_wind[1], -self.grass_wind[2]]);
        r.set_water_wind([self.grass_wind[0], self.grass_wind[1], -self.grass_wind[2]]);
        r.set_camera_underwater(self.camera_underwater);
        r.set_enhanced_grass_exposure(self.grass_exposure.clone());
        r.set_weather_input(self.weather_input.clone());
        r.set_dynamic_lights(&std::mem::take(&mut self.dynamic_lights));
        for (key,texture) in self.screen_textures.drain(..) {r.upload_screen_texture(key,&texture);}
        let fog=self.native_fog.as_ref().map(|state|state.get());
        if let Some(fog)=fog {r.set_fog_density_scale(fog.weight);}
        r.set_screen_effects(std::mem::take(&mut self.screen_layers),fog.is_some_and(|fog|!fog.enabled));
        r.set_viewport_depth_clears(std::mem::take(&mut self.viewport_depth_clears));
    }

    /// A host without a window or renderer (headless tests of [`Frontend`]s); scenes handed to it are kept, never drawn.
    pub fn headless() -> Self {
        let grade_lut_root = None;
        let enhanced_suspended = false;
        let effect_gpu_overlay = false;
        let slow_frame_log = false;
        let effect_gpu_costs = Vec::new();
        let effect_gpu_error = None;
        let enhanced_requested = None;
        let enhanced_runtime_suspended = false;
        Host { effect_gpu_profiling_suppressed: false, enhanced_suspended, effect_gpu_overlay, slow_frame_log, effect_gpu_costs, effect_gpu_error, enhanced_requested, enhanced_runtime_suspended, weather_input: Default::default(), post_focus: None, grade_lut_root, camera_underwater: None, enhanced_settings: None, enhanced_switch: None, grass_wind: [0.0; 3], grass_exposure: None, grass_benders: vec![], actor_visual_resets: vec![], actor_visual_retirements: vec![], pointer_warp: None, smooth_animations: true, previous_visible_actors: HashSet::new(), ground_textures: None, texture_policy: None, anisotropy: None, msaa_samples: None, interface_scale: None, borderless_game: false, pinch_zoom: false, cloud_preferences: None, ground_rendering: 3, camera: Camera::look_at(Vec3::ZERO, -Vec3::Z), fly: false, quit: false, scene: None, repose: None, scene_generation: 0, lens: None, fog_density_scale: None, spaceships_shown: true, realistic_water: true, ground_detail: None, wait_for_vert_sync: None, mouse_lag_fix: false, look: false, actor_models: vec![], actor_model_updates: vec![], actor_model_removals: vec![], actors: vec![], dynamic_lights: vec![], clear_actors: false, live_sky: None, sky_clock: None, mods: Default::default(), hide_cursor: false, effect_wind: [0.0; 3],effect_day_time:None,effect_game_seconds:None,effect_first_person:false,effect_environment_center:None,effect_environment_wind:None,effect_playfield:None,effect_sun_direction:None,effect_dungeon:None,screen_layers:vec![],screen_textures:vec![],viewport_depth_clears:vec![],native_fog:None,native_fog_preference:None,screenshot:None,window_scale:1 }
    }

    /// Updates vertex positions/instance transforms of the current scene in place ([`Renderer::repose`]).
    pub fn repose(&mut self, scene: Scene) {
        self.repose = Some(scene);
    }

    /// Changes whenever replacing the scene invalidates cached actor uploads.
    pub fn scene_generation(&self) -> u64 {
        self.scene_generation
    }

    /// Replaces the rendered scene (uploaded before the next frame).
    pub fn set_scene(&mut self, scene: Scene) {
        self.effect_sun_direction = scene.environment.map(|env| env.sun_dir.map(|v| -v));
        self.effect_dungeon = None;
        self.weather_input = Default::default();
        self.scene_generation = self.scene_generation.wrapping_add(1);
        // Queued poses/models belong to the replaced topology, not the newly uploaded scene.
        self.repose = None;
        self.actor_models.clear();
        self.actor_model_updates.clear();
        self.actor_model_removals.clear();
        self.actor_visual_retirements.clear();
        self.actor_visual_resets.clear();
        self.actors.clear();
        self.dynamic_lights.clear();
        self.screen_layers.clear();
        self.screen_textures.clear();
        self.viewport_depth_clears.clear();
        self.clear_actors = true;
        self.scene = Some(scene);
    }
}

/// Raw game input forwarded while the frontend has free-fly off (physical keys, so bindings are layout independent).
#[derive(Clone, Copy, Debug)]
pub enum GameInput {
    Key { code: KeyCode, pressed: bool, repeat: bool },
    /// Relative hardware mouse counts, summed once per frame while [`Host::look`] is set,
    /// with that frame's delta in seconds (not the previous frontend frame's delta).
    MouseMotion { dx: f32, dy: f32, dt: f32 },
    /// The window lost focus; release game input even while new presses are disabled.
    FocusLost,
    /// Trackpad pinch gesture (aomac Enhanced option, audit item 8); positive = zooming in, same convention as
    /// `winit::event::WindowEvent::PinchGesture`. Only forwarded while [`Host::pinch_zoom`] is set.
    Pinch(f32),
}

/// The windowed GUI application: draws the original AO GUI (`ao_gui`) over the 3D scene.
pub trait Frontend {
    fn gui(&self) -> &ao_gui::Gui;
    /// One input event, positions in GUI pixels.
    fn input(&mut self, ev: ao_gui::InputEvent, host: &mut Host);
    /// Raw key/mouse-look input while `host.fly` is off (default: ignored).
    fn game_input(&mut self, _ev: GameInput, _host: &mut Host) {}
    /// Authored traffic-ship sound side effect of the last rendered FXS frame.
    fn mover_sound(&mut self, _event: ao_scene::mover::SoundEvent, _host: &Host) {}
    /// Window-level scale known before any frame runs (aomac Enhanced option, audit item 2: Interface Scale); read
    /// once by [`State::new`] so the very first window/frame (the login screen) already honours it. `0` = Auto.
    fn interface_scale(&self) -> u8 { 0 }
    /// Per-frame logic (`dt` seconds, `size` = window in GUI pixels); returns the GUI draw list to paint.
    fn frame(&mut self, dt: f32, size: (u32, u32), host: &mut Host) -> ao_gui::DrawList;
}

type SkyPoseSource = Box<dyn FnMut(f32, &mut Scene) + Send>;

/// Live time of day: `source(day_time)` returns a scene holding only the sky, its new textures and the environment
/// ([`Renderer::set_sky`]); it runs on a worker thread, about twice a second of real time.
pub struct LiveSky {
    /// Game day time (seconds) at start.
    pub start: f32,
    /// Game seconds per real second.
    pub scale: f32,
    pub source: Box<dyn FnMut(f32) -> Scene + Send>,
    /// Exact per-frame poses of already-loaded resources; never waits for the worker.
    pub pose: Option<SkyPoseSource>,
}

/// Worker thread + clock of a [`LiveSky`].
struct LiveRun {
    day_time: f64,
    scale: f32,
    since: f32,
    busy: bool,
    req: std::sync::mpsc::Sender<f32>,
    resp: std::sync::mpsc::Receiver<Scene>,
    pose: Option<SkyPoseSource>,
}

impl LiveRun {
    fn new(live: LiveSky) -> Self {
        let (req, req_rx) = std::sync::mpsc::channel::<f32>();
        let (resp_tx, resp) = std::sync::mpsc::channel();
        let mut source = live.source;
        std::thread::spawn(move || {
            for t in req_rx {
                if resp_tx.send(source(t)).is_err() {
                    break;
                }
            }
        });
        Self { day_time: f64::from(live.start), scale: live.scale, since: f32::MAX, busy: false, req, resp, pose: live.pose }
    }

    /// Advances the clock by `dt` real seconds; the freshly computed sky, if one arrived.
    fn tick(&mut self, dt: f32) -> Option<Scene> {
        self.day_time += f64::from(dt) * f64::from(self.scale);
        self.since += dt;
        let got = self.resp.try_recv().ok();
        self.busy &= got.is_none();
        if !self.busy && self.since >= 0.5 && self.req.send(self.day_time as f32).is_ok() {
            (self.busy, self.since) = (true, 0.0);
        }
        got
    }

    fn pose(&mut self, scene: &mut Scene) {
        if let Some(pose) = &mut self.pose { pose(self.day_time as f32, scene); }
    }

    fn apply_host(live: &mut Option<Self>, host: &mut Host) {
        if let Some(next) = host.live_sky.take() {
            *live = next.map(Self::new);
        }
        if let (Some(time), Some(run)) = (host.sky_clock.take(), live.as_mut()) {
            run.day_time = f64::from(time);
        }
    }

    fn advance(live: &mut Option<Self>, renderer: &mut Renderer, dt: f32) -> Option<[f32; 3]> {
        if let Some(run) = live {
            renderer.day_time_rate = run.scale;
            if let Some(sky) = run.tick(dt) {
                renderer.set_sky(&sky);
            }
            if run.pose.is_some() {
                run.pose(&mut renderer.sky.pose);
                renderer.apply_sky_pose();
                return renderer.sky.pose.environment.map(|env| env.sun_dir.map(|v| -v));
            } else if let Some(env) = renderer.sky.pose.environment {
                return Some(env.sun_dir.map(|v| -v));
            }
        }
        None
    }
}

struct Gui {
    renderer: crate::GuiRenderer,
    frontend: Box<dyn Frontend>,
    host: Host,
    cursor: (f32, f32),
    mods: ao_gui::Modifiers,
    /// Physical pixels per GUI pixel.
    scale: u32,
    /// Fractional trackpad `PixelDelta` scroll not yet accumulated into a whole 40 px notch (`gui_input`'s
    /// `MouseWheel` case); a mouse wheel's `LineDelta` never touches this. Without it, each small trackpad event
    /// rounds to a fresh whole `dy`, so a scroll gesture either jumps a full zoom level per event (`hud_map.rs`'s
    /// "any nonzero dy = one zoom step") or never moves at all (`interact_ptrade.rs`'s `dy as i64` truncation).
    scroll_accum: f32,
    /// Last `Host::interface_scale` the frontend asked for (audit item 2); `scale`/`Host::interface_scale` itself is
    /// drained every frame, so `ScaleFactorChanged` (a separate event, after the drain) needs this to stay sticky.
    requested_scale: u8,
}

/// Physical pixels per GUI pixel. `requested` is [`Host::interface_scale`] (`None`/`Some(0)` = Auto, today's whole
/// backing-scale rounding); explicit scales are honoured even when they reduce the logical viewport below 1024x768.
fn gui_scale(scale_factor: f64, requested: Option<u8>, _physical: (u32, u32)) -> u32 {
    let auto = (scale_factor.round() as u32).max(1);
    match requested {
        None | Some(0) => auto,
        Some(n) => u32::from(n).max(1),
    }
}

/// Accumulates a trackpad's fractional `MouseScrollDelta::PixelDelta` into whole 40-pixel-per-notch steps (the same
/// unit `LineDelta`/`Wheel::dy` already use), carrying the leftover fraction to the next event instead of rounding
/// it away. Without this, a scroll gesture sent as many small pixel events either jumps a whole notch per event
/// (any consumer that treats nonzero `dy` as one step, e.g. `hud_map.rs`'s map zoom) or never moves at all (a
/// consumer that truncates `dy` to an integer, e.g. `interact_ptrade.rs`'s trade-list scroll).
fn accumulate_scroll(accum: &mut f32, pixel_y: f32) -> Option<f32> {
    *accum += pixel_y / 40.0;
    let dy = accum.trunc();
    *accum -= dy;
    (dy != 0.0).then_some(dy)
}

/// winit -> GUI input. Cursor positions are physical pixels / `scale`; Cmd counts as Ctrl (macOS shortcuts).
fn gui_input(ev: &WindowEvent, cursor: &mut (f32, f32), mods: &mut ao_gui::Modifiers, scale: u32, scroll_accum: &mut f32) -> Option<ao_gui::InputEvent> {
    use ao_gui::{InputEvent as I, Key as K};
    use winit::keyboard::{Key, NamedKey};
    let s = scale as f32;
    match ev {
        WindowEvent::CursorMoved { position, .. } => {
            *cursor = (position.x as f32 / s, position.y as f32 / s);
            Some(I::MouseMove { x: cursor.0, y: cursor.1 })
        }
        WindowEvent::MouseInput { state, button, .. } => {
            let button = match button {
                MouseButton::Left => ao_gui::MouseButton::Left,
                MouseButton::Right => ao_gui::MouseButton::Right,
                MouseButton::Middle => ao_gui::MouseButton::Middle,
                _ => return None,
            };
            let (x, y) = *cursor;
            Some(if *state == ElementState::Pressed { I::MouseDown { x, y, button } } else { I::MouseUp { x, y, button } })
        }
        WindowEvent::MouseWheel { delta, .. } => match delta {
            MouseScrollDelta::LineDelta(_, y) => Some(I::Wheel { x: cursor.0, y: cursor.1, dy: *y }),
            // Trackpads send many small fractional pixel steps per gesture; a whole-level consumer (`hud_map.rs`'s
            // map zoom, any nonzero dy = one level) or a truncating one (`interact_ptrade.rs`'s `dy as i64`) both
            // need accumulation across events into whole 40 px notches, not a fresh round-per-event.
            MouseScrollDelta::PixelDelta(p) => accumulate_scroll(scroll_accum, p.y as f32).map(|dy| I::Wheel { x: cursor.0, y: cursor.1, dy }),
        },
        WindowEvent::ModifiersChanged(m) => {
            let st = m.state();
            *mods = ao_gui::Modifiers { shift: st.shift_key(), ctrl: st.control_key() || st.super_key(), alt: st.alt_key() };
            None
        }
        WindowEvent::KeyboardInput { event, .. } => {
            let pressed = event.state == ElementState::Pressed;
            let key: Option<K> = match &event.logical_key {
                Key::Named(NamedKey::Backspace) => Some(K::Backspace),
                Key::Named(NamedKey::Delete) => Some(K::Delete),
                Key::Named(NamedKey::ArrowLeft) => Some(K::Left),
                Key::Named(NamedKey::ArrowRight) => Some(K::Right),
                Key::Named(NamedKey::ArrowUp) => Some(K::Up),
                Key::Named(NamedKey::ArrowDown) => Some(K::Down),
                Key::Named(NamedKey::Home) => Some(K::Home),
                Key::Named(NamedKey::End) => Some(K::End),
                Key::Named(NamedKey::Enter) => Some(K::Enter),
                Key::Named(NamedKey::Tab) => Some(K::Tab),
                Key::Named(NamedKey::Escape) => Some(K::Escape),
                Key::Character(c) => Some(K::Letter(c.chars().next()?.to_ascii_lowercase())),
                // keys without a GUI meaning (Space, ...) still type their text into a focused field
                _ => None,
            };
            let text = event.text.as_deref().filter(|t| pressed && !mods.ctrl && !t.chars().any(char::is_control));
            match (text, key) {
                (Some(t), _) => Some(I::Text(t.to_string())),
                (None, Some(key)) => Some(I::Key { key, pressed, mods: *mods }),
                _ => None,
            }
        }
        _ => None,
    }
}

struct State {
    window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    renderer: Renderer,
    targets: Targets,
    cam: Camera,
    keys: HashSet<KeyCode>,
    speed: f32,
    looking: bool,
    focused: bool,
    /// Raw winit keyboard modifiers (not the collapsed `ao_gui::Modifiers`): Ctrl+Cmd+F (audit item 6) needs both
    /// physical modifiers, unlike the GUI's "Cmd counts as Ctrl" shortcuts.
    mods: ModifiersState,
    raw_mouse: raw_mouse::RawMouse,
    cursor_hidden: bool,
    last: Instant,
    frame_deadline: Instant,
    display_period: Option<Duration>,
    stat_t: Instant,
    stat_frames: u32,
    perf: bool,
    cpu: f32,
    previous_submission: Option<wgpu::SubmissionIndex>,
    // PERF-only seconds: frame wall, acquisition, GUI/frame work, reconfigure, present.
    perf_times: [f64; 5],
    perf_frames: u32,
    perf_gap: f64,
    gui: Option<Gui>,
    /// Seconds since start (drives `Submesh::uv_scroll`).
    clock: f32,
    live: Option<LiveRun>,
    hook: Option<FrameHook>,
    /// `Host::screenshot`, drained from `run_gui` and captured after this frame renders (`frame`'s F12 handling).
    pending_screenshot: Option<std::path::PathBuf>,
}

/// Called once per frame with listener position, delta, and renderer (including drainable ship sounds).
pub type FrameHook = Box<dyn FnMut(Vec3, f32, &mut Renderer)>;

struct App {
    scene: Scene,
    frontend: Option<Box<dyn Frontend>>,
    live: Option<LiveSky>,
    hook: Option<FrameHook>,
    state: Option<State>,
    error: Option<anyhow::Error>,
}

/// Opens the interactive viewer; returns when the window closes.
pub fn run_viewer(scene: Scene) -> Result<()> {
    run(scene, None, None, None)
}

/// [`run_viewer`] with a moving sun and sky ([`LiveSky`]).
pub fn run_viewer_live(scene: Scene, live: LiveSky) -> Result<()> {
    run(scene, None, Some(live), None)
}

/// [`run_viewer`]/[`run_viewer_live`] calling `hook(camera position, dt, renderer)` every frame.
pub fn run_viewer_hooked(scene: Scene, live: Option<LiveSky>, hook: FrameHook) -> Result<()> {
    run(scene, None, live, Some(hook))
}

/// Opens the window with `frontend` drawn over `scene` (free-fly off until the frontend enables it).
pub fn run_frontend(scene: Scene, frontend: impl Frontend + 'static) -> Result<()> {
    run(scene, Some(Box::new(frontend)), None, None)
}

// Missing native refresh information falls back to GPU vsync, never a guessed Hz.
fn display_period(rate_millihertz: Option<u32>) -> Option<Duration> {
    rate_millihertz.filter(|rate| *rate != 0).map(|rate| Duration::from_secs_f64(1000.0 / f64::from(rate)))
}

fn frame_deadline(start: Instant, end: Instant, period: Duration) -> Instant {
    let next = start + period;
    next.max(end)
}

fn wait_for_frame(mode: wgpu::PresentMode, period: Option<Duration>, deadline: Instant, now: Instant) -> bool {
    matches!(mode, wgpu::PresentMode::AutoVsync | wgpu::PresentMode::Fifo | wgpu::PresentMode::FifoRelaxed)
        && period.is_some() && now < deadline
}

fn run(scene: Scene, frontend: Option<Box<dyn Frontend>>, live: Option<LiveSky>, hook: Option<FrameHook>) -> Result<()> {
    let el = EventLoop::new()?;
    el.set_control_flow(ControlFlow::Poll);
    let mut app = App { scene, frontend, live, hook, state: None, error: None };
    el.run_app(&mut app)?;
    app.error.map_or(Ok(()), Err)
}

impl State {
    fn new(el: &ActiveEventLoop, scene: &Scene, frontend: Option<Box<dyn Frontend>>, live: Option<LiveSky>, hook: Option<FrameHook>) -> Result<Self> {
        let saved = window_prefs::load();
        // audit item 7: place on the saved monitor if it's still connected, else the primary one; clamp the saved
        // position fully onto it (a display unplugged/resized/rearranged since the last run must not open off-screen).
        let monitor = saved.as_ref().and_then(|s| s.monitor.as_deref())
            .and_then(|name| el.available_monitors().find(|m| m.name().as_deref() == Some(name)))
            .or_else(|| el.primary_monitor());
        // `Saved::width/height` are logical pixels (audit item 7): resolved against *this* run's monitor scale, so a
        // window saved on a 1x display and restored on a 2x one (or back) still looks the same physical size.
        let target_scale = monitor.as_ref().map_or(1.0, MonitorHandle::scale_factor);
        let (sw, sh) = saved.as_ref().map_or((1280, 800), |s| ((f64::from(s.width) * target_scale).round() as u32, (f64::from(s.height) * target_scale).round() as u32));
        let mut attrs = Window::default_attributes().with_title("aomac").with_inner_size(winit::dpi::PhysicalSize::new(sw, sh));
        if let Some(pos) = saved.as_ref().and_then(|s| s.pos) {
            let pos = monitor.as_ref().map_or(pos, |m| {
                window_prefs::clamp_to_monitor(pos, (sw, sh), (m.position().x, m.position().y), (m.size().width, m.size().height))
            });
            attrs = attrs.with_position(winit::dpi::PhysicalPosition::new(pos.0, pos.1));
        }
        // audit item 6: fullscreen remembered across restarts.
        if saved.as_ref().is_some_and(|s| s.fullscreen) {
            attrs = attrs.with_fullscreen(Some(Fullscreen::Borderless(monitor.clone())));
        }
        let window = Arc::new(el.create_window(attrs)?);
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let surface = instance.create_surface(window.clone())?;
        let mut renderer = Renderer::new(&instance, Some(&surface))?;
        renderer.upload(scene);
        let size = window.inner_size();
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format: renderer.format,
            width: size.width.max(1),
            height: size.height.max(1),
            present_mode: if std::env::var_os("AOMAC_NOVSYNC").is_some() { wgpu::PresentMode::AutoNoVsync } else { wgpu::PresentMode::AutoVsync },
            // Minimum wgpu/Metal allows (`CAMetalLayer.maximumDrawableCount` = this + 1 = 2, Apple's floor):
            // fewer buffered drawables means less input-to-photon latency, at the cost of occasionally blocking
            // `get_current_texture` a little longer for the compositor to free a drawable.
            desired_maximum_frame_latency: 1,
            alpha_mode: wgpu::CompositeAlphaMode::Auto,
            view_formats: vec![],
        };
        surface.configure(&renderer.device, &config);
        let targets = Targets::new(&renderer, config.width, config.height);
        let (eye, at) = default_view(scene);
        let speed = (renderer.radius() * 0.15).max(5.0);
        if std::env::var_os("AOMAC_PERF").is_some() {
            eprintln!("start eye {eye:?} at {at:?}, scene bounds {:?}", crate::scene_bounds(scene));
        }
        let cam = Camera::look_at(eye, at);
        let gui = frontend.map(|frontend| {
            let renderer = crate::GuiRenderer::new(&renderer, frontend.gui());
            // audit item 2: the requested scale is known before this first frame runs, so the login screen is never a frame late.
            let requested_scale = frontend.interface_scale();
            let scale = gui_scale(window.scale_factor(), Some(requested_scale), (config.width, config.height));
            Gui { renderer, frontend, host: Host { camera: cam, effect_sun_direction: scene.environment.map(|env| env.sun_dir.map(|v| -v)), ..Host::headless() }, cursor: (0.0, 0.0), mods: Default::default(), scale, requested_scale, scroll_accum: 0.0 }
        });
        let now = Instant::now();
        let focused = window.has_focus();
        let display_period = display_period(window.current_monitor().and_then(|monitor| monitor.refresh_rate_millihertz()));
        Ok(Self {
            window,
            surface,
            config,
            renderer,
            targets,
            cam,
            keys: HashSet::new(),
            speed,
            looking: false,
            focused,
            mods: ModifiersState::empty(),
            raw_mouse: raw_mouse::RawMouse::new(),
            cursor_hidden: false,
            last: now,
            frame_deadline: now,
            display_period,
            stat_t: now,
            stat_frames: 0,
            cpu: 0.0,
            previous_submission: None,
            perf: std::env::var_os("AOMAC_PERF").is_some(),
            perf_times: [0.0; 5],
            perf_frames: 0,
            perf_gap: 0.0,
            gui,
            clock: 0.0,
            live: live.map(LiveRun::new),
            hook,
            pending_screenshot: None,
        })
    }

    /// Saves size, position, monitor and fullscreen state for the next launch (audit items 6/7; `window_prefs::Saved`).
    fn save_window(&self) {
        let pos = self.window.outer_position().ok().map(|p| (p.x, p.y));
        let monitor = self.window.current_monitor().and_then(|m| m.name());
        let scale = self.window.scale_factor();
        let (width, height) = ((f64::from(self.config.width) / scale).round() as u32, (f64::from(self.config.height) / scale).round() as u32);
        window_prefs::save(&window_prefs::Saved { width, height, pos, monitor, fullscreen: self.window.fullscreen().is_some() });
    }

    fn fly(&self) -> bool {
        self.gui.as_ref().is_none_or(|g| g.host.fly)
    }

    fn set_look(&mut self, on: bool) {
        let on = on && self.focused;
        self.looking = on;
        self.raw_mouse.set_active(on && !self.fly());
        if on {
            let _ = self.window.set_cursor_grab(CursorGrabMode::Locked).or_else(|_| self.window.set_cursor_grab(CursorGrabMode::Confined));
        } else {
            let _ = self.window.set_cursor_grab(CursorGrabMode::None);
        }
        self.sync_cursor();
    }

    /// The OS cursor is hidden while the mouse looks and while the frontend draws its own pointer ([`Host::hide_cursor`]).
    fn sync_cursor(&mut self) {
        let hide = self.looking || self.gui.as_ref().is_some_and(|g| g.host.hide_cursor);
        if hide != self.cursor_hidden {
            self.cursor_hidden = hide;
            self.window.set_cursor_visible(!hide);
        }
        if let Some(g) = &mut self.gui {
            if let Some((x, y)) = g.host.pointer_warp.take().filter(|_| self.focused && self.window.has_focus() && !self.looking) {
                let pos = winit::dpi::PhysicalPosition::new(x as f64 * g.scale as f64, y as f64 * g.scale as f64);
                match self.window.set_cursor_position(pos) {
                    Ok(()) => g.cursor = (x, y),
                    Err(e) => eprintln!("cursor position: {e}"),
                }
            }
        }
    }

    /// Deliver counts while the button state that collected them is still active.
    /// In particular, release must not clear an entire between-redraw drag.
    fn flush_mouse(&mut self, dt: f32) {
        let Some(g) = &mut self.gui else { return };
        let (dx, dy) = self.raw_mouse.take();
        g.host.camera = self.cam;
        if !g.host.fly && (dx != 0.0 || dy != 0.0) {
            g.frontend.game_input(GameInput::MouseMotion { dx, dy, dt }, &mut g.host);
        }
        self.cam = g.host.camera;
    }

    /// Runs the frontend; returns its draw list (`None` without a frontend). True in `.1` = quit.
    fn run_gui(&mut self, dt: f32, diagnostics: &mut FrameDiagnostics) -> (Option<ao_gui::DrawList>, bool) {
        self.flush_mouse(dt);
        let Some(g) = &mut self.gui else { return (None, false) };
        // audit item 2: live-apply a scale change from the Enhanced page without a restart.
        if let Some(requested) = g.host.interface_scale.take() {
            g.requested_scale = requested;
            let scale = gui_scale(self.window.scale_factor(), Some(requested), (self.config.width, self.config.height));
            if scale != g.scale {
                let ratio = g.scale as f32 / scale as f32;
                g.cursor.0 *= ratio;
                g.cursor.1 *= ratio;
                g.scale = scale;
            }
        }
        let size = (self.config.width / g.scale, self.config.height / g.scale);
        self.renderer.visible_actor_ids(&mut g.host.previous_visible_actors);
        g.host.window_scale = g.scale;
        g.host.refresh_gpu_diagnostics(&mut self.renderer);
        diagnostics.mark(1);
        for event in self.renderer.take_mover_sounds() { g.frontend.mover_sound(event, &g.host); }
        let list = g.frontend.frame(dt, size, &mut g.host);
        diagnostics.mark(2);
        if let Some(wait) = g.host.wait_for_vert_sync.take() {
            let mode = present_mode(wait && std::env::var_os("AOMAC_NOVSYNC").is_none());
            if self.config.present_mode != mode {
                self.config.present_mode = mode;
                self.surface.configure(&self.renderer.device, &self.config);
            }
        }
        self.cam = g.host.camera;
        if let Some((quality, compressed, better_mips)) = g.host.texture_policy.take() {
            self.renderer.set_texture_policy(quality, compressed, better_mips);
        }
        if let Some(level) = g.host.anisotropy.take() {
            self.renderer.set_anisotropy(level);
        }
        g.host.apply_world_options(&mut self.renderer);
        if let Some(scene) = g.host.scene.take() {
            self.renderer.upload(&scene);
            self.speed = (self.renderer.radius() * 0.15).max(5.0);
        }
        if let Some(scene) = g.host.repose.take() {
            self.renderer.repose(&scene);
        }
        LiveRun::apply_host(&mut self.live, &mut g.host);
        if let Some(lens) = g.host.lens.take() {
            self.renderer.set_lens(lens);
        }
        if let Some(s) = g.host.fog_density_scale.take() {
            self.renderer.set_fog_density_scale(s);
        }
        self.renderer.set_spaceships_shown(g.host.spaceships_shown);
        self.renderer.set_realistic_water(g.host.realistic_water);
        if let Some((realistic,simple))=g.host.cloud_preferences.take() {self.renderer.set_cloud_preferences(realistic,simple,false);}
        if let Some((quality, radius)) = g.host.ground_detail.take() {
            self.renderer.set_ground_detail(quality, radius);
        }
        self.renderer.set_ground_rendering(g.host.ground_rendering);
        if let Some(textures) = g.host.ground_textures.take() { self.renderer.refresh_ground_textures(&textures); }
        self.pending_screenshot = g.host.screenshot.take();
        g.host.apply_actors(&mut self.renderer);
        diagnostics.mark(3);
        (Some(list), g.host.quit)
    }

    fn paint_gui(&mut self, list: ao_gui::DrawList, view: &wgpu::TextureView) {
        let Some(g) = &mut self.gui else { return };
        g.renderer.draw(&self.renderer, view, (self.config.width, self.config.height), g.scale, g.frontend.gui(), &list);
    }

    /// F12 screenshot (`Host::screenshot`): re-renders the just-drawn frame (world + GUI) into a one-off
    /// `RENDER_ATTACHMENT | COPY_SRC` texture (render-to-texture readback, not `COPY_SRC` on the swapchain itself --
    /// macOS `CAMetalLayer` drawables don't reliably support it) and saves it as a PNG off the render thread. Only
    /// runs on the rare frame a screenshot is requested, so it costs nothing otherwise.
    fn capture_screenshot(&mut self, path: std::path::PathBuf, gui_list: Option<ao_gui::DrawList>) {
        let (width, height) = (self.config.width, self.config.height);
        let tex = self.renderer.device.create_texture_tracked(&wgpu::TextureDescriptor {
            label: Some("F12 screenshot"),
            size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: self.renderer.format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = tex.create_view_tracked(&Default::default());
        self.renderer.render(&view, &self.targets, &self.cam);
        if let (Some(list), Some(g)) = (gui_list, &mut self.gui) {
            g.renderer.draw(&self.renderer, &crate::GuiRenderer::view(&tex), (width, height), g.scale, g.frontend.gui(), &list);
        }
        if let Err(e) = crate::capture_frame_to_png(&self.renderer, &tex, width, height, path.clone()) {
            eprintln!("screenshot: {}: {e}", path.display());
        }
    }

    fn finish_perf_frame(&mut self, start: Instant) {
        if self.perf {
            self.perf_times[0] += start.elapsed().as_secs_f64();
            self.perf_frames += 1;
        }
    }

    /// True = quit requested.
    fn frame(&mut self) -> bool {
        let _frame = crate::pipeline_creation::FrameGuard::enter();
        let now = Instant::now();
        let mut diagnostics = FrameDiagnostics::new(self.gui.as_ref().is_some_and(|g| g.host.slow_frame_log));
        if self.perf {
            self.perf_gap = self.perf_gap.max((now - self.last).as_secs_f64());
        }
        let work_start = self.perf.then(Instant::now);
        // Retail high-resolution Timer_t bounds elapsed time at 100 seconds, not 0.1.
        let dt = (now - self.last).as_secs_f32().min(100.0);
        self.last = now;
        self.clock += dt;
        self.renderer.time = self.clock;
        self.renderer.advance_grass_wind(dt);
        if let Some(sun) = LiveRun::advance(&mut self.live, &mut self.renderer, dt) {
            if let Some(gui) = &mut self.gui { gui.host.effect_sun_direction = Some(sun); }
        }

        // HiDPI/resize: follow the physical size every frame.
        if let Some(t) = work_start { self.perf_times[2] += t.elapsed().as_secs_f64(); }
        let PhysicalSize { width, height } = self.window.inner_size();
        if width == 0 || height == 0 {
            self.finish_perf_frame(now);
            return false;
        }
        if (width, height) != (self.config.width, self.config.height) {
            let t = self.perf.then(Instant::now);
            self.config.width = width;
            self.config.height = height;
            self.surface.configure(&self.renderer.device, &self.config);
            self.targets = Targets::new(&self.renderer, width, height);
            if let Some(t) = t { self.perf_times[3] += t.elapsed().as_secs_f64(); }
            self.save_window();
        }

        let work_start = self.perf.then(Instant::now);
        if self.fly() {
            let k = |c| self.keys.contains(&c) as i32 as f32;
            let fast = if self.keys.contains(&KeyCode::ShiftLeft) || self.keys.contains(&KeyCode::ShiftRight) { 5.0 } else { 1.0 };
            let step = self.speed * fast * dt;
            let up = k(KeyCode::Space) + k(KeyCode::KeyE) - k(KeyCode::ControlLeft) - k(KeyCode::KeyQ);
            let c = &mut self.cam;
            c.pos += (c.forward() * (k(KeyCode::KeyW) - k(KeyCode::KeyS)) + c.right() * (k(KeyCode::KeyD) - k(KeyCode::KeyA)) + Vec3::Y * up) * step;
        }
        self.raw_mouse.set_active(self.looking && self.focused && !self.fly());
        let (gui_out, quit) = self.run_gui(dt, &mut diagnostics);
        self.renderer.settle_enhanced_world();
        if !self.targets.matches_renderer(&self.renderer) {
            self.targets = Targets::new(&self.renderer, self.config.width, self.config.height);
        }
        if let Some(want) = self.gui.as_ref().filter(|_| !self.fly()).map(|g| g.host.look) {
            if want != self.looking {
                self.set_look(want);
            }
        }
        self.sync_cursor();
        if let Some(h) = self.hook.as_mut() {
            h(self.cam.pos, dt, &mut self.renderer);
        }

        if let Some(t) = work_start { self.perf_times[2] += t.elapsed().as_secs_f64(); }
        diagnostics.mark(1);
        let acquire_start = self.perf.then(Instant::now);
        let acquired = self.surface.get_current_texture();
        if let Some(t) = acquire_start { self.perf_times[1] += t.elapsed().as_secs_f64(); }
        diagnostics.mark(8);
        let frame = match acquired {
            wgpu::CurrentSurfaceTexture::Success(f) | wgpu::CurrentSurfaceTexture::Suboptimal(f) => f,
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                let t = self.perf.then(Instant::now);
                self.surface.configure(&self.renderer.device, &self.config);
                if let Some(t) = t { self.perf_times[3] += t.elapsed().as_secs_f64(); }
                self.finish_perf_frame(now);
                return quit;
            }
            _ => {
                self.finish_perf_frame(now);
                return quit; // timeout / occluded: skip this frame
            }
        };
        let view = frame.texture.create_view_tracked(&Default::default());
        let t0 = self.perf.then(Instant::now);
        let submission = self.renderer.render(&view, &self.targets, &self.cam);
        if let Some(t) = t0 { self.cpu += t.elapsed().as_secs_f32(); }
        diagnostics.mark(4);
        let capture = self.pending_screenshot.take().map(|path| (path, gui_out.clone()));
        if let Some(list) = gui_out {
            let t = self.perf.then(Instant::now);
            let gview = crate::GuiRenderer::view(&frame.texture);
            self.paint_gui(list, &gview);
            if let Some(t) = t { self.perf_times[2] += t.elapsed().as_secs_f64(); }
        }
        diagnostics.mark(5);
        if self.gui.as_ref().is_some_and(|g| g.host.mouse_lag_fix) {
            if let Some(previous) = self.previous_submission.take() {
                if let Err(error) = self.renderer.device.poll(wgpu::PollType::Wait { submission_index: Some(previous), timeout: None }) {
                    eprintln!("render: MouseLagFix GPU synchronization: {error}");
                }
            }
        } else {
            // Without MouseLagFix nothing else polls the device: every buffer/texture/bind group replaced this frame
            // (actor skins, lightning/combat-effect actors, sky/liquid geometry) sits as a pending GPU destruction
            // that wgpu only reclaims on a poll. A frame of uncapped per-frame churn left unpolled for a whole
            // session is the "massive memory leak" the user hit: a non-blocking maintain keeps it reclaimed live.
            let _ = self.renderer.device.poll(wgpu::PollType::Poll);
        }
        self.previous_submission = Some(submission);
        diagnostics.mark(6);
        let present_start = self.perf.then(Instant::now);
        self.window.pre_present_notify();
        frame.present();
        if let Some(t) = present_start { self.perf_times[4] += t.elapsed().as_secs_f64(); }
        diagnostics.mark(7);
        if let Some(g) = &self.gui { diagnostics.finish(g.host.slow_frame_log, "windowed", &g.host.effect_gpu_costs); }
        self.finish_perf_frame(now);
        // Capture/readback is explicit evidence work, not a slow gameplay frame.
        if let Some((path, list)) = capture { self.capture_screenshot(path, list); }

        self.stat_frames += 1;
        let el = self.stat_t.elapsed().as_secs_f32();
        if el >= 0.5 {
            let fps = self.stat_frames as f32 / el;
            let p = self.cam.pos;
            if self.fly() {
                self.window.set_title(&format!("aomac | {fps:.0} fps | {:.1} {:.1} {:.1} | speed {:.0}", p.x, p.y, p.z, self.speed));
            }
            if self.perf {
                let st = self.renderer.stats;
                let ms = self.perf_times.map(|t| 1000.0 * t / self.perf_frames as f64);
                eprintln!("{:.2} ms/frame ({fps:.0} fps), render() cpu {:.2} ms, {} instances, {} draws; frame cpu wall {:.2} ms, acquire {:.2} ms, GUI/frame work {:.2} ms, resize/reconfigure {:.2} ms, present {:.2} ms, worst gap {:.2} ms, {} attempts/{} presented", 1000.0 / fps, 1000.0 * self.cpu / self.stat_frames as f32, st.instances, st.draw_calls, ms[0], ms[1], ms[2], ms[3], ms[4], 1000.0 * self.perf_gap, self.perf_frames, self.stat_frames);
                self.cpu = 0.0;
                self.perf_times = [0.0; 5];
                self.perf_frames = 0;
                self.perf_gap = 0.0;
            }
            self.stat_t = Instant::now();
            self.stat_frames = 0;
        }
        quit
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, el: &ActiveEventLoop) {
        if self.state.is_none() {
            match State::new(el, &self.scene, self.frontend.take(), self.live.take(), self.hook.take()) {
                Ok(s) => self.state = Some(s),
                Err(e) => {
                    self.error = Some(e);
                    el.exit();
                }
            }
        }
    }

    fn window_event(&mut self, el: &ActiveEventLoop, _: WindowId, ev: WindowEvent) {
        let Some(s) = &mut self.state else { return };
        if let WindowEvent::Focused(focused) = &ev { s.focused = *focused; }
        if let WindowEvent::ModifiersChanged(m) = &ev { s.mods = m.state(); }
        // Raw reports are batched until redraw. Drain before a button changes the
        // frontend's look mode or releases capture (which clears the accumulator).
        if matches!(&ev, WindowEvent::MouseInput { .. }) && s.looking && !s.fly() {
            s.flush_mouse(s.last.elapsed().as_secs_f32());
        }
        if let Some(g) = &mut s.gui {
            let gi = gui_input(&ev, &mut g.cursor, &mut g.mods, g.scale, &mut g.scroll_accum);
            g.host.mods = g.mods;
            if let Some(ie) = gi {
                // a key that types a character also presses that key first: hotkeys (docs/gui.md 12.1) read the key, text fields the text.
                // The OS key repeat only repeats the text: hotkeys act on the press.
                let repeat = matches!(&ev, WindowEvent::KeyboardInput { event, .. } if event.repeat);
                let key = match (&ie, &ev) {
                    (ao_gui::InputEvent::Text(_), WindowEvent::KeyboardInput { event, .. }) if !repeat => match &event.logical_key {
                        winit::keyboard::Key::Character(c) => c.chars().next().map(|c| ao_gui::InputEvent::Key { key: ao_gui::Key::Letter(c.to_ascii_lowercase()), pressed: true, mods: g.mods }),
                        _ => None,
                    },
                    _ => None,
                };
                let ie = Some(ie).filter(|ie| !(repeat && matches!(ie, ao_gui::InputEvent::Key { key: ao_gui::Key::Letter(_), pressed: true, .. })));
                // the frontend may move the camera from an input handler (e.g. a click that switches screens)
                g.host.camera = s.cam;
                for ie in key.into_iter().chain(ie) {
                    g.frontend.input(ie, &mut g.host);
                }
                s.cam = g.host.camera;
            }
        }
        let fly = s.fly();
        if let (WindowEvent::KeyboardInput { event, .. }, Some(g), false) = (&ev, &mut s.gui, fly) {
            if let PhysicalKey::Code(code) = event.physical_key {
                g.host.camera = s.cam;
                g.frontend.game_input(GameInput::Key { code, pressed: event.state == ElementState::Pressed, repeat: event.repeat }, &mut g.host);
                s.cam = g.host.camera;
            }
        }
        if let Some(want) = s.gui.as_ref().filter(|_| !fly).map(|g| g.host.look) {
            if want != s.looking {
                s.set_look(want);
            }
        }
        s.sync_cursor();
        match ev {
            // Refresh pacing follows the current screen, not a "moving window" throttle.
            WindowEvent::Moved(_) => {
                s.display_period = display_period(s.window.current_monitor().and_then(|monitor| monitor.refresh_rate_millihertz()));
                s.save_window();
            }
            WindowEvent::CloseRequested => {
                s.save_window();
                el.exit();
            }
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                s.display_period = display_period(s.window.current_monitor().and_then(|monitor| monitor.refresh_rate_millihertz()));
                if let Some(g) = &mut s.gui {
                    // Preserve the cursor's physical position when logical GUI pixels change.
                    let scale = gui_scale(scale_factor, Some(g.requested_scale), (s.config.width, s.config.height));
                    let ratio = g.scale as f32 / scale as f32;
                    g.cursor.0 *= ratio;
                    g.cursor.1 *= ratio;
                    g.scale = scale;
                }
            }
            // Audit item 6: remembered across restarts, toggled instantly, optionally hiding the menu bar/dock.
            WindowEvent::KeyboardInput { event, .. }
                if event.state == ElementState::Pressed && !event.repeat && s.mods.control_key() && s.mods.super_key()
                    && matches!(event.physical_key, PhysicalKey::Code(KeyCode::KeyF)) =>
            {
                if s.window.fullscreen().is_some() {
                    s.window.set_fullscreen(None);
                    s.window.set_borderless_game(false);
                } else {
                    let borderless_game = s.gui.as_ref().is_some_and(|g| g.host.borderless_game);
                    s.window.set_borderless_game(borderless_game);
                    s.window.set_fullscreen(Some(Fullscreen::Borderless(s.window.current_monitor())));
                }
                s.save_window();
            }
            // Audit item 8: trackpad pinch gesture zooms the camera, opt-in (`Host::pinch_zoom`); same gating as the
            // other raw `GameInput` (in-world, not blocked by text input).
            WindowEvent::PinchGesture { delta, .. } if !fly && s.gui.as_ref().is_some_and(|g| g.host.pinch_zoom) => {
                if let Some(g) = &mut s.gui {
                    g.host.camera = s.cam;
                    g.frontend.game_input(GameInput::Pinch(delta as f32), &mut g.host);
                    s.cam = g.host.camera;
                }
            }
            WindowEvent::KeyboardInput { event, .. } if fly || s.gui.is_none() => {
                if let PhysicalKey::Code(code) = event.physical_key {
                    if event.state == ElementState::Pressed {
                        if code == KeyCode::Escape && s.gui.is_none() {
                            el.exit();
                        }
                        s.keys.insert(code);
                    } else {
                        s.keys.remove(&code);
                    }
                }
            }
            WindowEvent::Focused(false) => {
                s.keys.clear();
                s.set_look(false);
                if let Some(g) = &mut s.gui {
                    g.host.look = false;
                    g.mods = ao_gui::Modifiers::default();
                    g.host.mods = g.mods;
                    g.host.camera = s.cam;
                    g.frontend.game_input(GameInput::FocusLost, &mut g.host);
                    s.cam = g.host.camera;
                }
            }
            WindowEvent::MouseInput { button: MouseButton::Right, state, .. } if fly => s.set_look(state == ElementState::Pressed),
            WindowEvent::MouseWheel { delta, .. } if fly => {
                let y = match delta {
                    MouseScrollDelta::LineDelta(_, y) => y,
                    MouseScrollDelta::PixelDelta(p) => p.y as f32 / 40.0,
                };
                s.speed = (s.speed * 1.15f32.powf(y)).clamp(0.1, 100_000.0);
            }
            WindowEvent::RedrawRequested => {
                let start = Instant::now();
                if s.frame() {
                    el.exit();
                    return;
                }
                if let Some(period) = s.display_period {
                    s.frame_deadline = frame_deadline(start, Instant::now(), period);
                }
            }
            _ => {}
        }
    }

    fn device_event(&mut self, _: &ActiveEventLoop, _: DeviceId, ev: DeviceEvent) {
        let (Some(s), DeviceEvent::MouseMotion { delta: (dx, dy) }) = (&mut self.state, ev) else { return };
        if !s.looking {
            return;
        }
        if !s.fly() {
            s.raw_mouse.push_winit(dx as f32, dy as f32);
        } else {
            s.cam.yaw += dx as f32 * 0.0025;
            s.cam.pitch = (s.cam.pitch - dy as f32 * 0.0025).clamp(-1.55, 1.55);
        }
    }

    fn about_to_wait(&mut self, el: &ActiveEventLoop) {
        if let Some(s) = &self.state {
            // Yield the AppKit runloop BEFORE the next drawable acquisition.
            // Keep actual display synchronization enabled; profiling/no-vsync stays uncapped.
            if cfg!(target_os = "macos") && wait_for_frame(s.config.present_mode, s.display_period, s.frame_deadline, Instant::now()) {
                el.set_control_flow(ControlFlow::WaitUntil(s.frame_deadline));
            } else {
                el.set_control_flow(ControlFlow::Poll);
                s.window.request_redraw();
            }
        }
    }

    /// winit calls this when the event loop is about to stop, including on macOS when the system "Quit" menu item
    /// (Cmd+Q, part of winit's default app menu) terminates the app: that path never sends `WindowEvent::CloseRequested`,
    /// so without this handler `App` (and the `State`/`Gui`/boxed `Frontend` it owns) was simply leaked by the process
    /// exit instead of dropped, skipping every `Drop` impl a frontend relies on for shutdown (aomac's `Play::drop`
    /// saves the character-viewer cache). Window position/size are saved the same way `CloseRequested` saves them.
    fn exiting(&mut self, _el: &ActiveEventLoop) {
        if let Some(s) = self.state.take() {
            s.save_window();
        } // `s` drops here: State -> Gui -> Box<dyn Frontend>, running the frontend's own Drop impl.
    }
}

/// A [`Frontend`] driven without a window: the same per-frame sequence as the windowed app (`frame`, scene upload, lens,
/// 3D render, GUI draw) into an offscreen texture. Used by the headless screenshot tests.
pub struct Offscreen {
    r: Renderer,
    targets: Targets,
    gr: crate::GuiRenderer,
    pub host: Host,
    size: (u32, u32),
    live: Option<LiveRun>,
    perf: Option<OffscreenPerf>,
    fixed_camera: Option<Camera>,
    diagnostic_wall_ms: [f64; 9],
}

// Bounded samples and a persistent attachment: no screenshot/readback or metrics allocation per frame.
struct OffscreenPerf {
    _texture: wgpu::Texture,
    view: wgpu::TextureView,
    gui_view: wgpu::TextureView,
    samples: [[f64; 7]; 600],
    cpu_samples: [[f64; 7]; 600],
    count: usize,
    msaa: Option<u32>,
}

impl OffscreenPerf {
    fn new(r: &Renderer, size: (u32, u32)) -> Self {
        let msaa = std::env::var("AOMAC_PERF_MSAA").ok().map(|value| {
            let samples = value.parse::<u32>().expect("AOMAC_PERF_MSAA must be 1, 2, 4, or 8");
            assert!(matches!(samples, 1 | 2 | 4 | 8), "AOMAC_PERF_MSAA must be 1, 2, 4, or 8");
            samples
        });
        Self::explicit(r, size, msaa)
    }

    fn explicit(r: &Renderer, size: (u32, u32), msaa: Option<u32>) -> Self {
        let texture = r.device.create_texture_tracked(&wgpu::TextureDescriptor {
            label: Some("offscreen performance target"),
            size: wgpu::Extent3d { width: size.0, height: size.1, depth_or_array_layers: 1 },
            mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2,
            format: r.format, usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = texture.create_view_tracked(&Default::default());
        let gui_view = crate::GuiRenderer::view(&texture);
        Self { _texture: texture, view, gui_view, samples: [[0.0; 7]; 600], cpu_samples: [[0.0; 7]; 600], count: 0, msaa }
    }
}

fn perf_percentiles(samples: &mut [f64]) -> (f64, f64) {
    samples.sort_unstable_by(f64::total_cmp);
    let at = |percent: usize| samples[(samples.len() * percent).div_ceil(100).saturating_sub(1)];
    (at(50), at(95))
}

/// Calling-thread CPU time: excludes descheduling, GPU time, and background asset/driver threads.
fn perf_thread_cpu() -> Duration {
    #[repr(C)]
    struct Timespec { seconds: std::os::raw::c_long, nanos: std::os::raw::c_long }
    unsafe extern "C" {
        fn clock_gettime(clock: std::os::raw::c_int, time: *mut Timespec) -> std::os::raw::c_int;
    }
    let mut time = Timespec { seconds: 0, nanos: 0 };
    let clock = if cfg!(target_os = "macos") { 16 } else { 3 };
    // SAFETY: valid native timespec storage; Darwin/Linux CLOCK_THREAD_CPUTIME_ID.
    assert_eq!(unsafe { clock_gettime(clock, &mut time) }, 0);
    Duration::new(time.seconds as u64, time.nanos as u32)
}

fn native_gpu_allocated_bytes(r: &Renderer) -> Option<usize> {
    #[cfg(target_os = "macos")]
    {
        // SAFETY: the HAL guard keeps the Metal queue alive; its device implements these
        // read-only MTLDevice selectors. No raw object escapes the guard's lifetime.
        unsafe {
            let queue = r.queue.as_hal::<wgpu::hal::api::Metal>()?;
            let device: *mut objc2::runtime::AnyObject = objc2::msg_send![queue.as_raw(), device];
            if device.is_null() { return None; }
            Some(objc2::msg_send![device, currentAllocatedSize])
        }
    }
    #[cfg(not(target_os = "macos"))]
    { let _ = r; None }
}

impl Offscreen {
    /// Effect-owned buffer bytes, latest cell preparation time, and GPU-expanded cells.
    pub fn evidence_grass_metrics(&self) -> (usize, u64, usize) {
        (self.r.enhanced_grass_resource_bytes(), self.r.enhanced_grass_prepare_cpu_ns(), self.r.enhanced_grass_cell_count())
    }
    /// Warms only procedural tiles, preserving native render-count animation clocks.
    pub fn warm_grass_tiles(&mut self) { self.r.warm_grass_tiles(&self.host.camera); }
    /// Offline-only per-tile material, exposure and projected-root diagnostics.
    pub fn evidence_grass_diagnostics(&self) -> String { self.r.grass_diagnostics(&self.host.camera) }
    pub fn new(fe: &dyn Frontend, size: (u32, u32)) -> Result<Self> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let r = Renderer::new(&instance, None)?;
        let (targets, gr) = (Targets::new(&r, size.0, size.1), crate::GuiRenderer::new(&r, fe.gui()));
        let perf = std::env::var_os("AOMAC_PERF").map(|_| OffscreenPerf::new(&r, size));
        Ok(Self { r, targets, gr, host: Host::headless(), size, live: None, perf, fixed_camera: None, diagnostic_wall_ms: [0.0; 9] })
    }

    /// Headless capture without ambient performance environment configuration.
    pub fn evidence(fe: &dyn Frontend, size: (u32, u32)) -> Result<Self> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let r = Renderer::new(&instance, None)?;
        let (targets, gr) = (Targets::new(&r, size.0, size.1), crate::GuiRenderer::new(&r, fe.gui()));
        Ok(Self { r, targets, gr, host: Host::headless(), size, live: None, perf: None, fixed_camera: None, diagnostic_wall_ms: [0.0; 9] })
    }

    /// Persistent frozen-snapshot timing, single-sample by default; explicit host quality still wins.
    pub fn evidence_timing(fe: &dyn Frontend, size: (u32, u32)) -> Result<Self> {
        let mut off = Self::evidence(fe, size)?;
        off.host.msaa_samples = Some(1);
        off.prepare_frozen_rendering();
        Ok(off)
    }

    /// Installs a persistent target after the frozen capture, without advancing renderer counters.
    pub fn prepare_frozen_rendering(&mut self) {
        if self.perf.is_none() { self.perf = Some(OffscreenPerf::explicit(&self.r, self.size, None)); }
    }

    /// Renders existing scene/actors/camera without frontend updates or capture/readback allocation.
    /// Requires `evidence_timing`; read enabled GPU timers before calling this again.
    pub fn render_frozen(&mut self) -> Result<()> {
        let perf = self.perf.as_ref().ok_or_else(|| anyhow::anyhow!("render_frozen requires a persistent evidence target"))?;
        self.r.render(&perf.view, &self.targets, &self.host.camera);
        Ok(())
    }

    /// Isolates actual GUI encoding/submission CPU wall time from preceding world work.
    /// Evidence-only blocking boundary; the live profiler supplies rolling GUI GPU costs.
    /// Does not update the frontend/world or allocate a capture target/readback.
    pub fn render_gui_only_for_evidence(&mut self, fe: &dyn Frontend, list: &ao_gui::DrawList) -> Result<f64> {
        let _frame = crate::pipeline_creation::FrameGuard::enter();
        let perf = self.perf.as_ref().ok_or_else(|| anyhow::anyhow!("render_gui_only_for_evidence requires a persistent evidence target"))?;
        self.r.device.poll(wgpu::PollType::wait_indefinitely())?;
        self.host.refresh_gpu_diagnostics(&mut self.r);
        let start = Instant::now();
        self.gr.draw(&self.r, &perf.gui_view, self.size, 1, fe.gui(), list);
        let cpu_ms = start.elapsed().as_secs_f64() * 1000.0;
        self.r.device.poll(wgpu::PollType::wait_indefinitely())?;
        self.host.refresh_gpu_diagnostics(&mut self.r);
        Ok(cpu_ms)
    }

    /// Tests weather-off composition on the last native image, without rerasterizing
    /// native geometry or advancing its render-count animation. No timing sample.
    pub fn weather_off_reference_rgba(&mut self, input: crate::WeatherInput) -> Result<Vec<u8>> {
        anyhow::ensure!(!self.r.enhanced_settings.any_enabled(), "weather-off reference requires faithful settings");
        let original = std::mem::replace(&mut self.r.weather_input, input);
        let mut encoder = self.r.device.create_command_encoder(&Default::default());
        // The off guard must return before projection, shelter or input is used.
        let env = self.r.env;
        self.r.encode_weather(&mut encoder, glam::Mat4::IDENTITY, self.host.camera.pos, env);
        self.r.weather_input = original;
        anyhow::ensure!(self.r.rain_wet.is_none() && self.r.weather_particles.is_none(), "weather-off composition retained optional weather resources");
        self.r.queue.submit([encoder.finish()]);
        self.frozen_rgba()
    }

    /// Reads the last persistent rendered frame without rerendering or changing motion history.
    pub fn frozen_rgba(&self) -> Result<Vec<u8>> {
        let perf = self.perf.as_ref().ok_or_else(|| anyhow::anyhow!("frozen_rgba requires a persistent evidence target"))?;
        crate::texture_to_rgba(&self.r, &perf._texture, self.size.0, self.size.1)
    }

    /// Timing-only simulation step; leaves the authored scene clock, frontend and actor poses frozen.
    pub fn advance_effect_time(&mut self, dt: f32) {
        self.r.time += dt;
        self.r.advance_grass_wind(dt);
    }

    pub fn set_gpu_timing(&mut self, enabled: bool) -> Result<()> { self.r.set_gpu_timing(enabled) }
    pub fn set_cpu_timing(&mut self, enabled: bool) { self.r.set_cpu_timing(enabled); }
    pub fn frame_cpu_timings(&self) -> Option<crate::FrameCpuTimings> { self.r.frame_cpu_timings() }
    pub fn read_frame_gpu_ms(&mut self) -> Result<Option<f64>> { self.r.read_frame_gpu_ms() }
    pub fn enhanced_target_bytes(&self) -> u64 { self.r.enhanced_target_bytes() }
    pub fn msaa_samples(&self) -> u32 { self.r.msaa_samples() }
    pub fn enhanced_settings(&self) -> crate::EnhancedSettings { self.r.enhanced_settings() }
    /// Last actual frontend/world/GUI frame CPU wall milliseconds; excludes screenshot/readback.
    /// Order: total, update, frontend, upload, render, gui, gpu_wait, present, acquire.
    /// Enabled by `evidence_timing` (persistent attachment) or opt-in slow-frame logging.
    pub fn diagnostic_frame_wall_ms(&self) -> [f64; 9] { self.diagnostic_wall_ms }
    /// Nonblocking collection for harnesses after a completed actual Play + UI frame.
    pub fn poll_effect_gpu_costs(&mut self) { self.host.refresh_gpu_diagnostics(&mut self.r); }
    pub fn effect_gpu_costs(&self) -> &[crate::EffectGpuCost] { self.r.effect_gpu_costs() }
    /// Separates overlay-visible whole-frame GPU measurements from pass profiling.
    /// Disable before enabling the exclusive whole-frame timer; cached overlay samples remain visible.
    pub fn set_effect_gpu_profiling_for_evidence(&mut self, enabled: bool) -> Result<()> {
        let profiling = enabled && (self.host.effect_gpu_overlay || self.host.slow_frame_log);
        self.r.set_effect_gpu_profiling(profiling)?;
        self.host.effect_gpu_profiling_suppressed = !enabled;
        Ok(())
    }
    pub fn world_pipeline_compiles(&self) -> u64 { self.r.world_pipeline_compiles() }
    pub fn world_frame_thread_compiles(&self) -> u64 { self.r.world_frame_thread_compiles() }
    pub fn frame_thread_compiles(&self) -> u64 { self.r.frame_thread_compiles() }
    pub fn frame_pipeline_waits(&self) -> u64 { self.r.frame_pipeline_waits() }
    /// Readiness of both the world/effect pipelines and the GUI pipeline used by this frontend.
    pub fn pipeline_warmup_complete(&self) -> bool { self.r.pipeline_warmup_complete() && self.gr.is_ready() }
    /// Explicit loading boundary; ordinary frames wait only for the pipelines they actually use.
    pub fn wait_for_pipeline_warmup(&self) {
        self.r.wait_for_pipeline_warmup();
        self.gr.wait();
    }
    pub fn world_variant_cache_len(&self) -> usize { self.r.world_variant_cache_len() }
    pub fn emissive_seed_rgba(&self) -> Result<Option<Vec<u8>>> { self.r.emissive_seed_rgba() }
    /// Explicit probe confidence and same-stage observed/analytic reflection readbacks.
    #[allow(clippy::type_complexity)]
    pub fn material_probe_evidence(&self) -> Result<Option<(Vec<u8>, Vec<u8>, Vec<u8>)>> {
        self.r.material_probe_evidence()
    }

    /// Explicit side-map root for offline evidence; set before the first scene upload.
    pub fn set_materials_root(&mut self, root: Option<std::path::PathBuf>) {
        self.r.set_materials_root(root);
    }

    /// Changes rendering resources while retaining the frozen actors and scene.
    pub fn set_enhanced_settings(&mut self, settings: crate::EnhancedSettings) {
        self.r.set_enhanced_settings(settings);
        if !self.targets.matches_renderer(&self.r) {
            self.targets = Targets::new(&self.r, self.size.0, self.size.1);
        }
    }

    /// Changes the render surface size without replacing the live frontend or its GUI state.
    pub fn resize(&mut self, size: (u32, u32)) {
        self.size = (size.0.max(1), size.1.max(1));
        self.targets = Targets::new(&self.r, self.size.0, self.size.1);
        if self.perf.is_some() { self.perf = Some(OffscreenPerf::new(&self.r, self.size)); }
    }

    /// Overrides the rendered/picking camera after frontend updates; `None` restores normal following.
    pub fn fixed_camera(&mut self, camera: Option<Camera>) { self.fixed_camera = camera; }

    pub fn set_shadow_gpu_timing(&mut self, enabled: bool) -> Result<()> { self.r.set_shadow_gpu_timing(enabled) }
    pub fn read_shadow_gpu_ms(&mut self) -> Result<Option<f64>> { self.r.read_shadow_gpu_ms() }
    pub fn shadow_resident_bytes(&self) -> u64 { self.r.shadow_resident_bytes() }
    pub fn shadow_static_command_counts(&self) -> (usize, usize) { self.r.shadow_static_command_counts() }
    /// Diagnostic readback only: current GPU owner admission and selected point-shadow cube.
    pub fn point_shadow_owner_is_selected(&mut self, owner: u32) -> Result<bool> {
        self.r.point_shadow_owner_is_selected(owner)
    }
    /// Current point-cache CPU dispatch inputs; no allocation or GPU readback.
    pub fn point_shadow_dispatch_work(&self) -> Option<(usize, usize, u32, u32, u64)> {
        self.r.point_shadow_dispatch_work()
    }
    pub fn gpu_allocated_bytes(&self) -> Option<usize> { native_gpu_allocated_bytes(&self.r) }

    /// Starts a fresh measured interval without rebuilding the renderer or its resources.
    pub fn perf_reset(&mut self) {
        if let Some(perf) = &mut self.perf { perf.count = 0; }
    }

    /// Reports thread CPU stages and completed-frame wall latency, never PNG encoding.
    pub fn perf_report(&mut self, label: &str) {
        let Some(perf) = &mut self.perf else { return };
        let mut column = [0.0; 600];
        eprintln!("AOMAC_PERF {label} frames={} size={}x{} msaa_requested={:?} msaa_actual={}", perf.count, self.size.0, self.size.1, perf.msaa, self.r.msaa_samples());
        if perf.count > 0 {
            for (kind, samples) in [("wall", &perf.samples), ("thread_cpu", &perf.cpu_samples)] {
                for (stage, name) in ["total", "update", "frontend", "upload", "render", "gui", "gpu_wait"].into_iter().enumerate() {
                    for (out, sample) in column.iter_mut().zip(&samples[..perf.count]) { *out = sample[stage] * 1000.0; }
                    let values = &mut column[..perf.count];
                    let mean = values.iter().sum::<f64>() / values.len() as f64;
                    let (p50, p95) = perf_percentiles(values);
                    eprintln!("AOMAC_PERF {label} {kind}_{name}_ms mean={mean:.4} p50={p50:.4} p95={p95:.4}");
                }
            }
        }
        let rss = std::process::Command::new("ps").args(["-o", "rss=", "-p", &std::process::id().to_string()]).output();
        let rss_kib = rss.ok().filter(|out| out.status.success()).and_then(|out| String::from_utf8(out.stdout).ok()).and_then(|s| s.trim().parse::<u64>().ok());
        eprintln!("AOMAC_PERF {label} rss_kib={rss_kib:?} instances={} draws={} native_gpu_allocated_bytes={:?} (Metal device allocation, unified memory; not dedicated VRAM)", self.r.stats.instances, self.r.stats.draw_calls, native_gpu_allocated_bytes(&self.r));
        let (actor_textures, actor_texture_bytes) = self.r.actor_texture_resources();
        eprintln!("AOMAC_PERF {label} retained_actor_textures={actor_textures} retained_actor_texture_bytes_estimate={actor_texture_bytes} actor_texture_uploads_total={}", self.r.actor_texture_uploads());
        let (actor_models, actors, pooled_vbs, drawn_visuals) = self.r.actor_cache_sizes();
        eprintln!("AOMAC_PERF {label} actor_models={actor_models} actors={actors} pooled_vbs={pooled_vbs} drawn_visuals={drawn_visuals} vb_created={} model_rebuilds={} mesh_buffer_grows={}",
            self.r.actor_vb_creations(), self.r.actor_model_rebuilds(), self.r.actor_mesh_buffer_grows());
        let (creates, bytes) = crate::gpu_stats::perf_snapshot();
        eprintln!("AOMAC_PERF {label} tracked_creates_total={creates} tracked_created_bytes_estimate_total={bytes} (cumulative, not live VRAM)");
        perf.count = 0;
    }

    /// One `Frontend::frame`, applying what it asked of the host.
    pub fn frame(&mut self, fe: &mut dyn Frontend, dt: f32) -> ao_gui::DrawList {
        let _frame = crate::pipeline_creation::FrameGuard::enter();
        if self.perf.as_ref().is_some_and(|p| p.count == p.samples.len()) { self.perf_report("interval"); }
        let start = self.perf.as_ref().map(|_| Instant::now());
        let cpu_start = start.map(|_| perf_thread_cpu());
        let mut diagnostics = FrameDiagnostics::new(self.host.slow_frame_log || self.perf.is_some());
        self.r.time += dt;
        self.r.advance_grass_wind(dt);
        if let Some(sun) = LiveRun::advance(&mut self.live, &mut self.r, dt) { self.host.effect_sun_direction = Some(sun); }
        self.r.visible_actor_ids(&mut self.host.previous_visible_actors);
        self.host.refresh_gpu_diagnostics(&mut self.r);
        let update_end = start.map(|_| Instant::now());
        let cpu_update = start.map(|_| perf_thread_cpu());
        diagnostics.mark(1);
        for event in self.r.take_mover_sounds() { fe.mover_sound(event, &self.host); }
        let list = fe.frame(dt, self.size, &mut self.host);
        let frontend_end = start.map(|_| Instant::now());
        let cpu_frontend = start.map(|_| perf_thread_cpu());
        diagnostics.mark(2);
        if let Some((quality, compressed, better_mips)) = self.host.texture_policy.take() {
            self.r.set_texture_policy(quality, compressed, better_mips);
        }
        if let Some(level) = self.host.anisotropy.take() {
            self.r.set_anisotropy(level);
        }
        if let Some(samples) = self.perf.as_ref().and_then(|perf| perf.msaa) {
            self.host.msaa_samples = Some(samples);
        }
        self.host.apply_world_options(&mut self.r);
        if let Some(scene) = self.host.scene.take() {
            self.r.upload(&scene);
        }
        if let Some(scene) = self.host.repose.take() {
            self.r.repose(&scene);
        }
        LiveRun::apply_host(&mut self.live, &mut self.host);
        if let Some(lens) = self.host.lens.take() {
            self.r.set_lens(lens);
        }
        if let Some(s) = self.host.fog_density_scale.take() {
            self.r.set_fog_density_scale(s);
        }
        self.r.set_spaceships_shown(self.host.spaceships_shown);
        self.r.set_realistic_water(self.host.realistic_water);
        if let Some((realistic,simple))=self.host.cloud_preferences.take() {self.r.set_cloud_preferences(realistic,simple,false);}
        self.r.settle_enhanced_world();
        if !self.targets.matches_renderer(&self.r) {
            self.targets = Targets::new(&self.r, self.size.0, self.size.1);
        }
        if let Some((quality, radius)) = self.host.ground_detail.take() {
            self.r.set_ground_detail(quality, radius);
        }
        self.r.set_ground_rendering(self.host.ground_rendering);
        if let Some(textures) = self.host.ground_textures.take() { self.r.refresh_ground_textures(&textures); }
        self.host.apply_actors(&mut self.r);
        if let Some(camera) = self.fixed_camera { self.host.camera = camera; }
        diagnostics.mark(3);
        if let Some(perf) = self.perf.as_mut() {
            let upload_end = Instant::now();
            let cpu_upload = perf_thread_cpu();
            self.r.render(&perf.view, &self.targets, &self.host.camera);
            diagnostics.mark(4);
            let render_end = Instant::now();
            let cpu_render = perf_thread_cpu();
            self.gr.draw(&self.r, &perf.gui_view, self.size, 1, fe.gui(), &list);
            diagnostics.mark(5);
            let gui_end = Instant::now();
            let cpu_gui = perf_thread_cpu();
            // This submission follows both 3D and GUI work. Bound the queue to one frame, like a
            // swapchain, rather than accumulating GPU work and deferred destruction in a tight loop.
            let submission = self.r.queue.submit([]);
            self.r.device.poll(wgpu::PollType::Wait { submission_index: Some(submission), timeout: None }).expect("offscreen performance GPU wait");
            diagnostics.mark(6);
            let end = Instant::now();
            let cpu_end = perf_thread_cpu();
            let (start, update, frontend) = (start.unwrap(), update_end.unwrap(), frontend_end.unwrap());
            perf.samples[perf.count] = [
                (end - start).as_secs_f64(), (update - start).as_secs_f64(),
                (frontend - update).as_secs_f64(), (upload_end - frontend).as_secs_f64(),
                (render_end - upload_end).as_secs_f64(), (gui_end - render_end).as_secs_f64(),
                (end - gui_end).as_secs_f64(),
            ];
            let (cpu_start, cpu_update, cpu_frontend) = (cpu_start.unwrap(), cpu_update.unwrap(), cpu_frontend.unwrap());
            perf.cpu_samples[perf.count] = [
                (cpu_end - cpu_start).as_secs_f64(), (cpu_update - cpu_start).as_secs_f64(),
                (cpu_frontend - cpu_update).as_secs_f64(), (cpu_upload - cpu_frontend).as_secs_f64(),
                (cpu_render - cpu_upload).as_secs_f64(), (cpu_gui - cpu_render).as_secs_f64(),
                (cpu_end - cpu_gui).as_secs_f64(),
            ];
            perf.count += 1;
        } else {
            // Flush staged writes even when the normal logic-only harness does not render.
            self.r.queue.submit([]);
            let _ = self.r.device.poll(wgpu::PollType::Poll);
        }
        self.diagnostic_wall_ms = diagnostics.finish(self.host.slow_frame_log, "offscreen", &self.host.effect_gpu_costs);
        if let Some(path) = self.host.screenshot.take() {
            if let Err(e) = self.png(fe, &list, &path) { eprintln!("screenshot: {}: {e}", path.display()); }
        }
        list
    }
    /// Reads actual last-render sky fragments without advancing the simulation.
    /// `projection_far` changes only diagnostic clip Z, never uploaded sky geometry.
    pub fn sky_pixel_probe(&self, layer: usize, pixels: &[(u32, u32)], scene: &Scene, projection_far: Option<f32>) -> Result<String> {
        self.r.sky_pixel_probe(layer, pixels, self.size, &scene.textures, projection_far)
    }


    /// Captures a single frame as tightly packed, row-major RGBA8.
    pub fn rgba(&mut self, fe: &dyn Frontend, list: &ao_gui::DrawList) -> Result<Vec<u8>> {
        let _frame = crate::pipeline_creation::FrameGuard::enter();
        let tex = self.r.device.create_texture_tracked(&wgpu::TextureDescriptor {
            label: None,
            size: wgpu::Extent3d { width: self.size.0, height: self.size.1, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: self.r.format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = tex.create_view_tracked(&Default::default());
        self.r.render(&view, &self.targets, &self.host.camera);
        self.gr.draw(&self.r, &crate::GuiRenderer::view(&tex), self.size, 1, fe.gui(), list);
        crate::texture_to_rgba(&self.r, &tex, self.size.0, self.size.1)
    }

    /// Writes the same single-frame capture as PNG.
    pub fn png(&mut self, fe: &dyn Frontend, list: &ao_gui::DrawList, path: &std::path::Path) -> Result<()> {
        let pixels = self.rgba(fe, list)?;
        let mut encoder = png::Encoder::new(std::io::BufWriter::new(std::fs::File::create(path)?), self.size.0, self.size.1);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.write_header()?.write_image_data(&pixels)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runtime_suspend_preserves_requests_and_restores_via_worker() {
        let mut host = Host::headless();
        assert!(!host.enhanced_suspended && !host.effect_gpu_overlay && !host.slow_frame_log);
        assert!(host.effect_gpu_costs.is_empty() && host.effect_gpu_error.is_none());
        assert!(host.enhanced().is_none());
        let settings = crate::EnhancedSettings { hdr: true, ..Default::default() };
        host.enhanced_settings = Some(settings);
        assert_eq!(host.take_enhanced_requests(), (Some(settings), None));
        host.enhanced_suspended = true;
        host.enhanced_settings = Some(settings);
        assert_eq!(host.enhanced(), Some(crate::EnhancedSettings::default()));
        assert_eq!(host.take_enhanced_requests(), (None, Some(crate::EnhancedSettings::default())));
        let settings = crate::EnhancedSettings { ambient_occlusion: true, ..settings };
        host.enhanced_switch = Some(settings);
        assert_eq!(host.take_enhanced_requests(), (None, Some(crate::EnhancedSettings::default())));
        host.enhanced_suspended = false;
        assert_eq!(host.enhanced(), Some(settings));
        assert_eq!(host.take_enhanced_requests(), (None, Some(settings)));
        assert_eq!(host.take_enhanced_requests(), (None, None));
    }

    #[test]
    fn evidence_timing_honors_explicit_msaa_across_frames() {
        struct Idle(ao_gui::Gui);
        impl Frontend for Idle {
            fn gui(&self) -> &ao_gui::Gui { &self.0 }
            fn input(&mut self, _: ao_gui::InputEvent, _: &mut Host) {}
            fn frame(&mut self, dt: f32, _: (u32, u32), _: &mut Host) -> ao_gui::DrawList { self.0.frame(dt) }
        }
        let mut fe = Idle(ao_gui::Gui::new(&ao_gui::client_dir(), None).unwrap());
        let mut off = Offscreen::evidence_timing(&fe, (64, 64)).unwrap();
        off.host.msaa_samples = Some(4);
        // Evidence requests are synchronous; samples-only requests exercise the live worker path.
        off.host.enhanced_settings = Some(crate::EnhancedSettings::default());
        for _ in 0..2 {
            off.frame(&mut fe, 0.0);
            assert_eq!(off.r.msaa_samples(), 4, "the timing target must not overwrite explicit quality");
            assert_eq!(off.targets.sample_count, 4);
        }
    }

    #[test]
    fn offscreen_perf_percentiles_use_nearest_rank() {
        let cpu_start = perf_thread_cpu();
        for n in 0..10_000 { std::hint::black_box(n); }
        assert!(perf_thread_cpu() > cpu_start, "calling-thread CPU clock must advance during work");
        let mut samples = [5.0, 1.0, 4.0, 2.0, 3.0];
        assert_eq!(perf_percentiles(&mut samples), (3.0, 5.0));
        assert_eq!(perf_percentiles(&mut [7.0]), (7.0, 7.0));
    }

    #[test]
    fn live_sky_renderer_frame_positions_survive_delayed_worker_responses() {
        fn sample(time: f32) -> Scene {
            let mesh = ao_scene::Mesh {
                vertices: [[time - 1.0, 0.0, -100.0], [time + 1.0, 0.0, -100.0], [time, 1.0, -100.0]]
                    .map(|pos| ao_scene::Vertex { pos, color: [1.0; 4], ..Default::default() }).into(),
                submeshes: vec![ao_scene::Submesh::new(vec![0, 1, 2], None)],
            };
            Scene {
                day_time: time, meshes: vec![mesh],
                sky: vec![ao_scene::Instance { mesh: 0, transform: ao_scene::IDENTITY }],
                sky_pose_objects: vec![(0, 0)], ..Scene::default()
            }
        }
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let mut renderer = Renderer::new(&instance, None).expect("sky integration regression requires an offscreen GPU adapter");
        renderer.upload(&sample(100.0));
        let (started, requests) = std::sync::mpsc::channel();
        let (release, blocked) = std::sync::mpsc::channel::<()>();
        let mut live = Some(LiveRun::new(LiveSky {
            start: 100.0, scale: 1.0,
            source: Box::new(move |time| {
                let _ = started.send(time);
                let _ = blocked.recv();
                sample(time)
            }),
            pose: Some(Box::new(|time, scene| {
                scene.sky[0].transform = ao_scene::IDENTITY;
                scene.sky[0].transform[3][0] = time - scene.day_time;
            })),
        }));
        let mut elapsed = 0.0f64;
        let mut previous = 100.0;
        println!("renderer_celestial_csv,frame,dt,modeled_old_hold_x,observed_callback_x,baked_time");
        for frame in 0..120 {
            let dt = if frame % 2 == 0 { 1.0 / 60.0 } else { 1.0 / 30.0 };
            elapsed += f64::from(dt);
            LiveRun::advance(&mut live, &mut renderer, dt);
            if matches!(frame, 29 | 59 | 89) {
                let requested = requests.recv_timeout(std::time::Duration::from_secs(5)).expect("resource request");
                release.send(()).unwrap();
                let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
                while renderer.sky.pose.day_time != requested {
                    LiveRun::advance(&mut live, &mut renderer, 0.0);
                    assert!(std::time::Instant::now() < deadline, "resource response must be applied");
                    std::thread::sleep(std::time::Duration::from_millis(1));
                }
            }
            let baked = renderer.sky.pose.day_time;
            let observed = renderer.sky.xf[0].transform_point3(Vec3::new(baked, 0.0, -100.0)).x;
            assert!((f64::from(observed) - 100.0 - elapsed).abs() < 0.00002);
            assert!((observed - previous - dt).abs() < 0.00003, "frame {frame}: actual renderer matrix must advance proportionally to dt");
            println!("renderer_celestial_csv,{frame},{dt:.9},{:.9},{observed:.9},{baked:.9}",
                100.0 + (elapsed / 0.5).floor() * 0.5);
            previous = observed;
        }
        drop(release);
    }

    #[test]
    fn live_sky_poses_advance_every_frame_without_waiting_for_resource_samples() {
        let (release, blocked) = std::sync::mpsc::channel::<()>();
        let mut run = LiveRun::new(LiveSky {
            start: 4478.0,
            scale: 1.0,
            source: Box::new(move |_| { let _ = blocked.recv(); Scene::default() }),
            pose: Some(Box::new(|time, scene| scene.spawn = Some([time, 0.0, 0.0]))),
        });
        let mut pose = Scene::default();
        let mut elapsed = 0.0f64;
        let mut previous = 4478.0;
        for frame in 0..120 {
            let dt = if frame % 2 == 0 { 1.0 / 60.0 } else { 1.0 / 30.0 };
            elapsed += f64::from(dt);
            assert!(run.tick(dt).is_none(), "resource worker remains blocked");
            run.pose(&mut pose);
            let position = pose.spawn.unwrap()[0];
            assert!((f64::from(position) - 4478.0 - elapsed).abs() < 0.0005);
            assert!((position - previous - dt).abs() < 0.001, "frame {frame}: no held pose or coarse jump");
            previous = position;
        }
        drop(release); // let the worker exit without a leaked blocked thread
    }
    #[test]
    fn mouse_lag_fix_defaults_to_no_gpu_barrier() {
        assert!(!Host::headless().mouse_lag_fix);
    }
    #[test]
    fn vertical_sync_preference_selects_the_surface_present_mode() {
        assert_eq!(present_mode(false), wgpu::PresentMode::AutoNoVsync);
        assert_eq!(present_mode(true), wgpu::PresentMode::AutoVsync);
        assert_eq!(Host::headless().wait_for_vert_sync, None);
    }

    /// Leak regression: `queue.write_buffer`/`write_texture` (e.g. a ticking `LiveSky`) stage pending writes that
    /// wgpu only flushes/frees on the next `queue.submit`; `Offscreen::frame` has no swapchain/present to provide
    /// one, so it must submit explicitly every tick, not just poll (confirmed live: without the submit, real
    /// IOAccelerator GPU memory grew ~170 MB/s while merely idling, invisible to wgpu-object creation counters).
    /// Paced at real time (the backlog only manifests over real seconds, not a tight unthrottled loop) with a
    /// live sky ticking every `Offscreen::frame` call for 20 real seconds; RSS after the first couple of seconds
    /// of settling must stay flat.
    #[test]
    #[ignore = "real GPU + client data, real-time paced; run explicitly: cargo test --release -p ao-render -- --ignored offscreen_frame_with_live_sky_rss_stays_flat_paced --nocapture"]
    fn offscreen_frame_with_live_sky_rss_stays_flat_paced() {
        let dir = ao_gui::client_dir();
        if !dir.join("cd_image/gui").exists() { return; }
        struct Idle(ao_gui::Gui);
        impl Frontend for Idle {
            fn gui(&self) -> &ao_gui::Gui { &self.0 }
            fn input(&mut self, _: ao_gui::InputEvent, _: &mut Host) {}
            fn frame(&mut self, dt: f32, _: (u32, u32), _: &mut Host) -> ao_gui::DrawList { self.0.frame(dt) }
        }
        let gui = ao_gui::Gui::new(&dir, None).unwrap();
        let mut fe = Idle(gui);
        let mut o = Offscreen::new(&fe, (320, 200)).unwrap();
        o.host.live_sky = Some(Some(LiveSky { start: 100.0, scale: 10.0, source: Box::new(|t| Scene { spawn: Some([t, 0.0, 0.0]), ..Scene::default() }), pose: None }));
        let rss = || -> u64 {
            let pid = std::process::id();
            let out = std::process::Command::new("ps").args(["-o", "rss=", "-p", &pid.to_string()]).output().unwrap();
            String::from_utf8_lossy(&out.stdout).trim().parse().unwrap_or(0)
        };
        let t0 = Instant::now();
        let mut samples = Vec::new();
        while t0.elapsed().as_secs() < 20 {
            o.frame(&mut fe, 0.016);
            std::thread::sleep(Duration::from_millis(16));
            let el = t0.elapsed().as_secs();
            if samples.last() != Some(&el) { samples.push(el); eprintln!("t={el} rss_kb={}", rss()); }
        }
        let warm = rss();
        std::thread::sleep(Duration::from_secs(2));
        for _ in 0..125 { o.frame(&mut fe, 0.016); std::thread::sleep(Duration::from_millis(16)); }
        let after = rss();
        eprintln!("warm={warm} after={after}");
        assert!((after as i64 - warm as i64) < 200_000, "RSS grew {} KB over 2 further real seconds of idle sky-ticking Offscreen::frame calls: pending GPU writes are not being reclaimed", after as i64 - warm as i64);
    }

    #[test]
    fn native_pacing_uses_refresh_and_does_not_catch_up_missed_frames() {
        assert_eq!(display_period(None), None);
        assert_eq!(display_period(Some(0)), None);
        let period = display_period(Some(120_000)).unwrap();
        assert!((period.as_secs_f64() - 1.0 / 120.0).abs() < 1e-9);
        let start = Instant::now();
        assert_eq!(frame_deadline(start, start + period / 2, period), start + period);
        assert_eq!(frame_deadline(start, start + period * 3, period), start + period * 3);
        let deadline = start + period;
        assert!(wait_for_frame(wgpu::PresentMode::AutoVsync, Some(period), deadline, start));
        assert!(!wait_for_frame(wgpu::PresentMode::AutoVsync, Some(period), deadline, deadline));
        assert!(!wait_for_frame(wgpu::PresentMode::AutoVsync, None, deadline, start));
        assert!(!wait_for_frame(wgpu::PresentMode::AutoNoVsync, Some(period), deadline, start));
    }

    #[test]
    fn gui_scale_rounds_and_keeps_at_least_one_pixel() {
        assert_eq!(gui_scale(1.0, None, (1920, 1080)), 1);
        assert_eq!(gui_scale(1.49, None, (1920, 1080)), 1);
        assert_eq!(gui_scale(1.5, None, (1920, 1080)), 2);
        assert_eq!(gui_scale(2.0, None, (1920, 1080)), 2);
        assert_eq!(gui_scale(0.0, None, (1920, 1080)), 1);
        assert_eq!(gui_scale(f64::NAN, None, (1920, 1080)), 1);
    }

    #[test]
    fn gui_scale_honours_explicit_requests_regardless_of_viewport() {
        assert_eq!(gui_scale(2.0, Some(0), (1200, 800)), 2);
        for scale in [1, 2, 3] {
            assert_eq!(gui_scale(2.0, Some(scale), (5120, 1440)), u32::from(scale));
        }
        assert_eq!(gui_scale(2.0, Some(2), (1600, 1200)), 2);
        assert_eq!(gui_scale(2.0, Some(3), (2560, 1600)), 3);
        assert_eq!(gui_scale(2.0, Some(1), (200, 200)), 1);
    }

    /// Trackpad fix: a gesture sent as many small `PixelDelta` events (each well under one 40 px notch) must still
    /// add up to the same whole-notch `dy` sequence a discrete mouse wheel's `LineDelta` would send, not round each
    /// event to zero (trade-list scroll) or treat each nonzero event as a full notch (map zoom overshoot).
    #[test]
    fn fractional_trackpad_pixels_accumulate_into_whole_notches() {
        let mut accum = 0.0f32;
        // 8 events of 5 px each (a realistic trackpad step) = one 40 px notch; only the last reaches a whole notch.
        let mut dys = vec![];
        for _ in 0..8 {
            dys.push(accumulate_scroll(&mut accum, 5.0));
        }
        assert_eq!(dys, vec![None, None, None, None, None, None, None, Some(1.0)], "8x 5px must add up to exactly one notch on the final event, not before");
        assert_eq!(accum, 0.0, "a whole notch must not leave a stray fractional remainder");
        // The opposite direction accumulates the same way, and leftover fractions carry across calls.
        assert_eq!(accumulate_scroll(&mut accum, -20.0), None);
        assert_eq!(accumulate_scroll(&mut accum, -20.0), Some(-1.0));
        // A single large flick (> 40 px) still yields one whole-notch step per call, not a fractional one.
        assert_eq!(accumulate_scroll(&mut accum, 100.0), Some(2.0));
        assert!((accum - 0.5).abs() < 1e-6, "the 20px remainder above the 2 whole notches must carry over");
    }

    #[test]
    fn native_shadow_sun_uses_the_loaded_environment_ray_direction() {
        let mut host = Host::headless();
        let mut env = super::super::default_environment(100.0);
        env.sun_dir = [0.3, 0.8, -0.4];
        host.set_scene(Scene { environment: Some(env), ..Scene::default() });
        assert_eq!(host.effect_sun_direction, Some([-0.3, -0.8, 0.4]));
        host.effect_dungeon = Some(true);
        host.weather_input = crate::WeatherInput { rain: 1.0, outdoor: true, surface: Some(std::rc::Rc::new(|_| Some([-1e20, 0.0]))), ..Default::default() };
        host.set_scene(Scene::default());
        assert_eq!(host.effect_sun_direction, None);
        assert_eq!(host.effect_dungeon, None);
        assert!(!host.weather_input.outdoor);
        assert_eq!(host.weather_input.rain, 0.0);
        assert!(host.weather_input.surface.is_none());
    }


    #[test]
    fn post_focus_handoff_is_explicit_and_clears_without_a_producer() {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let mut renderer = Renderer::new(&instance, None).expect("focus handoff requires GPU");
        let mut host = Host::headless();
        assert_eq!(host.post_focus, None);
        host.post_focus = Some([3.0, 4.0, -5.0]);
        host.weather_input = crate::WeatherInput {
            rain: 0.7, storm: [0.1, 0.2, 0.3, 0.4], wind: [2.0, 0.0, -3.0], outdoor: true,
            surface: Some(std::rc::Rc::new(|_| Some([-1e20, 0.0]))),
        };
        host.apply_actors(&mut renderer);
        assert_eq!(renderer.post_focus, Some([3.0, 4.0, -5.0]));
        assert_eq!(host.post_focus, None);
        assert_eq!(renderer.weather_input.rain, 0.7);
        assert_eq!(renderer.weather_input.storm, [0.1, 0.2, 0.3, 0.4]);
        assert_eq!(renderer.weather_input.wind, [2.0, 0.0, -3.0], "Host forwarding must not flip scene-space wind again");
        assert!(renderer.weather_input.outdoor);
        assert!(std::rc::Rc::ptr_eq(renderer.weather_input.surface.as_ref().unwrap(), host.weather_input.surface.as_ref().unwrap()));
        // Even an always-admitted effect must not manufacture own-character focus.
        host.actors.push(ao_scene::ActorFrame { always: true, ..Default::default() });
        host.apply_actors(&mut renderer);
        assert_eq!(renderer.post_focus, None);
    }

    #[test]
    fn actor_model_handoff_keeps_the_original_scene_allocation() {
        let scene = std::sync::Arc::new(Scene::default());
        let mut host = Host::headless();
        host.actor_models.push((1, scene.clone()));
        host.actor_model_updates.push((1, scene.clone()));
        assert!(std::sync::Arc::ptr_eq(&scene, &host.actor_models[0].1));
        assert!(std::sync::Arc::ptr_eq(&scene, &host.actor_model_updates[0].1));
        assert_eq!(std::sync::Arc::strong_count(&scene), 3);
        host.set_scene(Scene::default());
        assert_eq!(std::sync::Arc::strong_count(&scene), 1);
    }

    #[test]
    fn replacing_scene_discards_pending_old_topology_updates() {
        let mut host = Host::headless();
        assert!(host.actor_model_removals.is_empty());
        host.repose(Scene::default());
        host.actor_models.push((1, std::sync::Arc::new(Scene::default())));
        host.actor_model_removals.push(1);
        host.actor_visual_retirements.push((0xe000_0000, 32));
        host.actor_visual_resets.push((0x4000_0000, 1));
        host.set_scene(Scene::default());
        assert!(host.scene.is_some());
        assert!(host.repose.is_none());
        assert!(host.actor_models.is_empty() && host.actors.is_empty());
        assert!(host.actor_model_removals.is_empty());
        assert!(host.actor_visual_retirements.is_empty());
        assert!(host.actor_visual_resets.is_empty());
        assert!(host.clear_actors);
        assert_eq!(host.scene_generation(), 1);
        host.set_scene(Scene::default());
        assert_eq!(host.scene_generation(), 2);
    }

    #[test]
    fn actor_visual_retirement_handoff_is_bounded_without_rendering() {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let mut renderer = Renderer::new(&instance, None).expect("retirement handoff requires GPU");
        let mut host = Host::headless();
        for cycle in 0..256 {
            let first = 0xe000_0000 + cycle * 32;
            let key = super::super::VisualKey::Actor(first + 31, 1, 0, 7);
            renderer.drawn_visuals.insert(key, 3);
            renderer.pending_visuals.get_mut().insert(key, 3);
            host.actor_visual_retirements.push((first, 32));
            host.apply_actors(&mut renderer);
            assert!(host.actor_visual_retirements.is_empty());
            assert_eq!(renderer.actor_visual_retirements.len(), 1);
            assert_eq!(renderer.drawn_visuals.len(), 1);
            assert_eq!(renderer.pending_visuals.borrow().len(), 1);
        }
        host.apply_actors(&mut renderer);
        assert!(renderer.actor_visual_retirements.is_empty());
        assert!(renderer.drawn_visuals.is_empty());
        assert!(renderer.pending_visuals.borrow().is_empty());
    }

    #[test]
    fn same_id_actor_replacement_resets_priority_before_its_first_frame_only() {
        use ao_scene::{ActorFrame, Mesh, Submesh, Vertex, IDENTITY};
        use super::super::VisualKey;
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let mut renderer = Renderer::new(&instance, None).expect("same-ID visual reset requires GPU");
        let vertices = [(-0.5, -0.5), (0.5, -0.5), (0.5, 0.5), (-0.5, 0.5)].into_iter()
            .map(|(x, y)| Vertex { pos: [x, y, 0.0], normal: [0.0, 0.0, 1.0], ..Default::default() }).collect();
        let red = Scene { meshes: vec![Mesh { vertices, submeshes: vec![Submesh {
            two_sided: true, emissive: [1.0; 3], base_color: [1.0, 0.0, 0.0, 1.0],
            lighting_enabled: false, fog_enabled: false, render_priority: Some(6), render_node: Some((7, 7)),
            ..Submesh::new(vec![0, 1, 2, 0, 2, 3], None)
        }] }], ..Default::default() };
        let mut blue = red.clone();
        let blocker = &mut blue.meshes[0].submeshes[0];
        blocker.base_color = [0.0, 0.0, 1.0, 1.0];
        blocker.render_priority = Some(10);
        blocker.render_node = None;
        renderer.add_actor_model(1, &red);
        renderer.add_actor_model(2, &blue);
        let target = renderer.device.create_texture_tracked(&wgpu::TextureDescriptor {
            label: None, size: wgpu::Extent3d { width: 32, height: 32, depth_or_array_layers: 1 },
            mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2,
            format: renderer.format, usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let targets = Targets::new(&renderer, 32, 32);
        let view = target.create_view_tracked(&Default::default());
        let camera = Camera::look_at(Vec3::ZERO, Vec3::NEG_Z);
        let mut transform = IDENTITY;
        transform[3][2] = -3.0;
        let id = 0x4000_0000;
        let frames = || vec![ActorFrame { id, model: 1, transform, ..Default::default() },
            ActorFrame { id: 2, model: 2, transform, ..Default::default() }];
        let key = VisualKey::Actor(id, 1, 0, 7);
        let center = (16 * 32 + 16) * 4;
        let mut host = Host::headless();
        host.actors = frames();
        host.apply_actors(&mut renderer);
        renderer.render(&view, &targets, &camera);
        let authored_reference = super::super::texture_to_rgba(&renderer, &target, 32, 32).unwrap();
        assert_eq!(&authored_reference[center..center + 4], &[0, 0, 255, 255]);
        renderer.render(&view, &targets, &camera);
        let advanced_reference = super::super::texture_to_rgba(&renderer, &target, 32, 32).unwrap();
        assert_eq!(&advanced_reference[center..center + 4], &[255, 0, 0, 255]);
        assert_ne!(authored_reference, advanced_reference);
        // True despawn and same-ID respawn arrive in one application, sharing the model.
        // Pending history must be cleared too, and never deferred onto the new visual.
        renderer.pending_visuals.get_mut().insert(key, 7);
        host.actor_visual_resets.push((id, 1));
        host.actors = frames();
        host.apply_actors(&mut renderer);
        assert!(host.actor_visual_resets.is_empty());
        assert!(!renderer.drawn_visuals.contains_key(&key));
        assert!(!renderer.pending_visuals.borrow().contains_key(&key));
        assert!(renderer.actor_visual_retirements.is_empty());
        renderer.render(&view, &targets, &camera);
        assert_eq!(authored_reference, super::super::texture_to_rgba(&renderer, &target, 32, 32).unwrap());
        assert_eq!(renderer.drawn_visuals.get(&key), Some(&7));
        for _ in 0..2 {
            renderer.render(&view, &targets, &camera);
            assert_eq!(advanced_reference, super::super::texture_to_rgba(&renderer, &target, 32, 32).unwrap());
        }
        host.actors = frames();
        host.apply_actors(&mut renderer);
        assert_eq!(renderer.drawn_visuals.get(&key), Some(&7), "no delayed reset of the replacement");
        renderer.render(&view, &targets, &camera);
        assert_eq!(advanced_reference, super::super::texture_to_rgba(&renderer, &target, 32, 32).unwrap());
    }

    #[test]
    fn live_sky_requests_at_the_scaled_clock_and_returns_the_worker_result() {
        let source = Box::new(|t: f32| Scene { spawn: Some([t, 0.0, 0.0]), ..Scene::default() });
        let mut live = LiveRun::new(LiveSky { start: 100.0, scale: 10.0, source, pose: None });
        // the first tick asks at once (clock 100 + 0.1 s * 10), later ticks wait for the answer and 0.5 s
        assert!(live.tick(0.1).is_none() && live.busy);
        let sky = (0..200).find_map(|_| {
            std::thread::sleep(std::time::Duration::from_millis(5));
            live.tick(0.0)
        });
        assert_eq!(sky.expect("worker answers").spawn, Some([101.0, 0.0, 0.0]));
        assert!(!live.busy);
        // no new request before half a second of real time has passed
        assert!(live.tick(0.2).is_none() && !live.busy);
        assert!(live.tick(0.4).is_none() && live.busy);
    }
}
