use smithay::{
    backend::input::{
        AbsolutePositionEvent, Axis, AxisSource, ButtonState, Event, InputBackend, InputEvent, KeyState,
        KeyboardKeyEvent, PointerAxisEvent, PointerButtonEvent,
    },
    desktop::Window,
    input::{
        keyboard::FilterResult,
        pointer::{AxisFrame, ButtonEvent, CursorImageStatus, GrabStartData, MotionEvent},
    },
    utils::SERIAL_COUNTER,
};

use crate::{grabs, shell, state::Carbs};

/// Linux button codes (`linux/input-event-codes.h`).
const BTN_LEFT: u32 = 0x110;
const BTN_RIGHT: u32 = 0x111;

impl Carbs {
    /// Gives keyboard focus to `window`, raises it, and updates the active (activated) state of every window.
    pub fn focus_window(&mut self, window: &Window) {
        self.space.raise_element(window, true);
        let serial = SERIAL_COUNTER.next_serial();
        let keyboard = self.seat.get_keyboard().unwrap();
        let surface = window.toplevel().map(|t| t.wl_surface().clone());
        keyboard.set_focus(self, surface, serial);

        let focused = window.clone();
        for w in self.space.elements() {
            let active = *w == focused;
            if let Some(t) = w.toplevel() {
                t.with_pending_state(|s| {
                    if active {
                        s.states.set(smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::State::Activated);
                    } else {
                        s.states.unset(smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::State::Activated);
                    }
                });
                t.send_pending_configure();
            }
        }
    }

    pub fn process_input_event<I: InputBackend>(&mut self, event: InputEvent<I>) {
        match event {
            InputEvent::Keyboard { event, .. } => {
                let serial = SERIAL_COUNTER.next_serial();
                let time = Event::time_msec(&event);
                let pressed = event.state() == KeyState::Pressed;
                let Some(keyboard) = self.seat.get_keyboard() else { return };

                // Compositor shortcuts are intercepted here, so the focused client
                // never sees them. See shell::action_for for the table.
                let action =
                    keyboard.input(self, event.key_code(), event.state(), serial, time, |state, mods, handle| {
                        if !pressed {
                            return FilterResult::Forward;
                        }
                        match shell::action_for(mods, handle.modified_sym(), &state.config) {
                            Some(action) => FilterResult::Intercept(action),
                            None => FilterResult::Forward,
                        }
                    });

                if let Some(action) = action {
                    self.run_action(action);
                }
            }

            InputEvent::PointerMotionAbsolute { event, .. } => {
                let Some(output) = self.space.outputs().next().cloned() else { return };
                let Some(geo) = self.space.output_geometry(&output) else { return };
                let pos = event.position_transformed(geo.size) + geo.loc.to_f64();
                let serial = SERIAL_COUNTER.next_serial();
                let under = self.surface_under(pos);

                // A surface's cursor image is undefined until that surface sets one,
                // so going back to the arrow on every crossing is correct - otherwise
                // a text I-beam would follow the pointer out of the text field.
                let focus = under.as_ref().map(|(surface, _)| surface.clone());
                if focus != self.pointer_focus {
                    self.pointer_focus = focus;
                    self.cursor_status = CursorImageStatus::default_named();
                }

                let pointer = self.seat.get_pointer().unwrap();
                pointer.motion(self, under, &MotionEvent { location: pos, serial, time: event.time_msec() });
                pointer.frame(self);
            }

            InputEvent::PointerButton { event, .. } => {
                let pointer = self.seat.get_pointer().unwrap();
                let serial = SERIAL_COUNTER.next_serial();
                let button = event.button_code();
                let state = event.state();

                if state == ButtonState::Pressed && !pointer.is_grabbed() {
                    let hit = self.space.element_under(pointer.current_location()).map(|(w, _)| w.clone());
                    if let Some(window) = hit {
                        self.focus_window(&window);

                        // Carbs draws no decorations yet, so Super+drag is how you move
                        // and resize windows that don't draw their own frame.
                        // Super+left  = move, Super+right = resize from the nearest corner.
                        let logo = self.seat.get_keyboard().is_some_and(|k| k.modifier_state().logo);
                        if logo && (button == BTN_LEFT || button == BTN_RIGHT) {
                            let location = pointer.current_location();
                            // focus: None - the client is not part of this drag, so it
                            // gets no press and no release for it.
                            let start_data = GrabStartData { focus: None, button, location };

                            if button == BTN_LEFT {
                                self.start_move(&window, start_data, serial);
                            } else if let Some(rect) = self.space.element_geometry(&window) {
                                let edges = grabs::edges_under_pointer(rect, location);
                                self.start_resize(&window, edges, start_data, serial);
                            }
                            return;
                        }
                    } else if let Some(kb) = self.seat.get_keyboard() {
                        kb.set_focus(self, None, serial);
                    }
                }

                pointer.button(self, &ButtonEvent { button, state, serial, time: event.time_msec() });
                pointer.frame(self);
            }

            InputEvent::PointerAxis { event, .. } => {
                let source = event.source();
                let amount = |axis, default: f64| {
                    event.amount(axis).unwrap_or_else(|| event.amount_v120(axis).unwrap_or(0.0) * default / 120.0)
                };
                let (h, v) = (amount(Axis::Horizontal, 3.0), amount(Axis::Vertical, 3.0));

                let mut frame = AxisFrame::new(event.time_msec()).source(source);
                if h != 0.0 {
                    frame = frame.value(Axis::Horizontal, h);
                    if let Some(d) = event.amount_v120(Axis::Horizontal) {
                        frame = frame.v120(Axis::Horizontal, d as i32);
                    }
                }
                if v != 0.0 {
                    frame = frame.value(Axis::Vertical, v);
                    if let Some(d) = event.amount_v120(Axis::Vertical) {
                        frame = frame.v120(Axis::Vertical, d as i32);
                    }
                }
                if source == AxisSource::Finger {
                    if event.amount(Axis::Horizontal) == Some(0.0) {
                        frame = frame.stop(Axis::Horizontal);
                    }
                    if event.amount(Axis::Vertical) == Some(0.0) {
                        frame = frame.stop(Axis::Vertical);
                    }
                }
                let pointer = self.seat.get_pointer().unwrap();
                pointer.axis(self, frame);
                pointer.frame(self);
            }

            _ => {}
        }
    }
}
