use std::{
    ffi::OsString,
    sync::{atomic::AtomicBool, Arc},
    time::{Duration, SystemTime},
};

use smithay::{
    backend::{
        renderer::element::{
            default_primary_scanout_output_compare, utils::select_dmabuf_feedback,
            RenderElementStates,
        },
        session::Session,
    },
    delegate_presentation,
    desktop::{
        utils::{
            surface_presentation_feedback_flags_from_states, surface_primary_scanout_output,
            update_surface_primary_scanout_output, with_surfaces_surface_tree,
            OutputPresentationFeedback,
        },
        PopupManager, Space, Window, WindowSurfaceType,
    },
    input::{pointer::CursorImageStatus, Seat, SeatState},
    output::Output,
    reexports::{
        calloop::{
            generic::Generic, EventLoop, Interest, LoopHandle, LoopSignal, Mode, PostAction,
        },
        wayland_server::{
            backend::{ClientData, ClientId, DisconnectReason},
            protocol::wl_surface::WlSurface,
            Display, DisplayHandle,
        },
    },
    utils::{Clock, Coordinate, Logical, Monotonic, Point, Rectangle},
    wayland::{
        compositor::{CompositorClientState, CompositorState},
        dmabuf::DmabufFeedback,
        output::OutputManagerState,
        presentation::PresentationState,
        selection::data_device::DataDeviceState,
        shell::xdg::XdgShellState,
        shm::ShmState,
        socket::ListeningSocketSource,
    },
};
use tracing::{info, warn};

use crate::{
    input::{get_pos, logical, MARGIN, MARGIN_POS},
    udev::UdevData,
};

#[derive(Clone)]
pub struct SmallvilWorkspace {
    pub left_window: Option<smithay::desktop::Window>,
    pub top_window: Option<smithay::desktop::Window>,
    pub right_window: Option<smithay::desktop::Window>,
    pub bottom_window: Option<smithay::desktop::Window>,

    pub top_left_window: Option<smithay::desktop::Window>,
    pub top_right_window: Option<smithay::desktop::Window>,
    pub bottom_left_window: Option<smithay::desktop::Window>,
    pub bottom_right_window: Option<smithay::desktop::Window>,
}

pub const EMPTY_WORKSPACE: SmallvilWorkspace = SmallvilWorkspace {
    left_window: Option::None,
    top_window: Option::None,
    right_window: Option::None,
    bottom_window: Option::None,

    top_left_window: Option::None,
    top_right_window: Option::None,
    bottom_left_window: Option::None,
    bottom_right_window: Option::None,
};

#[derive(Clone, Copy)]
pub enum WindowPosition {
    TopLeft,
    Top,
    TopRight,
    Right,
    BottomRight,
    Bottom,
    BottomLeft,
    Left,
}

#[derive(Debug)]
pub struct DndIcon {
    pub surface: WlSurface,
    pub offset: Point<i32, Logical>,
}

#[derive(Clone)]
pub struct SurfaceDmabufFeedback {
    pub render_feedback: DmabufFeedback,
    pub scanout_feedback: DmabufFeedback,
}

pub struct AnimationInfo {
    pub start_time: SystemTime,
    pub start_pos: Point<f64, Logical>,
    pub start_velocity: Point<f64, Logical>,
    pub end_pos: WindowPosition,
}

const ANIMATION_MS: f64 = 300.0;

fn get_progress(d: Duration) -> f64 {
    if d.as_secs() > 1 {
        1.0
    } else {
        d.subsec_millis().to_f64() / ANIMATION_MS
    }
}

// TODO: Return the actual velocity
// TODO: Take info.start_velocity into account
// TODO: Ease animation in and out rather than having a constant velocity
pub fn get_pos_and_velocity(
    info: &AnimationInfo,
    output_geoemetry: Rectangle<i32, Logical>,
) -> (Point<f64, Logical>, Point<f64, Logical>, bool) {
    let now = SystemTime::now();
    let progress = get_progress(now.duration_since(info.start_time).unwrap());
    let end_pos = get_pos(output_geoemetry, &info.end_pos);
    (
        logical(
            info.start_pos.x * (1.0 - progress) + end_pos.x * progress,
            info.start_pos.y * (1.0 - progress) + end_pos.y * progress,
        ),
        logical(10.0, 10.0),
        progress >= 1.0,
    )
}

pub enum WorkspaceState {
    WindowFocussed(WindowPosition),
    Animating(AnimationInfo),
    Grabbed(Point<f64, Logical>),
}

pub struct Smallvil {
    pub cur_workspace: usize,
    pub cur_workspace_state: WorkspaceState,
    pub cur_monitor: usize,
    pub workspaces: Vec<SmallvilWorkspace>,

    pub start_time: std::time::Instant,
    pub socket_name: OsString,
    pub display_handle: DisplayHandle,
    pub seat_name: String,
    pub running: Arc<AtomicBool>,
    pub handle: LoopHandle<'static, Smallvil>,
    pub clock: Clock<Monotonic>,
    pub cursor_status: CursorImageStatus,
    pub dnd_icon: Option<DndIcon>,
    pub backend_data: Option<UdevData>,

    pub space: Space<Window>,
    pub loop_signal: LoopSignal,

    // Smithay State
    pub compositor_state: CompositorState,
    pub xdg_shell_state: XdgShellState,
    pub shm_state: ShmState,
    // These states only register their globals; they are kept for access and parity with anvil.
    #[allow(dead_code)]
    pub output_manager_state: OutputManagerState,
    pub seat_state: SeatState<Smallvil>,
    pub data_device_state: DataDeviceState,
    pub popups: PopupManager,
    #[allow(dead_code)]
    pub presentation_state: PresentationState,

    pub seat: Seat<Self>,
}

impl Smallvil {
    pub fn new(
        event_loop: &mut EventLoop<'static, Self>,
        display: Display<Self>,
        backend_data: Option<UdevData>,
    ) -> Self {
        let start_time = std::time::Instant::now();

        let dh = display.handle();

        let clock = Clock::new();

        let compositor_state = CompositorState::new::<Self>(&dh);
        let xdg_shell_state = XdgShellState::new::<Self>(&dh);
        let shm_state = ShmState::new::<Self>(&dh, vec![]);
        let output_manager_state = OutputManagerState::new_with_xdg_output::<Self>(&dh);
        let mut seat_state = SeatState::new();
        let data_device_state = DataDeviceState::new::<Self>(&dh);
        let popups = PopupManager::default();
        let presentation_state = PresentationState::new::<Self>(&dh, clock.id() as u32);

        // A seat is a group of keyboards, pointer and touch devices.
        // A seat typically has a pointer and maintains a keyboard focus and a pointer focus.
        let seat_name = backend_data
            .as_ref()
            .map(|backend| backend.session.seat())
            .unwrap_or_else(|| "winit".to_string());
        let mut seat: Seat<Self> = seat_state.new_wl_seat(&dh, seat_name.clone());

        // Notify clients that we have a keyboard, for the sake of the example we assume that keyboard is always present.
        // You may want to track keyboard hot-plug in real compositor.
        seat.add_keyboard(Default::default(), 200, 25).unwrap();

        // Notify clients that we have a pointer (mouse)
        // Here we assume that there is always pointer plugged in
        seat.add_pointer();

        // A space represents a two-dimensional plane. Windows and Outputs can be mapped onto it.
        //
        // Windows get a position and stacking order through mapping.
        // Outputs become views of a part of the Space and can be rendered via Space::render_output.
        let space = Space::default();

        let socket_name = Self::init_wayland_listener(display, event_loop.handle());

        // Get the loop signal, used to stop the event loop
        let loop_signal = event_loop.get_signal();

        Self {
            cur_workspace: 0,
            cur_workspace_state: WorkspaceState::WindowFocussed(WindowPosition::Left),
            cur_monitor: 0,
            workspaces: vec![EMPTY_WORKSPACE],

            start_time,
            display_handle: dh,
            seat_name,
            running: Arc::new(AtomicBool::new(true)),
            handle: event_loop.handle(),
            clock,
            cursor_status: CursorImageStatus::default_named(),
            dnd_icon: None,
            backend_data,

            space,
            loop_signal,
            socket_name,

            compositor_state,
            xdg_shell_state,
            shm_state,
            output_manager_state,
            seat_state,
            data_device_state,
            popups,
            presentation_state,
            seat,
        }
    }

    fn init_wayland_listener(
        display: Display<Smallvil>,
        handle: LoopHandle<'static, Smallvil>,
    ) -> OsString {
        // Creates a new listening socket, automatically choosing the next available `wayland` socket name.
        let listening_socket = ListeningSocketSource::new_auto().unwrap();

        // Get the name of the listening socket.
        // Clients will connect to this socket.
        let socket_name = listening_socket.socket_name().to_os_string();

        handle
            .insert_source(
                listening_socket,
                move |client_stream, _, state: &mut Smallvil| {
                    // Inside the callback, you should insert the client into the display.
                    //
                    // You may also associate some data with the client when inserting the client.
                    state
                        .display_handle
                        .insert_client(client_stream, Arc::new(ClientState::default()))
                        .unwrap();
                },
            )
            .expect("Failed to init the wayland event source.");

        // You also need to add the display itself to the event loop, so that client events will be processed by wayland-server.
        handle
            .insert_source(
                Generic::new(display, Interest::READ, Mode::Level),
                |_, display, state: &mut Smallvil| {
                    // Safety: we don't drop the display
                    unsafe {
                        display.get_mut().dispatch_clients(state).unwrap();
                    }
                    Ok(PostAction::Continue)
                },
            )
            .unwrap();

        socket_name
    }

    pub fn surface_under(
        &self,
        pos: Point<f64, Logical>,
    ) -> Option<(WlSurface, Point<f64, Logical>)> {
        self.space
            .element_under(pos)
            .and_then(|(window, location)| {
                window
                    .surface_under(pos - location.to_f64(), WindowSurfaceType::ALL)
                    .map(|(s, p)| (s, (p + location).to_f64()))
            })
    }

    /// Updates the currently focused monitor to the output under the given pointer location.
    pub fn update_cur_monitor(&mut self, location: Point<f64, Logical>) {
        if let Some((index, _)) = self.space.outputs().enumerate().find(|(_, output)| {
            self.space
                .output_geometry(output)
                .map(|geometry| geometry.contains(location.to_i32_round()))
                .unwrap_or(false)
        }) {
            self.cur_monitor = index;
        }
    }

    /// Returns the area occupied by the main window
    pub fn main_window_area(&self) -> Option<smithay::utils::Rectangle<i32, Logical>> {
        let Option::Some(output) = self.space.outputs().nth(self.cur_monitor) else {
            warn!("Failed to get focussed monitor");
            return Option::None;
        };
        let Option::Some(geometry) = self.space.output_geometry(&output) else {
            warn!("Failed to get output geometry");
            return Option::None;
        };
        Option::Some(smithay::utils::Rectangle::<i32, Logical>::new(
            geometry.loc + MARGIN_POS,
            geometry.size - smithay::utils::Size::<i32, Logical>::new(MARGIN * 2, MARGIN * 2),
        ))
    }

    pub fn post_repaint(
        &mut self,
        output: &Output,
        time: impl Into<std::time::Duration>,
        dmabuf_feedback: Option<SurfaceDmabufFeedback>,
        render_element_states: &RenderElementStates,
    ) {
        let time = time.into();
        let throttle = Some(std::time::Duration::from_secs(1));

        self.update_primary_scanout_output(output, render_element_states);

        for window in self.space.elements() {
            if self.space.outputs_for_element(window).contains(output) {
                window.send_frame(output, time, throttle, surface_primary_scanout_output);
                if let Some(dmabuf_feedback) = dmabuf_feedback.as_ref() {
                    window.send_dmabuf_feedback(
                        output,
                        surface_primary_scanout_output,
                        |surface, _| {
                            select_dmabuf_feedback(
                                surface,
                                render_element_states,
                                &dmabuf_feedback.render_feedback,
                                &dmabuf_feedback.scanout_feedback,
                            )
                        },
                    );
                }
            }
        }
    }

    fn update_primary_scanout_output(
        &mut self,
        output: &Output,
        render_element_states: &RenderElementStates,
    ) {
        for window in self.space.elements() {
            window.with_surfaces(|surface, states| {
                update_surface_primary_scanout_output(
                    surface,
                    output,
                    states,
                    render_element_states,
                    default_primary_scanout_output_compare,
                );
            });
        }

        let cursor_status = self.cursor_status.clone();
        if let CursorImageStatus::Surface(ref surface) = cursor_status {
            with_surfaces_surface_tree(surface, |surface, states| {
                update_surface_primary_scanout_output(
                    surface,
                    output,
                    states,
                    render_element_states,
                    default_primary_scanout_output_compare,
                );
            });
        }

        if let Some(icon) = self.dnd_icon.as_ref() {
            with_surfaces_surface_tree(&icon.surface, |surface, states| {
                update_surface_primary_scanout_output(
                    surface,
                    output,
                    states,
                    render_element_states,
                    default_primary_scanout_output_compare,
                );
            });
        }
    }

    /// Computes the current position of the workspace based on
    /// [`Smallvil::cur_workspace_state`]
    ///
    /// - `WindowFocussed`: the workspace sits at the position of the focused window
    ///   (via [`get_pos`])
    /// - `Animating`: the position (and velocity) are animated (via
    ///   [`get_pos_and_velocity`]); once the animation finished the state
    ///   transitions to `WindowFocussed`
    /// - `Grabbed`: the workspace follows the position of the grab
    pub fn get_pos(&mut self) -> Option<Point<f64, Logical>> {
        let Option::Some(main_window_area) = self.main_window_area() else {
            warn!("Failed to get main window area");
            return Option::None;
        };
        Option::Some(match &self.cur_workspace_state {
            WorkspaceState::WindowFocussed(window) => get_pos(main_window_area, window),
            WorkspaceState::Animating(info) => {
                let (pos, _velocity, finished) = get_pos_and_velocity(info, main_window_area);
                if finished {
                    info!("Finished animation");
                    self.cur_workspace_state = WorkspaceState::WindowFocussed(info.end_pos);
                }
                pos
            }
            WorkspaceState::Grabbed(pos) => *pos,
        })
    }

    /// Should be called on every render.
    pub fn update_workspace_position(&mut self) {
        match self.get_pos() {
            Option::Some(pos) => crate::input::position_windows(self, pos),
            Option::None => {}
        };
    }
}

pub fn take_presentation_feedback(
    output: &Output,
    space: &Space<Window>,
    render_element_states: &RenderElementStates,
) -> OutputPresentationFeedback {
    let mut output_presentation_feedback = OutputPresentationFeedback::new(output);

    space.elements().for_each(|window| {
        if space.outputs_for_element(window).contains(output) {
            window.take_presentation_feedback(
                &mut output_presentation_feedback,
                surface_primary_scanout_output,
                |surface, _| {
                    surface_presentation_feedback_flags_from_states(surface, render_element_states)
                },
            );
        }
    });

    output_presentation_feedback
}

#[derive(Default)]
pub struct ClientState {
    pub compositor_state: CompositorClientState,
}

impl ClientData for ClientState {
    fn initialized(&self, _client_id: ClientId) {}
    fn disconnected(&self, _client_id: ClientId, _reason: DisconnectReason) {}
}

// The presentation protocol is only consumed by clients, no special handling needed.
delegate_presentation!(Smallvil);
