use smithay::{
    backend::renderer::utils::on_commit_buffer_handler,
    delegate_compositor, delegate_data_device, delegate_output, delegate_seat, delegate_shm, delegate_xdg_shell,
    desktop::{PopupKind, Window},
    input::{pointer::CursorImageStatus, Seat, SeatHandler, SeatState},
    reexports::wayland_server::{
        protocol::{wl_buffer, wl_seat, wl_surface::WlSurface},
        Client,
    },
    utils::Serial,
    wayland::{
        buffer::BufferHandler,
        compositor::{get_parent, is_sync_subsurface, with_states, CompositorClientState, CompositorHandler, CompositorState},
        output::OutputHandler,
        selection::{
            data_device::{ClientDndGrabHandler, DataDeviceHandler, DataDeviceState, ServerDndGrabHandler},
            SelectionHandler,
        },
        shell::xdg::{
            PopupSurface, PositionerState, ToplevelSurface, XdgShellHandler, XdgShellState, XdgToplevelSurfaceData,
        },
        shm::{ShmHandler, ShmState},
    },
};

use crate::state::{Carbs, ClientState};

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
            let mut root = surface.clone();
            while let Some(parent) = get_parent(&root) {
                root = parent;
            }
            if let Some(window) = self.space.elements().find(|w| w.toplevel().map(|t| t.wl_surface()) == Some(&root)) {
                window.on_commit();
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
        if let Some(window) = self.space.elements().find(|w| w.toplevel().map(|t| t.wl_surface()) == Some(surface)).cloned() {
            let initial_configure_sent = with_states(surface, |states| {
                states.data_map.get::<XdgToplevelSurfaceData>().unwrap().lock().unwrap().initial_configure_sent
            });
            if !initial_configure_sent {
                window.toplevel().unwrap().send_configure();
            }
        }

        if let Some(popup) = self.popups.find_popup(surface) {
            if let PopupKind::Xdg(ref popup) = popup {
                if !popup.is_initial_configure_sent() {
                    let _ = popup.send_configure();
                }
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
    fn cursor_image(&mut self, _seat: &Seat<Self>, _image: CursorImageStatus) {}
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
