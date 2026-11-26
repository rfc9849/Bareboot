mod keys;
mod net;

#[cfg(not(target_os = "macos"))]
mod gl;
#[cfg(target_os = "macos")]
mod mac;

use net::{Event, Sender};
use std::{collections::HashMap, sync::Arc, time::Instant};
use winit::{
    application::ApplicationHandler,
    dpi::{LogicalSize, PhysicalPosition},
    event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent},
    event_loop::{ActiveEventLoop, EventLoop, EventLoopProxy},
    keyboard::PhysicalKey,
    window::{ResizeDirection, Window, WindowAttributes, WindowId},
};

#[cfg(target_os = "macos")]
mod present {
    pub use crate::mac::{Gpu, View};
}
#[cfg(not(target_os = "macos"))]
mod present {
    pub use crate::gl::{Gpu, View};
}

struct Remote {
    id: u32,
    window: Arc<Window>,
    view: present::View,
    shown: bool,
    resized: Option<Instant>,
    expected: Option<(f64, f64)>,
}

struct App {
    sender: Sender,
    scale: u32,
    gpu: present::Gpu,
    windows: HashMap<WindowId, Remote>,
    ids: HashMap<u32, WindowId>,
    hello: bool,
}

impl App {
    fn open(&mut self, event_loop: &ActiveEventLoop, id: u32) {
        if self.ids.contains_key(&id) {
            return;
        }
        let attributes = WindowAttributes::default()
            .with_title("Bareboot")
            .with_visible(false)
            .with_inner_size(LogicalSize::new(800.0, 600.0));
        let Some((window, view)) = self.gpu.create_window(event_loop, attributes) else {
            eprintln!("could not create a window");
            return;
        };
        let wid = window.id();
        self.ids.insert(id, wid);
        self.windows.insert(wid, Remote { id, window, view, shown: false, resized: None, expected: None });
    }

    fn remote(&mut self, id: u32) -> Option<&mut Remote> {
        let wid = self.ids.get(&id)?;
        self.windows.get_mut(wid)
    }

    fn presented(remote: &mut Remote) {
        remote.window.request_redraw();
        if !remote.shown {
            remote.shown = true;
            remote.window.set_visible(true);
            remote.window.focus_window();
        }
    }
}

impl ApplicationHandler<Event> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.hello {
            return;
        }
        self.hello = true;
        let monitor = event_loop.primary_monitor().or_else(|| event_loop.available_monitors().next());
        let (factor, size) = monitor.map_or((1.0, LogicalSize::new(1920.0, 1080.0)), |m| {
            (m.scale_factor(), m.size().to_logical::<f64>(m.scale_factor()))
        });
        self.scale = (factor.ceil() as u32).clamp(1, 4);
        self.sender.send(net::HELLO, &[net::VERSION, self.scale, size.width as u32, size.height as u32]);
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: Event) {
        match event {
            Event::Open(id) => self.open(event_loop, id),
            Event::Close(id) => {
                if let Some(wid) = self.ids.remove(&id) {
                    self.windows.remove(&wid);
                }
            }
            Event::Title(id, title) => {
                if let Some(remote) = self.remote(id) {
                    remote.window.set_title(if title.is_empty() { "Bareboot" } else { &title });
                }
            }
            Event::Size(id, w, h) => {
                if let Some(remote) = self.remote(id)
                    && remote.resized.is_none_or(|t| t.elapsed().as_millis() > 500)
                {
                    let current = remote.window.inner_size().to_logical::<f64>(remote.window.scale_factor());
                    if (current.width - w as f64).abs() > 1.0 || (current.height - h as f64).abs() > 1.0 {
                        remote.expected = Some((w as f64, h as f64));
                        let _ = remote.window.request_inner_size(LogicalSize::new(w as f64, h as f64));
                    }
                }
            }
            Event::Move(id) => {
                if let Some(remote) = self.remote(id) {
                    let _ = remote.window.drag_window();
                }
            }
            Event::Resize(id, edges) => {
                let direction = match edges {
                    1 => ResizeDirection::North,
                    2 => ResizeDirection::South,
                    4 => ResizeDirection::West,
                    5 => ResizeDirection::NorthWest,
                    6 => ResizeDirection::SouthWest,
                    8 => ResizeDirection::East,
                    9 => ResizeDirection::NorthEast,
                    10 => ResizeDirection::SouthEast,
                    _ => return,
                };
                if let Some(remote) = self.remote(id) {
                    let _ = remote.window.drag_resize_window(direction);
                }
            }
            Event::Video { id, frame, crop, matrix } => {
                let scale = self.scale;
                let Some(wid) = self.ids.get(&id) else { return };
                let Some(remote) = self.windows.get_mut(wid) else { return };
                if remote.view.video(&mut self.gpu, frame, crop, matrix, scale) {
                    Self::presented(remote);
                }
            }
            Event::Pixels { id, width, height, y0, rows, data } => {
                let scale = self.scale;
                let Some(wid) = self.ids.get(&id) else { return };
                let Some(remote) = self.windows.get_mut(wid) else { return };
                remote.view.pixels(&mut self.gpu, (width, height), y0, rows, &data, scale);
                Self::presented(remote);
            }
            Event::Disconnected(reason) => {
                if !reason.is_empty() {
                    eprintln!("display disconnected: {reason}");
                }
                event_loop.exit();
            }
        }
    }

    fn window_event(&mut self, _: &ActiveEventLoop, wid: WindowId, event: WindowEvent) {
        let Some(remote) = self.windows.get_mut(&wid) else { return };
        let id = remote.id;
        let factor = remote.window.scale_factor();
        match event {
            WindowEvent::RedrawRequested => remote.view.draw(&mut self.gpu, &remote.window),
            WindowEvent::Resized(size) => {
                remote.view.resized(&mut self.gpu, &remote.window);
                let logical = size.to_logical::<f64>(factor);
                let requested = remote
                    .expected
                    .take()
                    .is_some_and(|(w, h)| (logical.width - w).abs() <= 1.0 && (logical.height - h).abs() <= 1.0);
                if remote.shown && !requested {
                    remote.resized = Some(Instant::now());
                    self.sender.send(net::CONFIGURE, &[id, logical.width.round() as u32, logical.height.round() as u32]);
                }
                remote.window.request_redraw();
            }
            WindowEvent::ScaleFactorChanged { .. } => remote.window.request_redraw(),
            WindowEvent::CloseRequested => self.sender.send(net::CLOSE_WINDOW, &[id]),
            WindowEvent::Focused(true) => self.sender.send(net::FOCUS, &[id]),
            WindowEvent::Focused(false) => self.sender.send(net::FOCUS, &[0]),
            WindowEvent::CursorMoved { position, .. } => {
                let p: PhysicalPosition<f64> = position;
                let l = p.to_logical::<f64>(factor);
                self.sender.send(net::MOTION, &[id, (l.x as f32).to_bits(), (l.y as f32).to_bits()]);
            }
            WindowEvent::CursorLeft { .. } => self.sender.send(net::LEAVE, &[id]),
            WindowEvent::MouseInput { state, button, .. } => {
                let code = match button {
                    MouseButton::Left => 272,
                    MouseButton::Right => 273,
                    MouseButton::Middle => 274,
                    MouseButton::Back => 275,
                    MouseButton::Forward => 276,
                    MouseButton::Other(_) => return,
                };
                self.sender.send(net::BUTTON, &[id, code, u32::from(state == ElementState::Pressed)]);
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let (x, y) = match delta {
                    MouseScrollDelta::LineDelta(x, y) => (-x as f64 * 10.0, -y as f64 * 10.0),
                    MouseScrollDelta::PixelDelta(p) => {
                        let l = p.to_logical::<f64>(factor);
                        (-l.x, -l.y)
                    }
                };
                if y != 0.0 {
                    self.sender.send(net::AXIS, &[id, 0, (y as f32).to_bits()]);
                }
                if x != 0.0 {
                    self.sender.send(net::AXIS, &[id, 1, (x as f32).to_bits()]);
                }
            }
            WindowEvent::KeyboardInput { event, .. } => {
                if event.repeat {
                    return;
                }
                if let PhysicalKey::Code(code) = event.physical_key
                    && let Some(key) = keys::evdev(code)
                {
                    self.sender.send(net::KEY, &[key, u32::from(event.state == ElementState::Pressed)]);
                }
            }
            _ => {}
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let valid = |s: &str| !s.is_empty() && s.len() <= 64 && s.bytes().all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b));
    if args.len() != 2 || !valid(&args[1]) {
        eprintln!("Usage: bareboot-desktop DISTRO");
        std::process::exit(2);
    }
    let connection = match net::connect(&args[1]) {
        Ok(connection) => connection,
        Err(e) => {
            eprintln!("could not open the display: {e}");
            std::process::exit(1);
        }
    };
    let event_loop = EventLoop::<Event>::with_user_event().build().expect("event loop");
    let proxy: EventLoopProxy<Event> = event_loop.create_proxy();
    let reader = connection.reader.try_clone().expect("display stream");
    let sender = connection.sender.clone();
    std::thread::spawn(move || {
        net::receive(reader, sender, |event| {
            let _ = proxy.send_event(event);
        })
    });
    let mut app = App {
        sender: connection.sender.clone(),
        scale: 1,
        gpu: present::Gpu::new(),
        windows: HashMap::new(),
        ids: HashMap::new(),
        hello: false,
    };
    let _ = event_loop.run_app(&mut app);
    drop(app);
    drop(connection);
}
