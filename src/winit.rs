use smithay::{
    backend::{
        renderer::damage::OutputDamageTracker,
        winit::{self, WinitEvent},
    },
    output::{Mode, Output, PhysicalProperties, Subpixel},
    reexports::calloop::EventLoop,
    utils::{Rectangle, Transform},
};

use crate::{
    glass::Glass,
    render::{scene_elements, CLEAR_COLOR},
    CalloopData, Nekoland,
};

pub fn init_winit(
    event_loop: &mut EventLoop<CalloopData>,
    data: &mut CalloopData,
) -> Result<(), Box<dyn std::error::Error>> {
    let display_handle = &mut data.display_handle;
    let state = &mut data.state;

    let (mut backend, winit) = winit::init()?;

    let mode = Mode {
        size: backend.window_size(),
        refresh: 60_000,
    };

    let output = Output::new(
        "winit".to_string(),
        PhysicalProperties {
            size: (0, 0).into(),
            subpixel: Subpixel::Unknown,
            make: "Smithay".into(),
            model: "Winit".into(),
        },
    );
    let _global = output.create_global::<Nekoland>(display_handle);
    output.change_current_state(Some(mode), Some(Transform::Flipped180), None, Some((0, 0).into()));
    output.set_preferred(mode);

    state.space.map_output(&output, (0, 0));

    let mut damage_tracker = OutputDamageTracker::from_output(&output);
    let mut glass = Glass::new(backend.renderer())
        .inspect_err(|err| tracing::warn!("no glass: {err}"))
        .ok();

    std::env::set_var("WAYLAND_DISPLAY", &state.socket_name);

    event_loop.handle().insert_source(winit, move |event, _, data| {
        let display = &mut data.display_handle;
        let state = &mut data.state;

        match event {
            WinitEvent::Resized { size, .. } => {
                output.change_current_state(
                    Some(Mode {
                        size,
                        refresh: 60_000,
                    }),
                    None,
                    None,
                    None,
                );
                smithay::desktop::layer_map_for_output(&output).arrange();
            }
            WinitEvent::Input(event) => state.process_input_event(event),
            WinitEvent::Redraw => {
                let size = backend.window_size();
                let damage = Rectangle::from_size(size);

                state.tick_overview();
                state.refresh_menu_bar();
                state.dock_ids();
                state.refresh_dock();
                state.refresh_apps();
                state.refresh_notices();
                let elements = {
                    let (renderer, mut framebuffer) = backend.bind().unwrap();
                    let elements = scene_elements(state, renderer, &output, glass.as_mut()).unwrap();
                    damage_tracker
                        .render_output(renderer, &mut framebuffer, 0, &elements, CLEAR_COLOR)
                        .unwrap();
                    elements
                };
                backend.submit(Some(&[damage])).unwrap();
                // The host draws the cursor: none is among the elements.
                let frames = state.take_screencopy(&output);
                if !frames.is_empty() {
                    crate::screencopy::copy(backend.renderer(), &output, &elements, 0, frames);
                }

                state.finish_frame(&output);
                let _ = display.flush_clients();

                // Ask for redraw to schedule new frame.
                backend.window().request_redraw();
            }
            WinitEvent::CloseRequested => {
                state.loop_signal.stop();
            }
            _ => (),
        };
    })?;

    Ok(())
}
