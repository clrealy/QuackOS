//! Pointer grabs: interactive move and resize.
//!
//! Two ways in:
//!   * the client asks (`xdg_toplevel.move` / `.resize`) - that's what a CSD titlebar does;
//!   * Super + drag, so windows with no decorations can still be pushed around.
//!
//! Resizing from the top or left edge has to move the window as it shrinks, otherwise the
//! far edge would walk away. The client applies sizes asynchronously, so the position is
//! fixed up on commit - see [`ResizeSurfaceState`].

use std::cell::RefCell;

use smithay::{
    desktop::Window,
    input::pointer::{
        AxisFrame, ButtonEvent, GestureHoldBeginEvent, GestureHoldEndEvent, GesturePinchBeginEvent,
        GesturePinchEndEvent, GesturePinchUpdateEvent, GestureSwipeBeginEvent, GestureSwipeEndEvent,
        GestureSwipeUpdateEvent, GrabStartData, MotionEvent, PointerGrab, PointerInnerHandle,
        RelativeMotionEvent,
    },
    reexports::{
        wayland_protocols::xdg::shell::server::xdg_toplevel,
        wayland_server::protocol::wl_surface::WlSurface,
    },
    utils::{IsAlive, Logical, Point, Rectangle, Size},
    wayland::{compositor::with_states, shell::xdg::SurfaceCachedState},
};

use crate::state::Carbs;

/// Nothing may be resized smaller than this, whatever the client says.
const MIN_SIZE: i32 = 64;

// ---------------------------------------------------------------- edges

/// Which edges a resize is pulling on. Same bit values as `xdg_toplevel::ResizeEdge`,
/// but combinable (`TOP_LEFT` really is `TOP | LEFT`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ResizeEdge(u32);

impl ResizeEdge {
    pub const NONE: Self = Self(0);
    pub const TOP: Self = Self(1);
    pub const BOTTOM: Self = Self(2);
    pub const LEFT: Self = Self(4);
    pub const RIGHT: Self = Self(8);

    /// True if every bit of `other` is set here. `NONE` is contained in nothing.
    pub fn contains(self, other: Self) -> bool {
        other.0 != 0 && self.0 & other.0 == other.0
    }

    pub fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }
}

impl From<xdg_toplevel::ResizeEdge> for ResizeEdge {
    fn from(edge: xdg_toplevel::ResizeEdge) -> Self {
        Self(edge as u32)
    }
}

/// Picks edges from where the pointer sits inside the window: the nearer half on each
/// axis wins, so a drag near a corner resizes both ways.
pub fn edges_under_pointer(rect: Rectangle<i32, Logical>, pos: Point<f64, Logical>) -> ResizeEdge {
    let frac = |p: f64, loc: i32, len: i32| (p - loc as f64) / len.max(1) as f64;
    let horizontal = if frac(pos.x, rect.loc.x, rect.size.w) < 0.5 {
        ResizeEdge::LEFT
    } else {
        ResizeEdge::RIGHT
    };
    let vertical = if frac(pos.y, rect.loc.y, rect.size.h) < 0.5 {
        ResizeEdge::TOP
    } else {
        ResizeEdge::BOTTOM
    };
    horizontal.union(vertical)
}

// ---------------------------------------------------------------- resize bookkeeping

/// Lives in the toplevel surface's data map for the duration of a resize.
#[derive(Default)]
pub struct ResizeSurfaceState {
    data: Option<ResizeData>,
}

#[derive(Debug, Clone, Copy)]
struct ResizeData {
    edges: ResizeEdge,
    /// Where the window sat and how big it was when the grab started.
    initial_rect: Rectangle<i32, Logical>,
    /// Set when the grab ends: the next commit applies the final spot and clears this.
    finished: bool,
}

impl ResizeSurfaceState {
    fn with<F, T>(surface: &WlSurface, f: F) -> T
    where
        F: FnOnce(&mut Self) -> T,
    {
        with_states(surface, |states| {
            states.data_map.insert_if_missing(|| RefCell::new(Self::default()));
            let state = states.data_map.get::<RefCell<Self>>().unwrap();
            f(&mut state.borrow_mut())
        })
    }
}

/// Remember where the window started, so commits can keep the anchored edges still.
pub fn resize_started(surface: &WlSurface, edges: ResizeEdge, initial_rect: Rectangle<i32, Logical>) {
    ResizeSurfaceState::with(surface, |state| {
        state.data = Some(ResizeData { edges, initial_rect, finished: false });
    });
}

/// The grab is over; one more commit and we forget about it.
pub fn resize_finished(surface: &WlSurface) {
    ResizeSurfaceState::with(surface, |state| {
        if let Some(data) = state.data.as_mut() {
            data.finished = true;
        }
    });
}

/// Where the window should sit now that it has committed `new_size`.
/// `None` means this surface isn't mid-resize and should be left alone.
pub fn resize_location(surface: &WlSurface, new_size: Size<i32, Logical>) -> Option<Point<i32, Logical>> {
    ResizeSurfaceState::with(surface, |state| {
        let data = state.data?;
        let mut loc = data.initial_rect.loc;
        if data.edges.contains(ResizeEdge::LEFT) {
            loc.x = data.initial_rect.loc.x + (data.initial_rect.size.w - new_size.w);
        }
        if data.edges.contains(ResizeEdge::TOP) {
            loc.y = data.initial_rect.loc.y + (data.initial_rect.size.h - new_size.h);
        }
        if data.finished {
            state.data = None;
        }
        Some(loc)
    })
}

// ---------------------------------------------------------------- move

/// Drags a window around. The pointer keeps no focus while this runs, so the client
/// below doesn't see the drag at all.
pub struct MoveGrab {
    start_data: GrabStartData<Carbs>,
    window: Window,
    initial_location: Point<i32, Logical>,
}

impl MoveGrab {
    pub fn new(
        start_data: GrabStartData<Carbs>,
        window: Window,
        initial_location: Point<i32, Logical>,
    ) -> Self {
        Self { start_data, window, initial_location }
    }
}

impl PointerGrab<Carbs> for MoveGrab {
    fn motion(
        &mut self,
        data: &mut Carbs,
        handle: &mut PointerInnerHandle<'_, Carbs>,
        _focus: Option<(WlSurface, Point<f64, Logical>)>,
        event: &MotionEvent,
    ) {
        handle.motion(data, None, event);

        if !self.window.alive() {
            handle.unset_grab(self, data, event.serial, event.time, true);
            return;
        }

        let delta = event.location - self.start_data.location;
        let location = (self.initial_location.to_f64() + delta).to_i32_round();
        data.space.map_element(self.window.clone(), location, true);
    }

    fn relative_motion(
        &mut self,
        data: &mut Carbs,
        handle: &mut PointerInnerHandle<'_, Carbs>,
        _focus: Option<(WlSurface, Point<f64, Logical>)>,
        event: &RelativeMotionEvent,
    ) {
        handle.relative_motion(data, None, event);
    }

    fn button(&mut self, data: &mut Carbs, handle: &mut PointerInnerHandle<'_, Carbs>, event: &ButtonEvent) {
        handle.button(data, event);
        if !handle.current_pressed().contains(&self.start_data.button) {
            handle.unset_grab(self, data, event.serial, event.time, true);
        }
    }

    fn axis(&mut self, data: &mut Carbs, handle: &mut PointerInnerHandle<'_, Carbs>, details: AxisFrame) {
        handle.axis(data, details);
    }

    fn frame(&mut self, data: &mut Carbs, handle: &mut PointerInnerHandle<'_, Carbs>) {
        handle.frame(data);
    }

    fn gesture_swipe_begin(
        &mut self,
        data: &mut Carbs,
        handle: &mut PointerInnerHandle<'_, Carbs>,
        event: &GestureSwipeBeginEvent,
    ) {
        handle.gesture_swipe_begin(data, event);
    }

    fn gesture_swipe_update(
        &mut self,
        data: &mut Carbs,
        handle: &mut PointerInnerHandle<'_, Carbs>,
        event: &GestureSwipeUpdateEvent,
    ) {
        handle.gesture_swipe_update(data, event);
    }

    fn gesture_swipe_end(
        &mut self,
        data: &mut Carbs,
        handle: &mut PointerInnerHandle<'_, Carbs>,
        event: &GestureSwipeEndEvent,
    ) {
        handle.gesture_swipe_end(data, event);
    }

    fn gesture_pinch_begin(
        &mut self,
        data: &mut Carbs,
        handle: &mut PointerInnerHandle<'_, Carbs>,
        event: &GesturePinchBeginEvent,
    ) {
        handle.gesture_pinch_begin(data, event);
    }

    fn gesture_pinch_update(
        &mut self,
        data: &mut Carbs,
        handle: &mut PointerInnerHandle<'_, Carbs>,
        event: &GesturePinchUpdateEvent,
    ) {
        handle.gesture_pinch_update(data, event);
    }

    fn gesture_pinch_end(
        &mut self,
        data: &mut Carbs,
        handle: &mut PointerInnerHandle<'_, Carbs>,
        event: &GesturePinchEndEvent,
    ) {
        handle.gesture_pinch_end(data, event);
    }

    fn gesture_hold_begin(
        &mut self,
        data: &mut Carbs,
        handle: &mut PointerInnerHandle<'_, Carbs>,
        event: &GestureHoldBeginEvent,
    ) {
        handle.gesture_hold_begin(data, event);
    }

    fn gesture_hold_end(
        &mut self,
        data: &mut Carbs,
        handle: &mut PointerInnerHandle<'_, Carbs>,
        event: &GestureHoldEndEvent,
    ) {
        handle.gesture_hold_end(data, event);
    }

    fn start_data(&self) -> &GrabStartData<Carbs> {
        &self.start_data
    }

    fn unset(&mut self, _data: &mut Carbs) {}
}

// ---------------------------------------------------------------- resize

/// Resizes a window by configuring it on every motion. The client decides the real
/// size (it may clamp or round); commits fix up the position.
pub struct ResizeGrab {
    start_data: GrabStartData<Carbs>,
    window: Window,
    edges: ResizeEdge,
    initial_rect: Rectangle<i32, Logical>,
    last_size: Size<i32, Logical>,
}

impl ResizeGrab {
    pub fn new(
        start_data: GrabStartData<Carbs>,
        window: Window,
        edges: ResizeEdge,
        initial_rect: Rectangle<i32, Logical>,
    ) -> Self {
        Self { start_data, window, edges, initial_rect, last_size: initial_rect.size }
    }

    /// Clamps a wanted size to the client's min/max (0 on an axis = unconstrained)
    /// and to Carbs' own floor.
    fn clamp(&self, surface: &WlSurface, mut size: Size<i32, Logical>) -> Size<i32, Logical> {
        let (min, max) = with_states(surface, |states| {
            let mut cached = states.cached_state.get::<SurfaceCachedState>();
            let current = cached.current();
            (current.min_size, current.max_size)
        });

        size.w = size.w.max(min.w.max(MIN_SIZE));
        size.h = size.h.max(min.h.max(MIN_SIZE));
        if max.w > 0 {
            size.w = size.w.min(max.w.max(MIN_SIZE));
        }
        if max.h > 0 {
            size.h = size.h.min(max.h.max(MIN_SIZE));
        }
        size
    }
}

impl PointerGrab<Carbs> for ResizeGrab {
    fn motion(
        &mut self,
        data: &mut Carbs,
        handle: &mut PointerInnerHandle<'_, Carbs>,
        _focus: Option<(WlSurface, Point<f64, Logical>)>,
        event: &MotionEvent,
    ) {
        handle.motion(data, None, event);

        let Some(toplevel) = self.window.toplevel().cloned() else {
            handle.unset_grab(self, data, event.serial, event.time, true);
            return;
        };
        if !self.window.alive() {
            handle.unset_grab(self, data, event.serial, event.time, true);
            return;
        }

        let delta = event.location - self.start_data.location;
        let (dx, dy) = (delta.x.round() as i32, delta.y.round() as i32);

        let mut size = self.initial_rect.size;
        if self.edges.contains(ResizeEdge::LEFT) {
            size.w -= dx;
        } else if self.edges.contains(ResizeEdge::RIGHT) {
            size.w += dx;
        }
        if self.edges.contains(ResizeEdge::TOP) {
            size.h -= dy;
        } else if self.edges.contains(ResizeEdge::BOTTOM) {
            size.h += dy;
        }
        let size = self.clamp(toplevel.wl_surface(), size);

        if size == self.last_size {
            return;
        }
        self.last_size = size;

        toplevel.with_pending_state(|state| {
            state.states.set(xdg_toplevel::State::Resizing);
            state.size = Some(size);
        });
        toplevel.send_pending_configure();
    }

    fn relative_motion(
        &mut self,
        data: &mut Carbs,
        handle: &mut PointerInnerHandle<'_, Carbs>,
        _focus: Option<(WlSurface, Point<f64, Logical>)>,
        event: &RelativeMotionEvent,
    ) {
        handle.relative_motion(data, None, event);
    }

    fn button(&mut self, data: &mut Carbs, handle: &mut PointerInnerHandle<'_, Carbs>, event: &ButtonEvent) {
        handle.button(data, event);
        if !handle.current_pressed().contains(&self.start_data.button) {
            handle.unset_grab(self, data, event.serial, event.time, true);
        }
    }

    fn axis(&mut self, data: &mut Carbs, handle: &mut PointerInnerHandle<'_, Carbs>, details: AxisFrame) {
        handle.axis(data, details);
    }

    fn frame(&mut self, data: &mut Carbs, handle: &mut PointerInnerHandle<'_, Carbs>) {
        handle.frame(data);
    }

    fn gesture_swipe_begin(
        &mut self,
        data: &mut Carbs,
        handle: &mut PointerInnerHandle<'_, Carbs>,
        event: &GestureSwipeBeginEvent,
    ) {
        handle.gesture_swipe_begin(data, event);
    }

    fn gesture_swipe_update(
        &mut self,
        data: &mut Carbs,
        handle: &mut PointerInnerHandle<'_, Carbs>,
        event: &GestureSwipeUpdateEvent,
    ) {
        handle.gesture_swipe_update(data, event);
    }

    fn gesture_swipe_end(
        &mut self,
        data: &mut Carbs,
        handle: &mut PointerInnerHandle<'_, Carbs>,
        event: &GestureSwipeEndEvent,
    ) {
        handle.gesture_swipe_end(data, event);
    }

    fn gesture_pinch_begin(
        &mut self,
        data: &mut Carbs,
        handle: &mut PointerInnerHandle<'_, Carbs>,
        event: &GesturePinchBeginEvent,
    ) {
        handle.gesture_pinch_begin(data, event);
    }

    fn gesture_pinch_update(
        &mut self,
        data: &mut Carbs,
        handle: &mut PointerInnerHandle<'_, Carbs>,
        event: &GesturePinchUpdateEvent,
    ) {
        handle.gesture_pinch_update(data, event);
    }

    fn gesture_pinch_end(
        &mut self,
        data: &mut Carbs,
        handle: &mut PointerInnerHandle<'_, Carbs>,
        event: &GesturePinchEndEvent,
    ) {
        handle.gesture_pinch_end(data, event);
    }

    fn gesture_hold_begin(
        &mut self,
        data: &mut Carbs,
        handle: &mut PointerInnerHandle<'_, Carbs>,
        event: &GestureHoldBeginEvent,
    ) {
        handle.gesture_hold_begin(data, event);
    }

    fn gesture_hold_end(
        &mut self,
        data: &mut Carbs,
        handle: &mut PointerInnerHandle<'_, Carbs>,
        event: &GestureHoldEndEvent,
    ) {
        handle.gesture_hold_end(data, event);
    }

    fn start_data(&self) -> &GrabStartData<Carbs> {
        &self.start_data
    }

    /// Called however the grab ends (released, or yanked away by the server).
    fn unset(&mut self, _data: &mut Carbs) {
        if let Some(toplevel) = self.window.toplevel() {
            toplevel.with_pending_state(|state| {
                state.states.unset(xdg_toplevel::State::Resizing);
                state.size = Some(self.last_size);
            });
            toplevel.send_pending_configure();
            resize_finished(toplevel.wl_surface());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edges_from_xdg() {
        assert_eq!(ResizeEdge::from(xdg_toplevel::ResizeEdge::None), ResizeEdge::NONE);
        assert_eq!(ResizeEdge::from(xdg_toplevel::ResizeEdge::Top), ResizeEdge::TOP);
        let tl = ResizeEdge::from(xdg_toplevel::ResizeEdge::TopLeft);
        assert!(tl.contains(ResizeEdge::TOP));
        assert!(tl.contains(ResizeEdge::LEFT));
        assert!(!tl.contains(ResizeEdge::BOTTOM));
        assert!(!tl.contains(ResizeEdge::RIGHT));
    }

    #[test]
    fn none_contains_nothing() {
        assert!(!ResizeEdge::NONE.contains(ResizeEdge::TOP));
        assert!(!ResizeEdge::NONE.contains(ResizeEdge::NONE));
        assert!(!ResizeEdge::TOP.contains(ResizeEdge::NONE));
    }

    #[test]
    fn pointer_quadrants_pick_edges() {
        let rect = Rectangle::new(Point::from((100, 100)), Size::from((200, 100)));
        // top-left corner area
        let e = edges_under_pointer(rect, Point::from((110.0, 110.0)));
        assert!(e.contains(ResizeEdge::TOP) && e.contains(ResizeEdge::LEFT));
        // bottom-right corner area
        let e = edges_under_pointer(rect, Point::from((290.0, 190.0)));
        assert!(e.contains(ResizeEdge::BOTTOM) && e.contains(ResizeEdge::RIGHT));
        // just past the middle horizontally, above the middle vertically
        let e = edges_under_pointer(rect, Point::from((250.0, 120.0)));
        assert!(e.contains(ResizeEdge::TOP) && e.contains(ResizeEdge::RIGHT));
    }
}
