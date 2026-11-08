//! Carbs 🍞 - the Wayland compositor for QuackOS (the Bread desktop runs on top of it).
//!
//!   carbs                 start a nested Carbs window
//!   carbs -c <command>    ...and launch a client inside it (e.g. carbs -c weston-terminal)
//!
//! Window handling:
//!   click                 focus and raise
//!   Super + left drag     move a window
//!   Super + right drag    resize from the nearest corner
//!   Super + Q             ask the focused window to close

mod config;
mod cursor;
mod grabs;
mod handlers;
mod input;
mod shell;
mod state;
mod winit;

use smithay::reexports::{calloop::EventLoop, wayland_server::Display};
use state::Carbs;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt().with_env_filter(
        tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
    ).init();

    let mut event_loop: EventLoop<'static, Carbs> = EventLoop::try_new()?;
    let display: Display<Carbs> = Display::new()?;
    let mut state = Carbs::new(&mut event_loop, display);

    winit::init_winit(&mut event_loop, &mut state)?;

    // SAFETY: single-threaded at this point; clients we spawn find us through this.
    unsafe { std::env::set_var("WAYLAND_DISPLAY", &state.socket_name) };
    tracing::info!("🍞 Carbs is up on WAYLAND_DISPLAY={}", state.socket_name.to_string_lossy());

    let mut args = std::env::args().skip(1);
    if let (Some(flag), Some(cmd)) = (args.next(), args.next()) {
        if flag == "-c" {
            let rest: Vec<String> = args.collect();
            if let Err(e) = std::process::Command::new(&cmd).args(rest).spawn() {
                tracing::error!("couldn't start '{cmd}': {e}");
            }
        }
    }

    event_loop.run(None, &mut state, |state| {
        let _ = state.display_handle.flush_clients();
    })?;
    Ok(())
}
