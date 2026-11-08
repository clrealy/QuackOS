use std::{ffi::OsString, sync::Arc, time::Instant};

use smithay::{
    desktop::{PopupManager, Space, Window, WindowSurfaceType},
    input::{
        pointer::{CursorImageStatus, Focus, GrabStartData},
        Seat, SeatState,
    },
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
        selection::data_device::DataDeviceState,
        shell::xdg::XdgShellState,
        shm::ShmState,
        socket::ListeningSocketSource,
    },
};

use crate::{
    cursor::{self, Arrow},
    grabs::{self, MoveGrab, ResizeEdge, ResizeGrab},
};

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
    pub shm_state: ShmState,
    pub output_manager_state: OutputManagerState,
    pub seat_state: SeatState<Carbs>,
    pub data_device_state: DataDeviceState,
    pub seat: Seat<Carbs>,

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

        let compositor_state = CompositorState::new::<Self>(&dh);
        let xdg_shell_state = XdgShellState::new::<Self>(&dh);
        let shm_state = ShmState::new::<Self>(&dh, vec![]);
        let output_manager_state = OutputManagerState::new_with_xdg_output::<Self>(&dh);
        let data_device_state = DataDeviceState::new::<Self>(&dh);

        let mut seat_state = SeatState::new();
        let mut seat: Seat<Self> = seat_state.new_wl_seat(&dh, "carbs");
        seat.add_keyboard(Default::default(), 200, 25).expect("keyboard (is xkbcommon working?)");
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
            shm_state,
            output_manager_state,
            seat_state,
            data_device_state,
            seat,
            cursor_status: CursorImageStatus::default_named(),
            cursor_arrow: Arrow::new(cursor::CURSOR_SIZE),
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
