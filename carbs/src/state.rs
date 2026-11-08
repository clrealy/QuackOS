use std::{ffi::OsString, sync::Arc, time::Instant};

use smithay::{
    desktop::{PopupManager, Space, Window, WindowSurfaceType},
    input::{
        keyboard::XkbConfig,
        pointer::{CursorImageStatus, Focus, GrabStartData},
        Seat, SeatState,
    },
    output::Output,
    reexports::{
        calloop::{generic::Generic, EventLoop, Interest, LoopSignal, Mode, PostAction},
        wayland_protocols::xdg::shell::server::xdg_toplevel,
        wayland_server::{
            backend::{ClientData, ClientId, DisconnectReason},
            protocol::wl_surface::WlSurface,
            Display, DisplayHandle, Resource,
        },
    },
    utils::{Logical, Point, Rectangle, Serial},
    wayland::{
        compositor::{get_parent, CompositorClientState, CompositorState},
        output::OutputManagerState,
        selection::{data_device::DataDeviceState, primary_selection::PrimarySelectionState},
        shell::xdg::{decoration::XdgDecorationState, XdgShellState},
        shm::ShmState,
        socket::ListeningSocketSource,
    },
};

use crate::{
    config::Config,
    cursor::Arrow,
    grabs::{self, MoveGrab, ResizeEdge, ResizeGrab},
};

/// Where the pointer sits inside a window that was just un-maximised by dragging:
/// near the top, where a titlebar would be.
const TITLE_GRAB_STRIP: i32 = 16;

/// All of Carbs' state lives here.
pub struct Carbs {
    pub start_time: Instant,
    pub socket_name: OsString,
    pub display_handle: DisplayHandle,
    pub loop_signal: LoopSignal,

    pub space: Space<Window>,
    pub popups: PopupManager,

    pub compositor_state: CompositorState,
    pub xdg_shell_state: XdgShellState,
    /// Kept alive so the decoration global stays registered; never read directly.
    #[allow(dead_code)]
    pub xdg_decoration_state: XdgDecorationState,
    pub shm_state: ShmState,
    /// Kept alive so the xdg-output global stays registered; never read directly.
    #[allow(dead_code)]
    pub output_manager_state: OutputManagerState,
    pub seat_state: SeatState<Carbs>,
    pub data_device_state: DataDeviceState,
    pub primary_selection_state: PrimarySelectionState,
    pub seat: Seat<Carbs>,

    /// Settings read from carbs.conf at startup.
    pub config: Config,
    /// What the cursor should look like right now.
    pub cursor_status: CursorImageStatus,
    /// Carbs' own arrow, for the background and for clients that set no cursor.
    pub cursor_arrow: Arrow,
    /// The surface the pointer was last over, so we can reset the cursor when it
    /// crosses into a different surface (the image is undefined until that surface
    /// sets one, so carrying the old one over would be wrong).
    pub pointer_focus: Option<WlSurface>,
}

impl Carbs {
    pub fn new(event_loop: &mut EventLoop<'static, Carbs>, display: Display<Carbs>) -> Self {
        let dh = display.handle();
        let config = Self::resolve_config(Config::load());

        let compositor_state = CompositorState::new::<Self>(&dh);
        let xdg_shell_state = XdgShellState::new::<Self>(&dh);
        let xdg_decoration_state = XdgDecorationState::new::<Self>(&dh);
        let shm_state = ShmState::new::<Self>(&dh, vec![]);
        let output_manager_state = OutputManagerState::new_with_xdg_output::<Self>(&dh);
        let data_device_state = DataDeviceState::new::<Self>(&dh);
        let primary_selection_state = PrimarySelectionState::new::<Self>(&dh);

        let mut seat_state = SeatState::new();
        let mut seat: Seat<Self> = seat_state.new_wl_seat(&dh, "carbs");

        let xkb = XkbConfig {
            layout: &config.keyboard_layout,
            variant: &config.keyboard_variant,
            options: config.keyboard_options.clone(),
            ..Default::default()
        };
        // A bad layout in the config must not stop the desktop coming up.
        if seat.add_keyboard(xkb, config.repeat_delay, config.repeat_rate).is_err() {
            tracing::error!(
                "xkb rejected layout {:?} variant {:?}; falling back to us",
                config.keyboard_layout,
                config.keyboard_variant
            );
            seat.add_keyboard(Default::default(), config.repeat_delay, config.repeat_rate)
                .expect("keyboard (is xkbcommon working?)");
        }
        seat.add_pointer();

        let socket_name = Self::init_wayland_listener(display, event_loop);

        Self {
            start_time: Instant::now(),
            socket_name,
            display_handle: dh,
            loop_signal: event_loop.get_signal(),
            space: Space::default(),
            popups: PopupManager::default(),
            compositor_state,
            xdg_shell_state,
            xdg_decoration_state,
            shm_state,
            output_manager_state,
            seat_state,
            data_device_state,
            primary_selection_state,
            seat,
            cursor_arrow: Arrow::new(config.cursor_size),
            config,
            cursor_status: CursorImageStatus::default_named(),
            pointer_focus: None,
        }
    }

    /// Opens the Wayland socket (wayland-N) and wires clients + requests into the event loop.
    fn init_wayland_listener(display: Display<Carbs>, event_loop: &mut EventLoop<'static, Carbs>) -> OsString {
        let socket = ListeningSocketSource::new_auto().expect("couldn't create a wayland socket");
        let socket_name = socket.socket_name().to_os_string();
        let handle = event_loop.handle();

        handle
            .insert_source(socket, |stream, _, state| {
                state
                    .display_handle
                    .insert_client(stream, Arc::new(ClientState::default()))
                    .expect("couldn't add client");
            })
            .expect("couldn't register socket");

        handle
            .insert_source(Generic::new(display, Interest::READ, Mode::Level), |_, display, state| {
                // SAFETY: we never drop the display while it's registered in the loop
                unsafe { display.get_mut().dispatch_clients(state).unwrap() };
                Ok(PostAction::Continue)
            })
            .expect("couldn't register display");

        socket_name
    }

    /// Fills in anything the config left for Carbs to work out, like which
    /// terminal is actually installed.
    fn resolve_config(mut config: Config) -> Config {
        if config.terminal.is_empty() {
            config.terminal = crate::config::first_on_path(crate::config::TERMINAL_CANDIDATES)
                .unwrap_or_default();
            if config.terminal.is_empty() {
                tracing::warn!("no terminal found on PATH; set `terminal = ...` in carbs.conf");
            } else {
                tracing::info!("using {} as the terminal", config.terminal);
            }
        }
        if config.launcher.is_empty() {
            config.launcher = crate::config::first_on_path(crate::config::LAUNCHER_CANDIDATES)
                .unwrap_or_default();
        }
        config
    }

    /// Area of `geometry` left over once layer-shell clients have reserved theirs.
    /// Nothing reserves anything yet; layer-shell fills this in.
    pub fn subtract_exclusive_zones(
        &self,
        _output: &Output,
        geometry: Rectangle<i32, Logical>,
    ) -> Rectangle<i32, Logical> {
        geometry
    }

    /// Topmost surface under `pos` (compositor coordinates) and that surface's origin.
    pub fn surface_under(&self, pos: Point<f64, Logical>) -> Option<(WlSurface, Point<f64, Logical>)> {
        self.space.element_under(pos).and_then(|(window, location)| {
            window
                .surface_under(pos - location.to_f64(), WindowSurfaceType::ALL)
                .map(|(s, p)| (s, (p + location).to_f64()))
        })
    }

    /// Walks up from a subsurface/popup to the surface its toplevel owns.
    pub fn root_surface(surface: &WlSurface) -> WlSurface {
        let mut root = surface.clone();
        while let Some(parent) = get_parent(&root) {
            root = parent;
        }
        root
    }

    /// The mapped window whose toplevel owns `surface` (exactly, not via a child).
    pub fn window_for_surface(&self, surface: &WlSurface) -> Option<Window> {
        self.space
            .elements()
            .find(|w| w.toplevel().map(|t| t.wl_surface()) == Some(surface))
            .cloned()
    }

    /// The window that currently holds keyboard focus.
    pub fn focused_window(&self) -> Option<Window> {
        let focus = self.seat.get_keyboard()?.current_focus()?;
        self.window_for_surface(&Self::root_surface(&focus))
    }

    /// Asks the focused window to close. It's a request: the client may put up a
    /// "really quit?" dialog, or ignore us. Carbs does not kill clients.
    pub fn close_focused_window(&mut self) {
        if let Some(toplevel) = self.focused_window().and_then(|w| w.toplevel().cloned()) {
            toplevel.send_close();
        }
    }

    /// Validates a client's move/resize request: the pointer must really be grabbed
    /// with that serial, by a surface belonging to the same client. Without this check
    /// any client could start dragging a window whenever it felt like it.
    pub fn client_grab_start_data(
        &self,
        surface: &WlSurface,
        serial: Serial,
    ) -> Option<GrabStartData<Self>> {
        let pointer = self.seat.get_pointer()?;
        if !pointer.has_grab(serial) {
            return None;
        }
        let start_data = pointer.grab_start_data()?;
        let (focus, _) = start_data.focus.as_ref()?;
        if !focus.id().same_client_as(&surface.id()) {
            return None;
        }
        Some(start_data)
    }

    /// Starts dragging `window` around.
    pub fn start_move(&mut self, window: &Window, start_data: GrabStartData<Self>, serial: Serial) {
        // Dragging a maximised or fullscreen window restores it first, the way
        // every other desktop does, and drops it under the pointer.
        if self.is_tiled(window) {
            if let Some(restored) = self.restore_floating(window) {
                let wanted = Point::from((
                    start_data.location.x.round() as i32 - restored.size.w / 2,
                    start_data.location.y.round() as i32 - TITLE_GRAB_STRIP,
                ));
                // Clamp against the size it is becoming, not the size it still has.
                let location = match self.work_area_for_window(window) {
                    Some(area) => crate::shell::clamp_to_area(area, restored.size, wanted),
                    None => wanted,
                };
                self.space.map_element(window.clone(), location, true);
            }
        }

        let Some(initial_location) = self.space.element_location(window) else {
            return;
        };
        let Some(pointer) = self.seat.get_pointer() else {
            return;
        };
        self.focus_window(window);
        let grab = MoveGrab::new(start_data, window.clone(), initial_location);
        pointer.set_grab(self, grab, serial, Focus::Clear);
    }

    /// Starts resizing `window` from `edges`.
    pub fn start_resize(
        &mut self,
        window: &Window,
        edges: ResizeEdge,
        start_data: GrabStartData<Self>,
        serial: Serial,
    ) {
        if edges == ResizeEdge::NONE {
            return;
        }
        let Some(toplevel) = window.toplevel().cloned() else {
            return;
        };
        let Some(location) = self.space.element_location(window) else {
            return;
        };
        let Some(pointer) = self.seat.get_pointer() else {
            return;
        };

        let initial_rect = Rectangle::new(location, window.geometry().size);
        grabs::resize_started(toplevel.wl_surface(), edges, initial_rect);

        toplevel.with_pending_state(|state| {
            state.states.set(xdg_toplevel::State::Resizing);
        });
        toplevel.send_pending_configure();

        self.focus_window(window);
        let grab = ResizeGrab::new(start_data, window.clone(), edges, initial_rect);
        pointer.set_grab(self, grab, serial, Focus::Clear);
    }
}

#[derive(Default)]
pub struct ClientState {
    pub compositor_state: CompositorClientState,
}

impl ClientData for ClientState {
    fn initialized(&self, _client_id: ClientId) {}
    fn disconnected(&self, _client_id: ClientId, _reason: DisconnectReason) {}
}
