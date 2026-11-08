//! Nested backend: Carbs runs as a window inside your current desktop (X11 or Wayland).
//! Safe for development - if Carbs crashes, your real session is untouched.

use std::time::Duration;

use smithay::{
    backend::{
        renderer::{
            damage::OutputDamageTracker, element::surface::WaylandSurfaceRenderElement, gles::GlesRenderer,
            glow::GlowRenderer,
        },
        winit::{self, WinitEvent},
    },
    output::{Mode, Output, PhysicalProperties, Subpixel},
    reexports::calloop::EventLoop,
    utils::{Rectangle, Transform},
};

use crate::state::Carbs;

const BACKGROUND: [f32; 4] = [0.96, 0.78, 0.25, 1.0]; // duck yellow 🦆

pub fn init_winit(event_loop: &mut EventLoop<'static, Carbs>, state: &mut Carbs) -> Result<(), Box<dyn std::error::Error>> {
    let (mut backend, winit_source) = winit::init::<GlowRenderer>().map_err(|e| format!("winit init failed: {e:?}"))?;
    let _ = std::marker::PhantomData::<GlesRenderer>;

    let mode = Mode { size: backend.window_size(), refresh: 60_000 };
    let output = Output::new(
        "carbs-nested".to_string(),
        PhysicalProperties { size: (0, 0).into(), subpixel: Subpixel::Unknown, make: "Carbs".into(), model: "Nested".into() },
    );
    let _global = output.create_global::<Carbs>(&state.display_handle);
    output.change_current_state(Some(mode), Some(Transform::Flipped180), None, Some((0, 0).into()));
    output.set_preferred(mode);
    state.space.map_output(&output, (0, 0));

    let mut damage_tracker = OutputDamageTracker::from_output(&output);

    event_loop.handle().insert_source(winit_source, move |event, _, state| match event {
        WinitEvent::Resized { size, .. } => {
            output.change_current_state(Some(Mode { size, refresh: 60_000 }), None, None, None);
        }
        WinitEvent::Input(event) => state.process_input_event(event),
        WinitEvent::Redraw => {
            let size = backend.window_size();
            let damage = Rectangle::from_size(size);
            {
                let (renderer, mut framebuffer) = backend.bind().unwrap();
                smithay::desktop::space::render_output::<_, WaylandSurfaceRenderElement<GlowRenderer>, _, _>(
                    &output,
                    renderer,
                    &mut framebuffer,
                    1.0,
                    0,
                    [&state.space],
                    &[],
                    &mut damage_tracker,
                    BACKGROUND,
                )
                .unwrap();
            }
            backend.submit(Some(&[damage])).unwrap();

            let now = state.start_time.elapsed();
            for window in state.space.elements() {
                window.send_frame(&output, now, Some(Duration::ZERO), |_, _| Some(output.clone()));
            }
            state.space.refresh();
            state.popups.cleanup();
            let _ = state.display_handle.flush_clients();

            backend.window().request_redraw();
        }
        WinitEvent::CloseRequested => state.loop_signal.stop(),
        _ => {}
    })?;

    Ok(())
}
