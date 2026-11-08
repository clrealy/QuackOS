use smithay::{
    backend::renderer::utils::on_commit_buffer_handler,
    delegate_compositor, delegate_data_device, delegate_output, delegate_primary_selection, delegate_seat,
    delegate_shm, delegate_xdg_decoration, delegate_xdg_shell,
    desktop::{PopupKind, Window},
    input::{pointer::CursorImageStatus, Seat, SeatHandler, SeatState},
    output::Output,
    reexports::{
        wayland_protocols::xdg::{
            decoration::zv1::server::zxdg_toplevel_decoration_v1::Mode as DecorationMode,
            shell::server::xdg_toplevel,
        },
        wayland_server::{
                protocol::{wl_buffer, wl_output, wl_seat, wl_surface::WlSurface},
            Client,
        },
    },
    utils::{Serial, SERIAL_COUNTER},
    wayland::{
        buffer::BufferHandler,
        compositor::{is_sync_subsurface, with_states, CompositorClientState, CompositorHandler, CompositorState},
        output::OutputHandler,
        selection::{
            data_device::{ClientDndGrabHandler, DataDeviceHandler, DataDeviceState, ServerDndGrabHandler},
            primary_selection::{PrimarySelectionHandler, PrimarySelectionState},
            SelectionHandler,
        },
        shell::xdg::{
            decoration::XdgDecorationHandler,
            PopupSurface, PositionerState, ToplevelSurface, XdgShellHandler, XdgShellState, XdgToplevelSurfaceData,
        },
        shm::{ShmHandler, ShmState},
    },
};

use crate::{
    grabs,
    state::{Carbs, ClientState},
};

// ---------------------------------------------------------------- compositor / shm

impl CompositorHandler for Carbs {
    fn compositor_state(&mut self) -> &mut CompositorState {
        &mut self.compositor_state
    }

    fn client_compositor_state<'a>(&self, client: &'a Client) -> &'a CompositorClientState {
        &client.get_data::<ClientState>().unwrap().compositor_state
    }

    fn commit(&mut self, surface: &WlSurface) {
        on_commit_buffer_handler::<Self>(surface);

        if !is_sync_subsurface(surface) {
            let root = Carbs::root_surface(surface);
            if let Some(window) = self.window_for_surface(&root) {
                window.on_commit();
                // Mid-resize the client picks its own size; if we're dragging the top or
                // left edge the window has to slide so the opposite edge stays put.
                if let Some(location) = grabs::resize_location(&root, window.geometry().size) {
                    self.space.map_element(window, location, false);
                }
            }
        }

        self.handle_xdg_commit(surface);
    }
}
delegate_compositor!(Carbs);

impl BufferHandler for Carbs {
    fn buffer_destroyed(&mut self, _buffer: &wl_buffer::WlBuffer) {}
}

impl ShmHandler for Carbs {
    fn shm_state(&self) -> &ShmState {
        &self.shm_state
    }
}
delegate_shm!(Carbs);

// ---------------------------------------------------------------- xdg-shell (app windows)

impl XdgShellHandler for Carbs {
    fn xdg_shell_state(&mut self) -> &mut XdgShellState {
        &mut self.xdg_shell_state
    }

    fn new_toplevel(&mut self, surface: ToplevelSurface) {
        // cascade new windows so they don't stack exactly on top of each other
        let n = self.space.elements().count() as i32;
        let offset = 48 + (n % 8) * 32;
        let window = Window::new_wayland_window(surface);
        self.space.map_element(window.clone(), (offset, offset), true);
        self.focus_window(&window);
    }

    fn new_popup(&mut self, surface: PopupSurface, _positioner: PositionerState) {
        let _ = self.popups.track_popup(PopupKind::Xdg(surface));
    }

    fn grab(&mut self, _surface: PopupSurface, _seat: wl_seat::WlSeat, _serial: Serial) {}

    /// The client wants to be dragged (its own titlebar, usually).
    fn move_request(&mut self, surface: ToplevelSurface, _seat: wl_seat::WlSeat, serial: Serial) {
        let Some(start_data) = self.client_grab_start_data(surface.wl_surface(), serial) else {
            return;
        };
        let Some(window) = self.window_for_surface(surface.wl_surface()) else {
            return;
        };
        self.start_move(&window, start_data, serial);
    }

    /// The client wants to be resized (dragging its own frame edge).
    fn resize_request(
        &mut self,
        surface: ToplevelSurface,
        _seat: wl_seat::WlSeat,
        serial: Serial,
        edges: xdg_toplevel::ResizeEdge,
    ) {
        let Some(start_data) = self.client_grab_start_data(surface.wl_surface(), serial) else {
            return;
        };
        let Some(window) = self.window_for_surface(surface.wl_surface()) else {
            return;
        };
        self.start_resize(&window, edges.into(), start_data, serial);
    }

    fn maximize_request(&mut self, surface: ToplevelSurface) {
        if let Some(window) = self.window_for_surface(surface.wl_surface()) {
            self.set_maximized(&window, true);
        }
    }

    fn unmaximize_request(&mut self, surface: ToplevelSurface) {
        if let Some(window) = self.window_for_surface(surface.wl_surface()) {
            self.set_maximized(&window, false);
        }
    }

    fn fullscreen_request(&mut self, surface: ToplevelSurface, output: Option<wl_output::WlOutput>) {
        if let Some(window) = self.window_for_surface(surface.wl_surface()) {
            let output = output.as_ref().and_then(Output::from_resource);
            self.set_fullscreen(&window, true, output);
        }
    }

    fn unfullscreen_request(&mut self, surface: ToplevelSurface) {
        if let Some(window) = self.window_for_surface(surface.wl_surface()) {
            self.set_fullscreen(&window, false, None);
        }
    }

    /// The window is gone: take it off the space and hand focus to whatever is on top now.
    fn toplevel_destroyed(&mut self, surface: ToplevelSurface) {
        if let Some(window) = self.window_for_surface(surface.wl_surface()) {
            self.space.unmap_elem(&window);
        }

        match self.space.elements().last().cloned() {
            Some(next) => self.focus_window(&next),
            None => {
                if let Some(keyboard) = self.seat.get_keyboard() {
                    let serial = SERIAL_COUNTER.next_serial();
                    keyboard.set_focus(self, None, serial);
                }
            }
        }
    }

    fn reposition_request(&mut self, surface: PopupSurface, positioner: PositionerState, token: u32) {
        surface.with_pending_state(|state| {
            state.geometry = positioner.get_geometry();
            state.positioner = positioner;
        });
        surface.send_repositioned(token);
    }
}
delegate_xdg_shell!(Carbs);

impl Carbs {
    /// Sends the first configure for new toplevels/popups (required before they can show up).
    fn handle_xdg_commit(&mut self, surface: &WlSurface) {
        if let Some(window) = self.window_for_surface(surface) {
            let initial_configure_sent = with_states(surface, |states| {
                states.data_map.get::<XdgToplevelSurfaceData>().unwrap().lock().unwrap().initial_configure_sent
            });
            if !initial_configure_sent {
                window.toplevel().unwrap().send_configure();
            }
        }

        if let Some(PopupKind::Xdg(ref popup)) = self.popups.find_popup(surface) {
            if !popup.is_initial_configure_sent() {
                let _ = popup.send_configure();
            }
        }
    }
}

// ---------------------------------------------------------------- seat / clipboard / output

impl SeatHandler for Carbs {
    type KeyboardFocus = WlSurface;
    type PointerFocus = WlSurface;
    type TouchFocus = WlSurface;

    fn seat_state(&mut self) -> &mut SeatState<Carbs> {
        &mut self.seat_state
    }

    fn focus_changed(&mut self, _seat: &Seat<Self>, _focused: Option<&WlSurface>) {}

    /// A client set the pointer image (or asked for it to be hidden).
    fn cursor_image(&mut self, _seat: &Seat<Self>, image: CursorImageStatus) {
        self.cursor_status = image;
    }
}
delegate_seat!(Carbs);

impl SelectionHandler for Carbs {
    type SelectionUserData = ();
}

impl DataDeviceHandler for Carbs {
    fn data_device_state(&self) -> &DataDeviceState {
        &self.data_device_state
    }
}
impl ClientDndGrabHandler for Carbs {}
impl ServerDndGrabHandler for Carbs {}
delegate_data_device!(Carbs);

impl OutputHandler for Carbs {}
delegate_output!(Carbs);

impl PrimarySelectionHandler for Carbs {
    fn primary_selection_state(&self) -> &PrimarySelectionState {
        &self.primary_selection_state
    }
}
delegate_primary_selection!(Carbs);

// ---------------------------------------------------------------- decorations

/// Carbs draws no window frames, so every toplevel is told to draw its own.
/// Without this protocol some toolkits come up with no titlebar and no way to
/// move or close the window.
impl XdgDecorationHandler for Carbs {
    fn new_decoration(&mut self, toplevel: ToplevelSurface) {
        self.set_client_side_decorations(&toplevel);
    }

    fn request_mode(&mut self, toplevel: ToplevelSurface, mode: DecorationMode) {
        if mode == DecorationMode::ServerSide {
            tracing::debug!("a client asked for server-side decorations; Carbs has none yet");
        }
        self.set_client_side_decorations(&toplevel);
    }

    fn unset_mode(&mut self, toplevel: ToplevelSurface) {
        self.set_client_side_decorations(&toplevel);
    }
}
delegate_xdg_decoration!(Carbs);

impl Carbs {
    fn set_client_side_decorations(&mut self, toplevel: &ToplevelSurface) {
        toplevel.with_pending_state(|state| {
            state.decoration_mode = Some(DecorationMode::ClientSide);
        });
        // Before the first configure the client isn't listening yet; the initial
        // configure in handle_xdg_commit will carry this.
        if toplevel.is_initial_configure_sent() {
            toplevel.send_pending_configure();
        }
    }
}
