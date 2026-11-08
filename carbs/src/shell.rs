//! Window management: where windows go, how big they get, which one is in front,
//! and what the keyboard shortcuts do.

use std::{cell::RefCell, process::Stdio};

use smithay::{
    desktop::Window,
    input::keyboard::{keysyms, Keysym, ModifiersState},
    output::Output,
    reexports::{
        wayland_protocols::xdg::shell::server::xdg_toplevel, wayland_server::Resource,
    },
    utils::{Logical, Point, Rectangle, Size},
};

use crate::{config::Config, state::Carbs};

/// How much of a window has to stay inside the work area when it's dragged, so it
/// can always be grabbed again. There is no dock or alt-tab fallback that can
/// rescue a window dragged completely off-screen.
pub const KEEP_VISIBLE: i32 = 72;

/// Where a window sat before it was maximised or made fullscreen.
#[derive(Default)]
struct RestoreGeometry(RefCell<Option<Rectangle<i32, Logical>>>);

// ---------------------------------------------------------------- keybindings

/// What a shortcut does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Ask the focused window to close.
    Close,
    ToggleMaximize,
    ToggleFullscreen,
    /// Focus the next window; repeated presses rotate through all of them.
    CycleForward,
    /// The exact inverse of [`Action::CycleForward`].
    CycleBackward,
    /// Run a command.
    Spawn(String),
    /// Shut Carbs down and end the session.
    Quit,
}

/// Maps a key press to an action, or `None` to pass it to the focused client.
///
/// A pure function so the whole shortcut table can be tested without a compositor.
pub fn action_for(mods: &ModifiersState, keysym: Keysym, config: &Config) -> Option<Action> {
    let sym = keysym.raw();

    // Alt+Tab / Super+Tab. xkb turns Shift+Tab into ISO_Left_Tab, so match both.
    if sym == keysyms::KEY_Tab || sym == keysyms::KEY_ISO_Left_Tab {
        if mods.alt || mods.logo {
            return Some(if mods.shift { Action::CycleBackward } else { Action::CycleForward });
        }
        return None;
    }

    if !mods.logo {
        return None;
    }

    match sym {
        keysyms::KEY_q | keysyms::KEY_Q => Some(Action::Close),
        keysyms::KEY_Up | keysyms::KEY_KP_Up => Some(Action::ToggleMaximize),
        keysyms::KEY_f | keysyms::KEY_F => Some(Action::ToggleFullscreen),
        keysyms::KEY_Return | keysyms::KEY_KP_Enter => {
            Some(Action::Spawn(config.terminal.clone()))
        }
        keysyms::KEY_d | keysyms::KEY_D => Some(Action::Spawn(config.launcher.clone())),
        // Deliberately awkward: this ends the session.
        keysyms::KEY_e | keysyms::KEY_E if mods.shift => Some(Action::Quit),
        _ => None,
    }
}

/// Runs a command detached from Carbs.
///
/// Goes through `sh -c 'cmd &'` so that shell syntax in the config works, and so
/// the app is reparented to init - Carbs never collects a zombie. The `sh` itself
/// exits immediately and is reaped here.
pub fn spawn(command: &str) {
    let command = command.trim();
    if command.is_empty() {
        tracing::warn!("nothing configured to run for that shortcut");
        return;
    }
    tracing::info!("spawning: {command}");
    let child = std::process::Command::new("/bin/sh")
        .arg("-c")
        .arg(format!("{command} &"))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();

    match child {
        // Returns as soon as the shell has forked the real command away.
        Ok(mut shell) => {
            if let Err(err) = shell.wait() {
                tracing::warn!("couldn't reap the shell for {command:?}: {err}");
            }
        }
        Err(err) => tracing::warn!("couldn't run {command:?}: {err}"),
    }
}

// ---------------------------------------------------------------- geometry helpers

/// Clamps a window position so at least [`KEEP_VISIBLE`] pixels stay reachable
/// inside `area`, and the top edge never goes above it.
///
/// Free function so it can be tested directly.
pub fn clamp_to_area(
    area: Rectangle<i32, Logical>,
    size: Size<i32, Logical>,
    location: Point<i32, Logical>,
) -> Point<i32, Logical> {
    let keep_x = KEEP_VISIBLE.min(size.w.max(1));
    let keep_y = KEEP_VISIBLE.min(size.h.max(1));

    let min_x = area.loc.x - (size.w - keep_x).max(0);
    let max_x = area.loc.x + area.size.w - keep_x;
    // Never above the work area: the top edge is the only reliable drag handle.
    let min_y = area.loc.y;
    let max_y = area.loc.y + area.size.h - keep_y;

    Point::from((
        clamp(location.x, min_x, max_x),
        clamp(location.y, min_y, max_y),
    ))
}

/// `i32::clamp` panics when min > max, which a silly output size could cause.
fn clamp(value: i32, min: i32, max: i32) -> i32 {
    if min >= max {
        min
    } else {
        value.clamp(min, max)
    }
}

impl Carbs {
    /// The output a window should be treated as living on: whichever one its
    /// geometry overlaps most, falling back to the first output.
    pub fn output_for_window(&self, window: &Window) -> Option<Output> {
        let geometry = self.space.element_geometry(window);
        let best = geometry.and_then(|geometry| {
            self.space
                .outputs()
                .filter_map(|output| {
                    let overlap = self.space.output_geometry(output)?.intersection(geometry)?;
                    Some((output.clone(), overlap.size.w as i64 * overlap.size.h as i64))
                })
                .max_by_key(|(_, area)| *area)
                .map(|(output, _)| output)
        });
        best.or_else(|| self.space.outputs().next().cloned())
    }

    /// The area windows may use on an output: its geometry, minus anything
    /// layer-shell clients have reserved (a top bar, a dock).
    pub fn work_area(&self, output: &Output) -> Rectangle<i32, Logical> {
        let geometry = self
            .space
            .output_geometry(output)
            .unwrap_or_else(|| Rectangle::new(Point::from((0, 0)), Size::from((1280, 720))));
        self.subtract_exclusive_zones(output, geometry)
    }

    /// The work area of the output a window is on.
    pub fn work_area_for_window(&self, window: &Window) -> Option<Rectangle<i32, Logical>> {
        self.output_for_window(window).map(|output| self.work_area(&output))
    }

    /// Keeps a window position reachable on its own output.
    pub fn clamp_location(
        &self,
        window: &Window,
        location: Point<i32, Logical>,
    ) -> Point<i32, Logical> {
        match self.work_area_for_window(window) {
            Some(area) => clamp_to_area(area, window.geometry().size, location),
            None => location,
        }
    }

    // ------------------------------------------------------------ saved geometry

    /// Remembers where a window is, unless something is already remembered.
    fn remember_geometry(&self, window: &Window) {
        window.user_data().insert_if_missing(RestoreGeometry::default);
        let Some(store) = window.user_data().get::<RestoreGeometry>() else {
            return;
        };
        let mut slot = store.0.borrow_mut();
        if slot.is_none() {
            if let Some(location) = self.space.element_location(window) {
                *slot = Some(Rectangle::new(location, window.geometry().size));
            }
        }
    }

    /// Takes back the remembered geometry, clearing it.
    fn forget_geometry(&self, window: &Window) -> Option<Rectangle<i32, Logical>> {
        window.user_data().get::<RestoreGeometry>()?.0.borrow_mut().take()
    }

    /// Is this window currently maximised or fullscreen?
    pub fn is_tiled(&self, window: &Window) -> bool {
        window.toplevel().is_some_and(|toplevel| {
            let state = toplevel.current_state();
            state.states.contains(xdg_toplevel::State::Maximized)
                || state.states.contains(xdg_toplevel::State::Fullscreen)
        })
    }

    // ------------------------------------------------------------ maximize

    pub fn set_maximized(&mut self, window: &Window, maximized: bool) {
        let Some(toplevel) = window.toplevel().cloned() else {
            return;
        };

        if maximized {
            let Some(area) = self.work_area_for_window(window) else {
                return;
            };
            self.remember_geometry(window);
            toplevel.with_pending_state(|state| {
                state.states.set(xdg_toplevel::State::Maximized);
                state.size = Some(area.size);
            });
            toplevel.send_pending_configure();
            self.space.map_element(window.clone(), area.loc, true);
        } else {
            let restore = self.forget_geometry(window);
            toplevel.with_pending_state(|state| {
                state.states.unset(xdg_toplevel::State::Maximized);
                // None means "you choose", which is what we want with nothing saved.
                state.size = restore.map(|rect| rect.size);
            });
            toplevel.send_pending_configure();
            if let Some(rect) = restore {
                self.space.map_element(window.clone(), rect.loc, true);
            }
        }
    }

    /// Takes a window out of maximised *and* fullscreen in one go, back to the
    /// geometry it had before. Returns that geometry.
    ///
    /// Separate from the two setters because unsetting both needs to consume the
    /// one saved rectangle exactly once.
    pub fn restore_floating(&mut self, window: &Window) -> Option<Rectangle<i32, Logical>> {
        let toplevel = window.toplevel().cloned()?;
        let restore = self.forget_geometry(window);
        toplevel.with_pending_state(|state| {
            state.states.unset(xdg_toplevel::State::Maximized);
            state.states.unset(xdg_toplevel::State::Fullscreen);
            state.fullscreen_output = None;
            state.size = restore.map(|rect| rect.size);
        });
        toplevel.send_pending_configure();
        if let Some(rect) = restore {
            self.space.map_element(window.clone(), rect.loc, true);
        }
        restore
    }

    pub fn toggle_maximized(&mut self, window: &Window) {
        let maximized = window
            .toplevel()
            .is_some_and(|t| t.current_state().states.contains(xdg_toplevel::State::Maximized));
        self.set_maximized(window, !maximized);
    }

    // ------------------------------------------------------------ fullscreen

    pub fn set_fullscreen(&mut self, window: &Window, fullscreen: bool, output: Option<Output>) {
        let Some(toplevel) = window.toplevel().cloned() else {
            return;
        };

        if fullscreen {
            let Some(output) = output.or_else(|| self.output_for_window(window)) else {
                return;
            };
            let Some(geometry) = self.space.output_geometry(&output) else {
                return;
            };
            self.remember_geometry(window);
            // Fullscreen covers the whole output, bars included. The client needs
            // its own wl_output handle, not ours.
            let wl_output = toplevel
                .wl_surface()
                .client()
                .and_then(|client| output.client_outputs(&client).next());
            toplevel.with_pending_state(|state| {
                state.states.set(xdg_toplevel::State::Fullscreen);
                state.size = Some(geometry.size);
                state.fullscreen_output = wl_output;
            });
            toplevel.send_pending_configure();
            self.space.map_element(window.clone(), geometry.loc, true);
        } else {
            let restore = self.forget_geometry(window);
            toplevel.with_pending_state(|state| {
                state.states.unset(xdg_toplevel::State::Fullscreen);
                state.size = restore.map(|rect| rect.size);
                state.fullscreen_output = None;
            });
            toplevel.send_pending_configure();
            if let Some(rect) = restore {
                self.space.map_element(window.clone(), rect.loc, true);
            }
        }
    }

    pub fn toggle_fullscreen(&mut self, window: &Window) {
        let fullscreen = window
            .toplevel()
            .is_some_and(|t| t.current_state().states.contains(xdg_toplevel::State::Fullscreen));
        self.set_fullscreen(window, !fullscreen, None);
    }

    // ------------------------------------------------------------ stacking

    /// Sends a window to the bottom of the stack by raising everything else.
    pub fn lower_to_bottom(&mut self, window: &Window) {
        let others: Vec<Window> =
            self.space.elements().filter(|other| *other != window).cloned().collect();
        for other in others {
            self.space.raise_element(&other, false);
        }
    }

    /// Alt+Tab. Forward raises the bottom window, so repeated presses rotate
    /// through every window rather than flipping between two.
    pub fn cycle_focus(&mut self, forward: bool) {
        if self.space.elements().count() < 2 {
            let only = self.space.elements().last().cloned();
            if let Some(only) = only {
                self.focus_window(&only);
            }
            return;
        }

        if forward {
            let bottom = self.space.elements().next().cloned();
            if let Some(bottom) = bottom {
                self.focus_window(&bottom);
            }
        } else {
            // Exactly undoes a forward cycle.
            let top = self.space.elements().last().cloned();
            if let Some(top) = top {
                self.lower_to_bottom(&top);
            }
            let new_top = self.space.elements().last().cloned();
            if let Some(new_top) = new_top {
                self.focus_window(&new_top);
            }
        }
    }

    // ------------------------------------------------------------ actions

    /// Runs a shortcut.
    pub fn run_action(&mut self, action: Action) {
        match action {
            Action::Close => self.close_focused_window(),
            Action::ToggleMaximize => {
                if let Some(window) = self.focused_window() {
                    self.toggle_maximized(&window);
                }
            }
            Action::ToggleFullscreen => {
                if let Some(window) = self.focused_window() {
                    self.toggle_fullscreen(&window);
                }
            }
            Action::CycleForward => self.cycle_focus(true),
            Action::CycleBackward => self.cycle_focus(false),
            Action::Spawn(command) => spawn(&command),
            Action::Quit => {
                tracing::info!("Super+Shift+E: ending the session");
                self.loop_signal.stop();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn area() -> Rectangle<i32, Logical> {
        Rectangle::new(Point::from((0, 0)), Size::from((1920, 1080)))
    }

    fn mods(logo: bool, alt: bool, shift: bool) -> ModifiersState {
        ModifiersState { logo, alt, shift, ..Default::default() }
    }

    // ---------------------------------------------------------------- clamping

    #[test]
    fn a_window_inside_the_area_is_left_alone() {
        let size = Size::from((800, 600));
        let point = Point::from((300, 200));
        assert_eq!(clamp_to_area(area(), size, point), point);
    }

    #[test]
    fn a_window_cannot_be_dragged_off_the_right_edge() {
        let size = Size::from((800, 600));
        let placed = clamp_to_area(area(), size, Point::from((5000, 100)));
        assert_eq!(placed.x, 1920 - KEEP_VISIBLE);
        assert!(placed.x < 1920, "some of it must stay on screen");
    }

    #[test]
    fn a_window_cannot_be_dragged_off_the_left_edge() {
        let size = Size::from((800, 600));
        let placed = clamp_to_area(area(), size, Point::from((-5000, 100)));
        assert_eq!(placed.x, -(800 - KEEP_VISIBLE));
        assert!(placed.x + 800 >= KEEP_VISIBLE, "some of it must stay on screen");
    }

    #[test]
    fn the_top_edge_stays_grabbable() {
        let size = Size::from((800, 600));
        assert_eq!(clamp_to_area(area(), size, Point::from((100, -400))).y, 0);
        let bottom = clamp_to_area(area(), size, Point::from((100, 99999))).y;
        assert_eq!(bottom, 1080 - KEEP_VISIBLE);
    }

    #[test]
    fn clamping_a_window_bigger_than_the_screen_does_not_panic() {
        let huge = Size::from((9000, 9000));
        let placed = clamp_to_area(area(), huge, Point::from((0, 0)));
        assert!(placed.x <= 0 && placed.y >= 0);
        // And a silly, tiny work area.
        let tiny = Rectangle::new(Point::from((0, 0)), Size::from((1, 1)));
        let _ = clamp_to_area(tiny, huge, Point::from((-10000, 10000)));
        let _ = clamp_to_area(tiny, Size::from((0, 0)), Point::from((5, 5)));
    }

    #[test]
    fn a_window_smaller_than_the_margin_still_stays_fully_on_screen() {
        let small = Size::from((20, 20));
        let placed = clamp_to_area(area(), small, Point::from((99999, 99999)));
        assert!(placed.x + 20 <= 1920 && placed.y + 20 <= 1080, "{placed:?}");
    }

    // ---------------------------------------------------------------- keybindings

    #[test]
    fn super_shortcuts() {
        let config = Config { terminal: "foot".into(), launcher: "wofi".into(), ..Config::default() };
        let s = mods(true, false, false);
        assert_eq!(action_for(&s, Keysym::from(keysyms::KEY_q), &config), Some(Action::Close));
        assert_eq!(action_for(&s, Keysym::from(keysyms::KEY_Up), &config), Some(Action::ToggleMaximize));
        assert_eq!(action_for(&s, Keysym::from(keysyms::KEY_f), &config), Some(Action::ToggleFullscreen));
        assert_eq!(
            action_for(&s, Keysym::from(keysyms::KEY_Return), &config),
            Some(Action::Spawn("foot".into()))
        );
        assert_eq!(
            action_for(&s, Keysym::from(keysyms::KEY_d), &config),
            Some(Action::Spawn("wofi".into()))
        );
    }

    #[test]
    fn shifted_letters_still_match() {
        // Super+Shift+Q arrives as 'Q', not 'q'.
        let config = Config::default();
        let s = mods(true, false, true);
        assert_eq!(action_for(&s, Keysym::from(keysyms::KEY_Q), &config), Some(Action::Close));
    }

    #[test]
    fn quit_needs_super_and_shift() {
        let config = Config::default();
        assert_eq!(
            action_for(&mods(true, false, true), Keysym::from(keysyms::KEY_E), &config),
            Some(Action::Quit)
        );
        // Super+E alone must not end the session.
        assert_eq!(action_for(&mods(true, false, false), Keysym::from(keysyms::KEY_e), &config), None);
    }

    #[test]
    fn tab_cycles_with_either_alt_or_super() {
        let config = Config::default();
        let tab = Keysym::from(keysyms::KEY_Tab);
        let back_tab = Keysym::from(keysyms::KEY_ISO_Left_Tab);
        assert_eq!(action_for(&mods(false, true, false), tab, &config), Some(Action::CycleForward));
        assert_eq!(action_for(&mods(true, false, false), tab, &config), Some(Action::CycleForward));
        assert_eq!(
            action_for(&mods(false, true, true), back_tab, &config),
            Some(Action::CycleBackward)
        );
        assert_eq!(
            action_for(&mods(false, true, true), tab, &config),
            Some(Action::CycleBackward)
        );
        // A bare Tab belongs to the application.
        assert_eq!(action_for(&mods(false, false, false), tab, &config), None);
    }

    #[test]
    fn unmodified_keys_go_to_the_app() {
        let config = Config::default();
        let plain = mods(false, false, false);
        for sym in [keysyms::KEY_q, keysyms::KEY_f, keysyms::KEY_d, keysyms::KEY_Return, keysyms::KEY_Up] {
            assert_eq!(action_for(&plain, Keysym::from(sym), &config), None, "sym {sym:#x}");
        }
        // Ctrl+Q is the app's own quit shortcut and must reach it.
        let ctrl = ModifiersState { ctrl: true, ..Default::default() };
        assert_eq!(action_for(&ctrl, Keysym::from(keysyms::KEY_q), &config), None);
    }
}
