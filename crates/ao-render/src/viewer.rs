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

/// What a [`Frontend`] can ask of the window/renderer between frames.
pub struct Host {
    /// Camera used for the next frame. Written back from free-fly controls while `fly`.
    pub camera: Camera,
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
    /// Frontend request: capture the cursor and deliver [`GameInput::MouseMotion`] (mouse-look); applied after each frame/input.
    pub look: bool,
    /// Actor models to upload before the next frame (`key`, model scene), see [`Renderer::add_actor_model`]; drained by the viewer.
    pub actor_models: Vec<(u64, Scene)>,
    /// Model keys to retire before uploads and actor frames; drained by the viewer. See [`Renderer::remove_actor_model`].
    pub actor_model_removals: Vec<u64>,
    /// The actors to draw this frame ([`ao_scene::ActorFrame`]); push every frame, the viewer drains it. Actors that are not pushed are forgotten.
    pub actors: Vec<ao_scene::ActorFrame>,
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
    /// Current weather wind in AO world coordinates (`GCComputeWind`, Gamecode 0x100cd993).
    pub effect_wind: [f32; 3],
}

impl Host {
    /// Hands the queued actor models and the frame's actors to the renderer.
    fn apply_actors(&mut self, r: &mut Renderer) {
        if std::mem::take(&mut self.clear_actors) {
            r.clear_actors();
        }
        for key in self.actor_model_removals.drain(..) {
            r.remove_actor_model(key);
        }
        for (key, scene) in self.actor_models.drain(..) {
            r.add_actor_model(key, &scene);
        }
        r.set_actors(std::mem::take(&mut self.actors));
    }

    /// A host without a window or renderer (headless tests of [`Frontend`]s); scenes handed to it are kept, never drawn.
    pub fn headless() -> Self {
        Host { camera: Camera::look_at(Vec3::ZERO, -Vec3::Z), fly: false, quit: false, scene: None, repose: None, scene_generation: 0, lens: None, fog_density_scale: None, look: false, actor_models: vec![], actor_model_removals: vec![], actors: vec![], clear_actors: false, live_sky: None, sky_clock: None, mods: Default::default(), hide_cursor: false, effect_wind: [0.0; 3] }
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
        self.scene_generation = self.scene_generation.wrapping_add(1);
        // Queued poses/models belong to the replaced topology, not the newly uploaded scene.
        self.repose = None;
        self.actor_models.clear();
        self.actor_model_removals.clear();
        self.actors.clear();
        self.clear_actors = true;
        self.scene = Some(scene);
    }
}

/// Raw game input forwarded while the frontend has free-fly off (physical keys, so bindings are layout independent).
#[derive(Clone, Copy, Debug)]
pub enum GameInput {
    Key { code: KeyCode, pressed: bool, repeat: bool },
    /// Relative mouse movement in physical pixels, only while [`Host::look`] is set.
    MouseMotion { dx: f32, dy: f32 },
}

/// The windowed GUI application: draws the original AO GUI (`ao_gui`) over the 3D scene.
pub trait Frontend {
    fn gui(&self) -> &ao_gui::Gui;
    /// One input event, positions in GUI pixels.
    fn input(&mut self, ev: ao_gui::InputEvent, host: &mut Host);
    /// Raw key/mouse-look input while `host.fly` is off (default: ignored).
    fn game_input(&mut self, _ev: GameInput, _host: &mut Host) {}
    /// Per-frame logic (`dt` seconds, `size` = window in GUI pixels); returns the GUI draw list to paint.
    fn frame(&mut self, dt: f32, size: (u32, u32), host: &mut Host) -> ao_gui::DrawList;
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

    fn apply_host(live: &mut Option<Self>, host: &mut Host) {
        if let Some(next) = host.live_sky.take() {
            *live = next.map(Self::new);
        }
        if let (Some(time), Some(run)) = (host.sky_clock.take(), live.as_mut()) {
            run.day_time = time;
        }
    }

    fn advance(live: &mut Option<Self>, renderer: &mut Renderer, dt: f32) {
        if let Some(run) = live {
            renderer.day_time_rate = run.scale;
            if let Some(sky) = run.tick(dt) {
                renderer.set_sky(&sky);
            }
        }
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
}

/// winit -> GUI input. Cursor positions are physical pixels / `scale`; Cmd counts as Ctrl (macOS shortcuts).
fn gui_input(ev: &WindowEvent, cursor: &mut (f32, f32), mods: &mut ao_gui::Modifiers, scale: u32) -> Option<ao_gui::InputEvent> {
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
        WindowEvent::MouseWheel { delta, .. } => {
            let dy = match delta {
                MouseScrollDelta::LineDelta(_, y) => *y,
                MouseScrollDelta::PixelDelta(p) => p.y as f32 / 40.0,
            };
            Some(I::Wheel { x: cursor.0, y: cursor.1, dy })
        }
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
    cursor_hidden: bool,
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
            Gui { renderer, frontend, host: Host { camera: cam, ..Host::headless() }, cursor: (0.0, 0.0), mods: Default::default(), scale: (window.scale_factor().round() as u32).max(1) }
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
            cursor_hidden: false,
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
        self.sync_cursor();
    }

    /// The OS cursor is hidden while the mouse looks and while the frontend draws its own pointer ([`Host::hide_cursor`]).
    fn sync_cursor(&mut self) {
        let hide = self.looking || self.gui.as_ref().is_some_and(|g| g.host.hide_cursor);
        if hide != self.cursor_hidden {
            self.cursor_hidden = hide;
            self.window.set_cursor_visible(!hide);
        }
    }

    /// Runs the frontend; returns its draw list (`None` without a frontend). True in `.1` = quit.
    fn run_gui(&mut self, dt: f32) -> (Option<ao_gui::DrawList>, bool) {
        let Some(g) = &mut self.gui else { return (None, false) };
        g.host.camera = self.cam;
        let size = (self.config.width / g.scale, self.config.height / g.scale);
        let list = g.frontend.frame(dt, size, &mut g.host);
        self.cam = g.host.camera;
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
        g.host.apply_actors(&mut self.renderer);
        (Some(list), g.host.quit)
    }

    fn paint_gui(&mut self, list: ao_gui::DrawList, view: &wgpu::TextureView) {
        let Some(g) = &mut self.gui else { return };
        g.renderer.draw(&self.renderer, view, (self.config.width, self.config.height), g.scale, g.frontend.gui(), &list);
    }

    /// True = quit requested.
    fn frame(&mut self) -> bool {
        let now = Instant::now();
        // Retail high-resolution Timer_t bounds elapsed time at 100 seconds, not 0.1.
        let dt = (now - self.last).as_secs_f32().min(100.0);
        self.last = now;
        self.clock += dt;
        self.renderer.time = self.clock;
        LiveRun::advance(&mut self.live, &mut self.renderer, dt);

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

        if self.fly() {
            let k = |c| self.keys.contains(&c) as i32 as f32;
            let fast = if self.keys.contains(&KeyCode::ShiftLeft) || self.keys.contains(&KeyCode::ShiftRight) { 5.0 } else { 1.0 };
            let step = self.speed * fast * dt;
            let up = k(KeyCode::Space) + k(KeyCode::KeyE) - k(KeyCode::ControlLeft) - k(KeyCode::KeyQ);
            let c = &mut self.cam;
            c.pos += (c.forward() * (k(KeyCode::KeyW) - k(KeyCode::KeyS)) + c.right() * (k(KeyCode::KeyD) - k(KeyCode::KeyA)) + Vec3::Y * up) * step;
        }
        let (gui_out, quit) = self.run_gui(dt);
        if let Some(want) = self.gui.as_ref().filter(|_| !self.fly()).map(|g| g.host.look) {
            if want != self.looking {
                self.set_look(want);
            }
        }
        self.sync_cursor();
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
        if let Some(list) = gui_out {
            let gview = crate::GuiRenderer::view(&frame.texture);
            self.paint_gui(list, &gview);
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
        if let Some(g) = &mut s.gui {
            let gi = gui_input(&ev, &mut g.cursor, &mut g.mods, g.scale);
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
            WindowEvent::CloseRequested => el.exit(),
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
        let (Some(s), DeviceEvent::MouseMotion { delta: (dx, dy) }) = (&mut self.state, ev) else { return };
        if !s.looking {
            return;
        }
        match s.gui.as_mut().filter(|g| !g.host.fly) {
            Some(g) => {
                g.host.camera = s.cam;
                g.frontend.game_input(GameInput::MouseMotion { dx: dx as f32, dy: dy as f32 }, &mut g.host);
                s.cam = g.host.camera;
            }
            None => {
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

/// A [`Frontend`] driven without a window: the same per-frame sequence as the windowed app (`frame`, scene upload, lens,
/// 3D render, GUI draw) into an offscreen texture. Used by the headless screenshot tests.
pub struct Offscreen {
    r: Renderer,
    targets: Targets,
    gr: crate::GuiRenderer,
    pub host: Host,
    size: (u32, u32),
    live: Option<LiveRun>,
}

impl Offscreen {
    pub fn new(fe: &dyn Frontend, size: (u32, u32)) -> Result<Self> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let r = Renderer::new(&instance, None)?;
        let (targets, gr) = (Targets::new(&r, size.0, size.1), crate::GuiRenderer::new(&r, fe.gui()));
        Ok(Self { r, targets, gr, host: Host::headless(), size, live: None })
    }

    /// Changes the render surface size without replacing the live frontend or its GUI state.
    pub fn resize(&mut self, size: (u32, u32)) {
        self.size = (size.0.max(1), size.1.max(1));
        self.targets = Targets::new(&self.r, self.size.0, self.size.1);
    }

    /// One `Frontend::frame`, applying what it asked of the host.
    pub fn frame(&mut self, fe: &mut dyn Frontend, dt: f32) -> ao_gui::DrawList {
        self.r.time += dt;
        LiveRun::advance(&mut self.live, &mut self.r, dt);
        let list = fe.frame(dt, self.size, &mut self.host);
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
        self.host.apply_actors(&mut self.r);
        list
    }

    /// Renders the current scene with the host camera, draws `list` over it and writes the PNG.
    pub fn png(&mut self, fe: &dyn Frontend, list: &ao_gui::DrawList, path: &std::path::Path) -> Result<()> {
        let tex = self.r.device.create_texture(&wgpu::TextureDescriptor {
            label: None,
            size: wgpu::Extent3d { width: self.size.0, height: self.size.1, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: self.r.format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = tex.create_view(&Default::default());
        self.r.render(&view, &self.targets, &self.host.camera);
        self.gr.draw(&self.r, &crate::GuiRenderer::view(&tex), self.size, 1, fe.gui(), list);
        crate::texture_to_png(&self.r, &tex, self.size.0, self.size.1, path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replacing_scene_discards_pending_old_topology_updates() {
        let mut host = Host::headless();
        assert!(host.actor_model_removals.is_empty());
        host.repose(Scene::default());
        host.actor_models.push((1, Scene::default()));
        host.actor_model_removals.push(1);
        host.set_scene(Scene::default());
        assert!(host.scene.is_some());
        assert!(host.repose.is_none());
        assert!(host.actor_models.is_empty() && host.actors.is_empty());
        assert!(host.actor_model_removals.is_empty());
        assert!(host.clear_actors);
        assert_eq!(host.scene_generation(), 1);
        host.set_scene(Scene::default());
        assert_eq!(host.scene_generation(), 2);
    }

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
