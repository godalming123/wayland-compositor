use std::time::Duration;

use smithay::{
    backend::{
        renderer::{
            damage::OutputDamageTracker, element::surface::WaylandSurfaceRenderElement, gles::GlesRenderer,
        },
        winit::{self, WinitEvent},
    },
    output::{Mode, Output, PhysicalProperties, Subpixel},
    reexports::calloop::LoopHandle,
    utils::{Rectangle, Transform},
};

use crate::Smallvil;

pub fn init_winit(
    event_loop: &LoopHandle<'static, Smallvil>,
    state: &mut Smallvil,
) -> Result<(), Box<dyn std::error::Error>> {
    let display_handle = state.display_handle.clone();

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
    let _global = output.create_global::<Smallvil>(&display_handle);
    output.change_current_state(Some(mode), Some(Transform::Flipped180), None, Some((0, 0).into()));
    output.set_preferred(mode);

    state.space.map_output(&output, (0, 0));

    let mut damage_tracker = OutputDamageTracker::from_output(&output);

    std::env::set_var("WAYLAND_DISPLAY", &state.socket_name);

    event_loop.insert_source(winit, move |event, _, data: &mut Smallvil| {
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
            }
            WinitEvent::Input(event) => data.process_input_event(event),
            WinitEvent::Redraw => {
                // Update the workspace position before rendering this frame.
                data.update_workspace_position();

                let size = backend.window_size();
                let damage = Rectangle::from_size(size);

                {
                    let (renderer, mut framebuffer) = backend.bind().unwrap();
                    smithay::desktop::space::render_output::<
                        _,
                        WaylandSurfaceRenderElement<GlesRenderer>,
                        _,
                        _,
                    >(
                        &output,
                        renderer,
                        &mut framebuffer,
                        1.0,
                        0,
                        [&data.space],
                        &[],
                        &mut damage_tracker,
                        [0.1, 0.1, 0.1, 1.0],
                    )
                    .unwrap();
                }
                backend.submit(Some(&[damage])).unwrap();

                data.space.elements().for_each(|window| {
                    window.send_frame(
                        &output,
                        data.start_time.elapsed(),
                        Some(Duration::ZERO),
                        |_, _| Some(output.clone()),
                    )
                });

                data.space.refresh();
                data.popups.cleanup();
                let _ = data.display_handle.flush_clients();

                // Ask for redraw to schedule new frame.
                backend.window().request_redraw();
            }
            WinitEvent::CloseRequested => {
                data.running.store(false, std::sync::atomic::Ordering::SeqCst);
                data.loop_signal.stop();
            }
            _ => (),
        };
    })?;

    Ok(())
}
