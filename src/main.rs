#![allow(irrefutable_let_patterns)]

mod cursor;
mod drawing;
mod handlers;
mod grabs;
mod input;
mod state;
mod udev;
mod winit;

use std::{
    collections::HashMap,
    sync::atomic::Ordering,
    time::Duration,
};

use smithay::{
    backend::{
        drm::{DrmNode, NodeType},
        egl::context::ContextPriority,
        input::{InputEvent},
        libinput::{LibinputInputBackend, LibinputSessionInterface},
        renderer::{
            multigpu::{gbm::GbmGlesBackend, GpuManager},
            ImportDma, ImportEgl, ImportMemWl,
        },
        session::{libseat::LibSeatSession, Event as SessionEvent, Session},
        udev::{all_gpus, primary_gpu, UdevBackend, UdevEvent},
    },
    reexports::input::{DeviceCapability, Libinput},
    reexports::calloop::EventLoop,
    wayland::{
        dmabuf::{DmabufFeedbackBuilder, DmabufState},
        drm_syncobj::{supports_syncobj_eventfd, DrmSyncobjState},
    },
};

use crate::state::Smallvil;

pub use udev::UdevData;
use udev::{get_surface_dmabuf_feedback, DeviceAddError};

fn print_help() {
    println!("- `winnit` - Start windowed");
    println!("- `tty-udev` - Start on a tty using the udev backend (requires root if without logind)");
    println!("- `help` - Show this help message");
}

fn main() {
    if let Ok(env_filter) = tracing_subscriber::EnvFilter::try_from_default_env() {
        tracing_subscriber::fmt().with_env_filter(env_filter).init();
    } else {
        tracing_subscriber::fmt().init();
    }

    let winnit = String::from("winnit");
    let tty_udev = String::from("tty-udev");
    let help = String::from("help");
    match std::env::args().collect::<Vec<String>>().as_slice() {
        [_, c] if *c == winnit => run_winnit(),
        [_, c] if *c == tty_udev => run_udev(),
        [_, c] if *c == help => {
            print_help();
        }
        _ => {
            println!("Invalid arguments");
            print_help();
        }
    }
}

fn run_winnit() {
    let mut event_loop: EventLoop<Smallvil> = EventLoop::try_new().unwrap();
    let display: smithay::reexports::wayland_server::Display<Smallvil> =
        smithay::reexports::wayland_server::Display::new().unwrap();
    let mut state = Smallvil::new(&mut event_loop, display, None);

    crate::winit::init_winit(&event_loop.handle(), &mut state).unwrap();

    event_loop
        .run(None, &mut state, move |_| {
            // Smallvil is running
        })
        .unwrap();
}

fn run_udev() {
    let mut event_loop: EventLoop<Smallvil> = EventLoop::try_new().unwrap();
    let display: smithay::reexports::wayland_server::Display<Smallvil> =
        smithay::reexports::wayland_server::Display::new().unwrap();
    let mut display_handle = display.handle();

    /*
     * Initialize session
     */
    let (session, notifier) = match LibSeatSession::new() {
        Ok(ret) => ret,
        Err(err) => {
            eprintln!("Could not initialize a session: {}", err);
            return;
        }
    };

    /*
     * Initialize the compositor
     */
    let primary_gpu = if let Ok(var) = std::env::var("ANVIL_DRM_DEVICE") {
        DrmNode::from_path(var).expect("Invalid drm device path")
    } else {
        primary_gpu(session.seat())
            .unwrap()
            .and_then(|x| {
                DrmNode::from_path(x)
                    .ok()?
                    .node_with_type(NodeType::Render)?
                    .ok()
            })
            .unwrap_or_else(|| {
                all_gpus(session.seat())
                    .unwrap()
                    .into_iter()
                    .find_map(|x| DrmNode::from_path(x).ok())
                    .expect("No GPU!")
            })
    };
    println!("Using {} as primary gpu.", primary_gpu);

    let gpus =
        GpuManager::new(GbmGlesBackend::with_context_priority(ContextPriority::High)).unwrap();

    let data = UdevData {
        dh: display_handle.clone(),
        dmabuf_state: None,
        syncobj_state: None,
        session,
        primary_gpu,
        gpus,
        backends: HashMap::new(),
        pointer_image: crate::cursor::Cursor::load(),
        pointer_images: Vec::new(),
        pointer_element: crate::drawing::PointerElement::default(),
        keyboards: Vec::new(),
    };
    let mut state = Smallvil::new(&mut event_loop, display, Some(data));

    /*
     * Initialize the udev backend
     */
    let udev_backend = match UdevBackend::new(&state.seat_name) {
        Ok(ret) => ret,
        Err(err) => {
            eprintln!("Failed to initialize udev backend: {}", err);
            return;
        }
    };

    /*
     * Initialize libinput backend
     */
    let mut libinput_context = Libinput::new_with_udev::<LibinputSessionInterface<LibSeatSession>>(
        state
            .backend_data
            .as_ref()
            .unwrap()
            .session
            .clone()
            .into(),
    );
    libinput_context.udev_assign_seat(&state.seat_name).unwrap();
    let libinput_backend = LibinputInputBackend::new(libinput_context.clone());

    /*
     * Bind all our objects that get driven by the event loop
     */
    event_loop
        .handle()
        .insert_source(libinput_backend, move |mut event, _, data: &mut Smallvil| {
            if let InputEvent::DeviceAdded { device } = &mut event {
                if device.has_capability(DeviceCapability::Keyboard) {
                    if let Some(led_state) = data
                        .seat
                        .get_keyboard()
                        .map(|keyboard| keyboard.led_state())
                    {
                        device.led_update(led_state.into());
                    }
                    if let Some(backend) = data.backend_data.as_mut() {
                        backend.keyboards.push(device.clone());
                    }
                }
            } else if let InputEvent::DeviceRemoved { ref device } = event {
                if device.has_capability(DeviceCapability::Keyboard) {
                    if let Some(backend) = data.backend_data.as_mut() {
                        backend.keyboards.retain(|item| item != device);
                    }
                }
            }

            data.process_input_event(event)
        })
        .unwrap();

    event_loop
        .handle()
        .insert_source(notifier, move |event, &mut (), data: &mut Smallvil| match event {
            SessionEvent::PauseSession => {
                libinput_context.suspend();
                println!("pausing session");

                if let Some(backend_data) = data.backend_data.as_mut() {
                    for backend in backend_data.backends.values_mut() {
                        backend.drm_output_manager.pause();
                    }
                }
            }
            SessionEvent::ActivateSession => {
                println!("resuming session");

                if let Err(err) = libinput_context.resume() {
                    eprintln!("Failed to resume libinput context: {:?}", err);
                }
                if let Some(backend_data) = data.backend_data.as_mut() {
                    for (node, backend) in backend_data
                        .backends
                        .iter_mut()
                        .map(|(handle, backend)| (*handle, backend))
                    {
                        // if we do not care about flicking (caused by modesetting) we could just
                        // pass true for disable connectors here. this would make sure our drm
                        // device is in a known state (all connectors and planes disabled).
                        // but for demonstration we choose a more optimistic path by leaving the
                        // state as is and assume it will just work. If this assumption fails
                        // we will try to reset the state when trying to queue a frame.
                        backend
                            .drm_output_manager
                            .activate(false)
                            .expect("failed to activate drm backend");
                        data.handle
                            .insert_idle(move |data: &mut Smallvil| {
                                data.render(node, None, data.clock.now())
                            });
                    }
                }
            }
        })
        .unwrap();

    // We try to initialize the primary node before others to make sure
    // any display only node can fall back to the primary node for rendering
    let primary_node = primary_gpu
        .node_with_type(NodeType::Primary)
        .and_then(|node| node.ok());
    let primary_device = udev_backend.device_list().find(|(device_id, _)| {
        primary_node
            .map(|primary_node| *device_id == primary_node.dev_id())
            .unwrap_or(false)
            || *device_id == primary_gpu.dev_id()
    });

    if let Some((device_id, path)) = primary_device {
        let node = DrmNode::from_dev_id(device_id).expect("failed to get primary node");
        state
            .device_added(node, path)
            .expect("failed to initialize primary node");
    }

    let primary_device_id = primary_device.map(|(device_id, _)| device_id);
    for (device_id, path) in udev_backend.device_list() {
        if Some(device_id) == primary_device_id {
            continue;
        }

        if let Err(err) = DrmNode::from_dev_id(device_id)
            .map_err(DeviceAddError::DrmNode)
            .and_then(|node| state.device_added(node, path))
        {
            eprintln!("Skipping device {device_id}: {err}");
        }
    }
    state.shm_state.update_formats(
        state
            .backend_data
            .as_mut()
            .unwrap()
            .gpus
            .single_renderer(&primary_gpu)
            .unwrap()
            .shm_formats(),
    );

    let mut renderer = state
        .backend_data
        .as_mut()
        .unwrap()
        .gpus
        .single_renderer(&primary_gpu)
        .unwrap();

    println!(
        "Trying to initialize EGL Hardware Acceleration for {:?}",
        primary_gpu
    );
    match renderer.bind_wl_display(&display_handle) {
        Ok(_) => println!("EGL hardware-acceleration enabled"),
        Err(err) => println!("Failed to initialize EGL hardware-acceleration: {:?}", err),
    }

    // init dmabuf support with format list from our primary gpu
    let dmabuf_formats = renderer.dmabuf_formats();
    let default_feedback = DmabufFeedbackBuilder::new(primary_gpu.dev_id(), dmabuf_formats)
        .build()
        .unwrap();
    let mut dmabuf_state = DmabufState::new();
    let global = dmabuf_state.create_global_with_default_feedback::<Smallvil>(
        &display_handle,
        &default_feedback,
    );
    state.backend_data.as_mut().unwrap().dmabuf_state = Some((dmabuf_state, global));

    let backend_data = state.backend_data.as_mut().unwrap();
    let gpus = &mut backend_data.gpus;
    backend_data
        .backends
        .iter_mut()
        .for_each(|(node, backend_data)| {
            // Update the per drm surface dmabuf feedback
            backend_data.surfaces.values_mut().for_each(|surface_data| {
                surface_data.dmabuf_feedback = surface_data.dmabuf_feedback.take().or_else(|| {
                    surface_data.drm_output.with_compositor(|compositor| {
                        get_surface_dmabuf_feedback(
                            primary_gpu,
                            surface_data.render_node,
                            *node,
                            gpus,
                            compositor.surface(),
                        )
                    })
                });
            });
        });

    // Expose syncobj protocol if supported by primary GPU
    if let Some(primary_node) = state
        .backend_data
        .as_ref()
        .unwrap()
        .primary_gpu
        .node_with_type(NodeType::Primary)
        .and_then(|x| x.ok())
    {
        if let Some(backend) = state
            .backend_data
            .as_ref()
            .unwrap()
            .backends
            .get(&primary_node)
        {
            let import_device = backend.drm_output_manager.device().device_fd().clone();
            if supports_syncobj_eventfd(&import_device) {
                let syncobj_state =
                    DrmSyncobjState::new::<Smallvil>(&display_handle, import_device);
                state.backend_data.as_mut().unwrap().syncobj_state = Some(syncobj_state);
            }
        }
    }

    event_loop
        .handle()
        .insert_source(udev_backend, move |event, _, data: &mut Smallvil| match event {
            UdevEvent::Added { device_id, path } => {
                if let Err(err) = DrmNode::from_dev_id(device_id)
                    .map_err(DeviceAddError::DrmNode)
                    .and_then(|node| data.device_added(node, &path))
                {
                    eprintln!("Skipping device {device_id}: {err}");
                }
            }
            UdevEvent::Changed { device_id } => {
                if let Ok(node) = DrmNode::from_dev_id(device_id) {
                    data.device_changed(node)
                }
            }
            UdevEvent::Removed { device_id } => {
                if let Ok(node) = DrmNode::from_dev_id(device_id) {
                    data.device_removed(node)
                }
            }
        })
        .unwrap();

    /*
     * And run our loop
     */

    while state.running.load(Ordering::SeqCst) {
        let result = event_loop.dispatch(Some(Duration::from_millis(16)), &mut state);
        if result.is_err() {
            state.running.store(false, Ordering::SeqCst);
        } else {
            state.space.refresh();
            state.popups.cleanup();
            display_handle.flush_clients().unwrap();
        }
    }
}
