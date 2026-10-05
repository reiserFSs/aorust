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
}

struct App {
    scene: Scene,
    state: Option<State>,
    error: Option<anyhow::Error>,
}

/// Opens the interactive viewer; returns when the window closes.
pub fn run_viewer(scene: Scene) -> Result<()> {
    let el = EventLoop::new()?;
    el.set_control_flow(ControlFlow::Poll);
    let mut app = App { scene, state: None, error: None };
    el.run_app(&mut app)?;
    app.error.map_or(Ok(()), Err)
}

impl State {
    fn new(el: &ActiveEventLoop, scene: &Scene) -> Result<Self> {
        let window = Arc::new(el.create_window(
            Window::default_attributes().with_title("aomac").with_inner_size(winit::dpi::LogicalSize::new(1280, 800)),
        )?);
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
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
        let now = Instant::now();
        Ok(Self {
            window,
            surface,
            config,
            renderer,
            targets,
            cam: Camera::look_at(eye, at),
            keys: HashSet::new(),
            speed,
            looking: false,
            last: now,
            stat_t: now,
            stat_frames: 0,
            perf: std::env::var_os("AOMAC_PERF").is_some(),
        })
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

    fn frame(&mut self) {
        let now = Instant::now();
        let dt = (now - self.last).as_secs_f32().min(0.1);
        self.last = now;

        // HiDPI/resize: follow the physical size every frame.
        let PhysicalSize { width, height } = self.window.inner_size();
        if width == 0 || height == 0 {
            return;
        }
        if (width, height) != (self.config.width, self.config.height) {
            self.config.width = width;
            self.config.height = height;
            self.surface.configure(&self.renderer.device, &self.config);
            self.targets = Targets::new(&self.renderer, width, height);
        }

        let k = |c| self.keys.contains(&c) as i32 as f32;
        let fast = if self.keys.contains(&KeyCode::ShiftLeft) || self.keys.contains(&KeyCode::ShiftRight) { 5.0 } else { 1.0 };
        let step = self.speed * fast * dt;
        let up = k(KeyCode::Space) + k(KeyCode::KeyE) - k(KeyCode::ControlLeft) - k(KeyCode::KeyQ);
        let c = &mut self.cam;
        c.pos += (c.forward() * (k(KeyCode::KeyW) - k(KeyCode::KeyS)) + c.right() * (k(KeyCode::KeyD) - k(KeyCode::KeyA)) + Vec3::Y * up) * step;

        let frame = match self.surface.get_current_texture() {
            Ok(f) => f,
            Err(_) => {
                self.surface.configure(&self.renderer.device, &self.config);
                return;
            }
        };
        let view = frame.texture.create_view(&Default::default());
        self.renderer.render(&view, &self.targets, &self.cam);
        self.window.pre_present_notify();
        frame.present();

        self.stat_frames += 1;
        let el = self.stat_t.elapsed().as_secs_f32();
        if el >= 0.5 {
            let fps = self.stat_frames as f32 / el;
            let p = self.cam.pos;
            self.window.set_title(&format!("aomac | {fps:.0} fps | {:.1} {:.1} {:.1} | speed {:.0}", p.x, p.y, p.z, self.speed));
            if self.perf {
                eprintln!("{:.2} ms/frame ({fps:.0} fps)", 1000.0 / fps);
            }
            self.stat_t = Instant::now();
            self.stat_frames = 0;
        }
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, el: &ActiveEventLoop) {
        if self.state.is_none() {
            match State::new(el, &self.scene) {
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
        match ev {
            WindowEvent::CloseRequested => el.exit(),
            WindowEvent::KeyboardInput { event, .. } => {
                if let PhysicalKey::Code(code) = event.physical_key {
                    if event.state == ElementState::Pressed {
                        if code == KeyCode::Escape {
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
            WindowEvent::MouseInput { button: MouseButton::Right, state, .. } => s.set_look(state == ElementState::Pressed),
            WindowEvent::MouseWheel { delta, .. } => {
                let y = match delta {
                    MouseScrollDelta::LineDelta(_, y) => y,
                    MouseScrollDelta::PixelDelta(p) => p.y as f32 / 40.0,
                };
                s.speed = (s.speed * 1.15f32.powf(y)).clamp(0.1, 100_000.0);
            }
            WindowEvent::RedrawRequested => {
                s.frame();
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
