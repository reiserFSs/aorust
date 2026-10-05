use crate::{default_view, Camera, Renderer, Targets, Vec3};
use anyhow::Result;
use ao_scene::Scene;
use std::collections::HashSet;
use std::sync::Arc;
use std::time::Instant;
use winit::application::ApplicationHandler;
use winit::dpi::PhysicalSize;
use winit::event::{DeviceEvent, DeviceId, ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{CursorGrabMode, Window, WindowId};

/// What an egui [`Frontend`] can ask of the window/renderer between frames.
pub struct Host {
    /// Camera used for the next frame. Written back from free-fly controls while `fly`.
    pub camera: Camera,
    /// WASD + right-mouse-look free-fly controls (the viewer's default).
    pub fly: bool,
    /// Close the window after this frame.
    pub quit: bool,
    scene: Option<Scene>,
    repose: Option<Scene>,
}

impl Host {
    /// Updates vertex positions/instance transforms of the current scene in place ([`Renderer::repose`]).
    pub fn repose(&mut self, scene: Scene) {
        self.repose = Some(scene);
    }

    /// Replaces the rendered scene (uploaded before the next frame).
    pub fn set_scene(&mut self, scene: Scene) {
        self.scene = Some(scene);
    }
}

/// An egui application drawn on top of the 3D scene (login screen, character select, ...).
pub trait Frontend {
    /// Called once per frame with the root `ui`; `dt` in seconds.
    fn frame(&mut self, ui: &mut egui::Ui, host: &mut Host, dt: f32);
}

/// Live time of day: `source(day_time)` returns a scene holding only the sky, its new textures and the environment
/// ([`Renderer::set_sky`]); it runs on a worker thread, about twice a second of real time.
pub struct LiveSky {
    /// Game day time (seconds) at start.
    pub start: f32,
    /// Game seconds per real second.
    pub scale: f32,
    pub source: Box<dyn FnMut(f32) -> Scene + Send>,
}

/// Worker thread + clock of a [`LiveSky`].
struct LiveRun {
    day_time: f32,
    scale: f32,
    since: f32,
    busy: bool,
    req: std::sync::mpsc::Sender<f32>,
    resp: std::sync::mpsc::Receiver<Scene>,
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
        Self { day_time: live.start, scale: live.scale, since: f32::MAX, busy: false, req, resp }
    }

    /// Advances the clock by `dt` real seconds; the freshly computed sky, if one arrived.
    fn tick(&mut self, dt: f32) -> Option<Scene> {
        self.day_time += dt * self.scale;
        self.since += dt;
        let got = self.resp.try_recv().ok();
        self.busy &= got.is_none();
        if !self.busy && self.since >= 0.5 && self.req.send(self.day_time).is_ok() {
            (self.busy, self.since) = (true, 0.0);
        }
        got
    }
}

struct Gui {
    ctx: egui::Context,
    winit: egui_winit::State,
    renderer: egui_wgpu::Renderer,
    frontend: Box<dyn Frontend>,
    host: Host,
    free: Vec<egui::TextureId>,
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
    last: Instant,
    stat_t: Instant,
    stat_frames: u32,
    perf: bool,
    cpu: f32,
    gui: Option<Gui>,
    /// Seconds since start (drives `Submesh::uv_scroll`).
    clock: f32,
    live: Option<LiveRun>,
    hook: Option<FrameHook>,
}

/// Called once per frame with the camera position and the frame time (audio listener).
pub type FrameHook = Box<dyn FnMut(Vec3, f32)>;

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

/// [`run_viewer`]/[`run_viewer_live`] calling `hook(camera position, dt)` every frame.
pub fn run_viewer_hooked(scene: Scene, live: Option<LiveSky>, hook: FrameHook) -> Result<()> {
    run(scene, None, live, Some(hook))
}

/// Opens the window with `frontend` drawn over `scene` (free-fly off until the frontend enables it).
pub fn run_frontend(scene: Scene, frontend: impl Frontend + 'static) -> Result<()> {
    run(scene, Some(Box::new(frontend)), None, None)
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
        let window = Arc::new(el.create_window(
            Window::default_attributes().with_title("aomac").with_inner_size(winit::dpi::LogicalSize::new(1280, 800)),
        )?);
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
            desired_maximum_frame_latency: 2,
            alpha_mode: wgpu::CompositeAlphaMode::Auto,
            // egui blends in gamma space: it draws through a non-sRGB view of the same texture.
            view_formats: vec![renderer.format.remove_srgb_suffix()],
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
            let ctx = egui::Context::default();
            let winit = egui_winit::State::new(ctx.clone(), egui::ViewportId::ROOT, &*window, Some(window.scale_factor() as f32), None, None);
            let renderer = egui_wgpu::Renderer::new(&renderer.device, renderer.format.remove_srgb_suffix(), egui_wgpu::RendererOptions::default());
            Gui { ctx, winit, renderer, frontend, host: Host { camera: cam, fly: false, quit: false, scene: None, repose: None }, free: vec![] }
        });
        let now = Instant::now();
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
            last: now,
            stat_t: now,
            stat_frames: 0,
            cpu: 0.0,
            perf: std::env::var_os("AOMAC_PERF").is_some(),
            gui,
            clock: 0.0,
            live: live.map(LiveRun::new),
            hook,
        })
    }

    fn fly(&self) -> bool {
        self.gui.as_ref().is_none_or(|g| g.host.fly)
    }

    fn set_look(&mut self, on: bool) {
        self.looking = on;
        if on {
            let _ = self.window.set_cursor_grab(CursorGrabMode::Locked).or_else(|_| self.window.set_cursor_grab(CursorGrabMode::Confined));
        } else {
            let _ = self.window.set_cursor_grab(CursorGrabMode::None);
        }
        self.window.set_cursor_visible(!on);
    }

    /// Runs the frontend; returns the egui output to paint (`None` without a frontend). True in `.1` = quit.
    fn run_gui(&mut self, dt: f32) -> (Option<egui::FullOutput>, bool) {
        let Some(g) = &mut self.gui else { return (None, false) };
        g.host.camera = self.cam;
        let input = g.winit.take_egui_input(&self.window);
        let (frontend, host) = (&mut g.frontend, &mut g.host);
        let out = g.ctx.run_ui(input, |ui| frontend.frame(ui, host, dt));
        g.winit.handle_platform_output(&self.window, out.platform_output.clone());
        // Texture deltas apply now, not at paint time: a frame whose surface texture is unavailable still consumes them.
        for (id, delta) in &out.textures_delta.set {
            g.renderer.update_texture(&self.renderer.device, &self.renderer.queue, *id, delta);
        }
        g.free.extend(out.textures_delta.free.iter().copied());
        self.cam = g.host.camera;
        if let Some(scene) = g.host.scene.take() {
            self.renderer.upload(&scene);
            self.speed = (self.renderer.radius() * 0.15).max(5.0);
        }
        if let Some(scene) = g.host.repose.take() {
            self.renderer.repose(&scene);
        }
        (Some(out), g.host.quit)
    }

    fn paint_gui(&mut self, out: egui::FullOutput, frame: &wgpu::SurfaceTexture) {
        let Some(g) = &mut self.gui else { return };
        let (dev, queue) = (&self.renderer.device, &self.renderer.queue);
        let view = frame.texture.create_view(&wgpu::TextureViewDescriptor { format: Some(self.renderer.format.remove_srgb_suffix()), ..Default::default() });
        let screen = egui_wgpu::ScreenDescriptor { size_in_pixels: [self.config.width, self.config.height], pixels_per_point: out.pixels_per_point };
        let jobs = g.ctx.tessellate(out.shapes, out.pixels_per_point);
        let mut enc = dev.create_command_encoder(&Default::default());
        let extra = g.renderer.update_buffers(dev, queue, &mut enc, &jobs, &screen);
        {
            let mut pass = enc
                .begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("egui"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &view,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store },
                    })],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                    multiview_mask: None,
                })
                .forget_lifetime();
            g.renderer.render(&mut pass, &jobs, &screen);
        }
        queue.submit(extra.into_iter().chain([enc.finish()]));
        for id in g.free.drain(..) {
            g.renderer.free_texture(&id);
        }
    }

    /// True = quit requested.
    fn frame(&mut self) -> bool {
        let now = Instant::now();
        let dt = (now - self.last).as_secs_f32().min(0.1);
        self.last = now;
        self.clock += dt;
        self.renderer.time = self.clock;
        if let Some(sky) = self.live.as_mut().and_then(|l| l.tick(dt)) {
            self.renderer.set_sky(&sky);
        }

        // HiDPI/resize: follow the physical size every frame.
        let PhysicalSize { width, height } = self.window.inner_size();
        if width == 0 || height == 0 {
            return false;
        }
        if (width, height) != (self.config.width, self.config.height) {
            self.config.width = width;
            self.config.height = height;
            self.surface.configure(&self.renderer.device, &self.config);
            self.targets = Targets::new(&self.renderer, width, height);
        }

        let typing = self.gui.as_ref().is_some_and(|g| g.ctx.egui_wants_keyboard_input());
        if self.fly() && !typing {
            let k = |c| self.keys.contains(&c) as i32 as f32;
            let fast = if self.keys.contains(&KeyCode::ShiftLeft) || self.keys.contains(&KeyCode::ShiftRight) { 5.0 } else { 1.0 };
            let step = self.speed * fast * dt;
            let up = k(KeyCode::Space) + k(KeyCode::KeyE) - k(KeyCode::ControlLeft) - k(KeyCode::KeyQ);
            let c = &mut self.cam;
            c.pos += (c.forward() * (k(KeyCode::KeyW) - k(KeyCode::KeyS)) + c.right() * (k(KeyCode::KeyD) - k(KeyCode::KeyA)) + Vec3::Y * up) * step;
        }
        let (gui_out, quit) = self.run_gui(dt);
        if let Some(h) = self.hook.as_mut() {
            h(self.cam.pos, dt);
        }

        let frame = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(f) | wgpu::CurrentSurfaceTexture::Suboptimal(f) => f,
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                self.surface.configure(&self.renderer.device, &self.config);
                return quit;
            }
            _ => return quit, // timeout / occluded: skip this frame
        };
        let view = frame.texture.create_view(&Default::default());
        let t0 = Instant::now();
        self.renderer.render(&view, &self.targets, &self.cam);
        self.cpu += t0.elapsed().as_secs_f32();
        if let Some(out) = gui_out {
            self.paint_gui(out, &frame);
        }
        self.window.pre_present_notify();
        frame.present();

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
                eprintln!("{:.2} ms/frame ({fps:.0} fps), render() cpu {:.2} ms, {} instances, {} draws", 1000.0 / fps, 1000.0 * self.cpu / self.stat_frames as f32, st.instances, st.draw_calls);
                self.cpu = 0.0;
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
        let mut over_gui = false;
        if let Some(g) = &mut s.gui {
            let r = g.winit.on_window_event(&s.window, &ev);
            over_gui = r.consumed || (matches!(ev, WindowEvent::MouseInput { .. } | WindowEvent::MouseWheel { .. }) && g.ctx.egui_wants_pointer_input());
        }
        match ev {
            WindowEvent::CloseRequested => el.exit(),
            WindowEvent::KeyboardInput { event, .. } if !over_gui => {
                if let PhysicalKey::Code(code) = event.physical_key {
                    if event.state == ElementState::Pressed {
                        // Escape quits the plain viewer only; the egui frontend owns it otherwise.
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
            }
            WindowEvent::MouseInput { button: MouseButton::Right, state, .. } if s.fly() && (!over_gui || state == ElementState::Released) => {
                s.set_look(state == ElementState::Pressed)
            }
            WindowEvent::MouseWheel { delta, .. } if s.fly() && !over_gui => {
                let y = match delta {
                    MouseScrollDelta::LineDelta(_, y) => y,
                    MouseScrollDelta::PixelDelta(p) => p.y as f32 / 40.0,
                };
                s.speed = (s.speed * 1.15f32.powf(y)).clamp(0.1, 100_000.0);
            }
            WindowEvent::RedrawRequested => {
                if s.frame() {
                    el.exit();
                    return;
                }
                s.window.request_redraw();
            }
            _ => {}
        }
    }

    fn device_event(&mut self, _: &ActiveEventLoop, _: DeviceId, ev: DeviceEvent) {
        if let (Some(s), DeviceEvent::MouseMotion { delta: (dx, dy) }) = (&mut self.state, ev) {
            if s.looking {
                s.cam.yaw += dx as f32 * 0.0025;
                s.cam.pitch = (s.cam.pitch - dy as f32 * 0.0025).clamp(-1.55, 1.55);
            }
        }
    }

    fn about_to_wait(&mut self, _: &ActiveEventLoop) {
        if let Some(s) = &self.state {
            s.window.request_redraw();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn live_sky_requests_at_the_scaled_clock_and_returns_the_worker_result() {
        let source = Box::new(|t: f32| Scene { spawn: Some([t, 0.0, 0.0]), ..Scene::default() });
        let mut live = LiveRun::new(LiveSky { start: 100.0, scale: 10.0, source });
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
