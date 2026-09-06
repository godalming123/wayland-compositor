use std::{
    ffi::OsString,
    sync::{atomic::AtomicBool, Arc},
    time::SystemTime,
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
    utils::{Clock, Coordinate, Logical, Monotonic, Point},
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
    input::{distance, get_position, logical, MARGIN, MARGIN_POS},
    udev::UdevData,
};

pub struct WorkspaceAreas<T> {
    pub top_left_window: T,
    pub top_window: T,
    pub top_right_window: T,
    pub right_window: T,
    pub bottom_right_window: T,
    pub bottom_window: T,
    pub bottom_left_window: T,
    pub left_window: T,
}

impl<A> WorkspaceAreas<A> {
    pub fn replace(&mut self, position: WindowPosition, new_value: A) {
        match position {
            WindowPosition::TopLeft => self.top_left_window = new_value,
            WindowPosition::Top => self.top_window = new_value,
            WindowPosition::TopRight => self.top_right_window = new_value,
            WindowPosition::Right => self.right_window = new_value,
            WindowPosition::BottomRight => self.bottom_right_window = new_value,
            WindowPosition::Bottom => self.bottom_window = new_value,
            WindowPosition::BottomLeft => self.bottom_left_window = new_value,
            WindowPosition::Left => self.left_window = new_value,
        }
    }
    pub fn merge_with<'l, B, C, F: Fn(&'l A, B) -> C>(
        &'l self,
        other: WorkspaceAreas<B>,
        func: F,
    ) -> WorkspaceAreas<C> {
        WorkspaceAreas::<C> {
            top_left_window: func(&self.top_left_window, other.top_left_window),
            top_window: func(&self.top_window, other.top_window),
            top_right_window: func(&self.top_right_window, other.top_right_window),
            right_window: func(&self.right_window, other.right_window),
            bottom_right_window: func(&self.bottom_right_window, other.bottom_right_window),
            bottom_window: func(&self.bottom_window, other.bottom_window),
            bottom_left_window: func(&self.bottom_left_window, other.bottom_left_window),
            left_window: func(&self.left_window, other.left_window),
        }
    }
}

pub type SmallvilWorkspace = WorkspaceAreas<Option<smithay::desktop::Window>>;
pub type WindowAreas = WorkspaceAreas<ProgressAndVelocity>;

struct Wrapped<T> {
    inner: T,
}

impl<A> Wrapped<A> {
    pub fn pipe<F, B>(self, func: F) -> Wrapped<B>
    where
        F: FnOnce(A) -> B,
    {
        Wrapped::<B> {
            inner: func(self.inner),
        }
    }

    pub fn unwrap(self) -> A {
        self.inner
    }
}

impl<T> Wrapped<(f64, T)> {
    fn min(self, b_num: f64, b_info: T) -> Self {
        self.pipe(|a| if a.0 < b_num { a } else { (b_num, b_info) })
    }
}

impl WindowAreas {
    pub fn min_progress(&self) -> (f64, WindowPosition) {
        Wrapped {
            inner: (self.top_left_window.progress, WindowPosition::TopLeft),
        }
        .min(self.top_window.progress, WindowPosition::Top)
        .min(self.top_right_window.progress, WindowPosition::TopRight)
        .min(self.right_window.progress, WindowPosition::Right)
        .min(
            self.bottom_right_window.progress,
            WindowPosition::BottomRight,
        )
        .min(self.bottom_window.progress, WindowPosition::Bottom)
        .min(self.bottom_left_window.progress, WindowPosition::BottomLeft)
        .min(self.left_window.progress, WindowPosition::Left)
        .unwrap()
    }

    pub fn from_position(position: WindowPosition) -> Self {
        let default_position = ProgressAndVelocity {
            progress: 1.0,
            velocity: 0.0,
        };
        let mut out = Self {
            top_left_window: default_position,
            top_window: default_position,
            top_right_window: default_position,
            right_window: default_position,
            bottom_right_window: default_position,
            bottom_window: default_position,
            bottom_left_window: default_position,
            left_window: default_position,
        };
        out.replace(
            position,
            ProgressAndVelocity {
                progress: 0.0,
                velocity: 1.0,
            },
        );
        out
    }

    pub fn from_gesture(
        grab_start: WindowPosition,
        p: Point<f64, Logical>, // x and y in range -1.0 to 1.0 inclusive
    ) -> Self {
        let n = f64::sqrt(0.5);
        // TODO: Specify velocity
        let mut out = WorkspaceAreas::<ProgressAndVelocity> {
            top_left_window: ProgressAndVelocity {
                progress: distance(logical(-n, -n), p),
                velocity: 0.0,
            },
            top_window: ProgressAndVelocity {
                progress: distance(logical(0.0, -1.0), p),
                velocity: 0.0,
            },
            top_right_window: ProgressAndVelocity {
                progress: distance(logical(n, -n), p),
                velocity: 0.0,
            },
            right_window: ProgressAndVelocity {
                progress: distance(logical(1.0, 0.0), p),
                velocity: 0.0,
            },
            bottom_right_window: ProgressAndVelocity {
                progress: distance(logical(n, n), p),
                velocity: 0.0,
            },
            bottom_window: ProgressAndVelocity {
                progress: distance(logical(0.0, 1.0), p),
                velocity: 0.0,
            },
            bottom_left_window: ProgressAndVelocity {
                progress: distance(logical(-n, n), p),
                velocity: 0.0,
            },
            left_window: ProgressAndVelocity {
                progress: distance(logical(-1.0, 0.0), p),
                velocity: 0.0,
            },
        };
        out.replace(
            grab_start,
            ProgressAndVelocity {
                progress: 1.0,
                velocity: 0.0,
            },
        );
        let (min_progress, _) = out.min_progress();
        out.replace(
            grab_start,
            ProgressAndVelocity {
                progress: 1.0 - min_progress,
                velocity: 0.0,
            },
        );
        out
    }

    // TODO: Return the actual velocity
    // TODO: Take info.start_velocity into account
    // TODO: Ease animation in and out rather than having a constant velocity
    pub fn from_animation(info: &AnimationInfo, end_pos: WindowPosition) -> (Self, bool) {
        let now = SystemTime::now();
        let elapsed = now.duration_since(info.start_time).unwrap();
        let progress = if elapsed.as_secs() > 1 {
            1.0
        } else {
            elapsed.subsec_millis().to_f64() / ANIMATION_MS
        };
        let c = |start: &ProgressAndVelocity, end: ProgressAndVelocity| ProgressAndVelocity {
            progress: start.progress * (1.0 - progress).max(0.0) + end.progress * progress,
            velocity: 10.0,
        };
        (
            info.start_info.merge_with(Self::from_position(end_pos), c),
            progress >= 1.0,
        )
    }
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
    pub start_info: WorkspaceAreas<ProgressAndVelocity>,
}

const ANIMATION_MS: f64 = 300.0;

#[derive(Clone, Copy)]
pub struct ProgressAndVelocity {
    pub progress: f64, // 0.0 - 1.0
    velocity: f64,     // TODO
}

pub enum WorkspaceState {
    Normal,
    Animating(AnimationInfo),
    Grabbed(Point<f64, Logical>),
}

pub struct Smallvil {
    pub cur_workspace: usize,
    pub cur_workspace_focussed_window: WindowPosition,
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
            cur_workspace_focussed_window: WindowPosition::TopLeft,
            cur_workspace_state: WorkspaceState::Normal,
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
    pub fn main_window_area(
        &self,
    ) -> Option<(
        smithay::utils::Rectangle<f64, Logical>,
        smithay::utils::Size<i32, Logical>,
    )> {
        let Option::Some(output) = self.space.outputs().nth(self.cur_monitor) else {
            warn!("Failed to get focussed monitor");
            return Option::None;
        };
        let Option::Some(geometry) = self.space.output_geometry(&output) else {
            warn!("Failed to get output geometry");
            return Option::None;
        };
        let size =
            geometry.size - smithay::utils::Size::<i32, Logical>::new(MARGIN * 2, MARGIN * 2);
        Option::Some((
            smithay::utils::Rectangle::<f64, Logical>::new(
                (geometry.loc + MARGIN_POS).to_f64(),
                size.to_f64(),
            ),
            size,
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
    /// Should be called on every render.
    pub fn update_workspace_position(&mut self) {
        let Option::Some((main_window_area, main_window_area_size)) = self.main_window_area()
        else {
            warn!("Failed to get output geometry");
            return;
        };

        let window_areas = match &self.cur_workspace_state {
            WorkspaceState::Normal => {
                info!("Getting window areas from position");
                WindowAreas::from_position(self.cur_workspace_focussed_window)
            }
            WorkspaceState::Animating(info) => {
                info!("Getting window areas from animation");
                let (window_areas, finished) =
                    WindowAreas::from_animation(info, self.cur_workspace_focussed_window);
                if finished {
                    info!("Finished animation");
                    self.cur_workspace_state = WorkspaceState::Normal;
                }
                window_areas
            }
            WorkspaceState::Grabbed(pos) => {
                info!("Getting window areas from grabbed");
                WindowAreas::from_gesture(
                    self.cur_workspace_focussed_window,
                    get_position(main_window_area, *pos),
                )
            }
        };
        crate::input::position_windows(self, window_areas, main_window_area, main_window_area_size);
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
