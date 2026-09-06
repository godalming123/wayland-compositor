use std::{sync::atomic::Ordering, time::SystemTime};

use smithay::{
    backend::{
        input::{
            AbsolutePositionEvent, Axis, AxisSource,
            ButtonState::{self},
            Event, GestureSwipeUpdateEvent, InputBackend, InputEvent, KeyState, KeyboardKeyEvent,
            PointerAxisEvent, PointerButtonEvent, PointerMotionEvent,
        },
        session::Session,
    },
    desktop::{Space, Window},
    input::{
        keyboard::{keysyms as xkb, FilterResult, Keysym},
        pointer::{AxisFrame, ButtonEvent, MotionEvent},
    },
    reexports::{
        wayland_protocols::xdg::shell::server::xdg_toplevel,
        wayland_server::protocol::wl_surface::WlSurface,
    },
    utils::{Logical, Point, Rectangle, Size, SERIAL_COUNTER},
};
use tracing::{error, info, warn};

use crate::state::{
    AnimationInfo, ProgressAndVelocity, Smallvil, WindowAreas, WindowPosition, WorkspaceAreas,
    WorkspaceState,
};

/// Possible results of a keyboard action
enum Action {
    FocusInDirection(WindowPosition),
    SpawnCommand(&'static str), // Spawn a command
    CloseFocussedWindow,        // Close the currently focussed window
    Quit,                       // Quit the compositor
    VtSwitch(i32),              // Trigger a vt-switch
}

fn parse_released_key(
    _: &mut Smallvil,
    _modifiers: &smithay::input::keyboard::ModifiersState,
    _handle: smithay::input::keyboard::KeysymHandle<'_>,
) -> FilterResult<Action> {
    return FilterResult::Forward;
}

fn parse_pressed_key(
    _: &mut Smallvil,
    modifiers: &smithay::input::keyboard::ModifiersState,
    handle: smithay::input::keyboard::KeysymHandle<'_>,
) -> FilterResult<Action> {
    let sym = handle.modified_sym();
    info!("Parsing keyboard event {:?}", sym.name());
    // ctrl+alt+Fx switches to the corresponding virtual terminal
    if modifiers.alt
        && modifiers.ctrl
        && (xkb::KEY_XF86Switch_VT_1..=xkb::KEY_XF86Switch_VT_12).contains(&sym.raw())
    {
        return FilterResult::Intercept(Action::VtSwitch(
            (sym.raw() - xkb::KEY_XF86Switch_VT_1 + 1) as i32,
        ));
    }
    if modifiers.alt || modifiers.logo {
        match sym {
            Keysym::Tab => FilterResult::Intercept(Action::Quit),
            Keysym::f => FilterResult::Intercept(Action::SpawnCommand("firefox")),
            Keysym::g => FilterResult::Intercept(Action::SpawnCommand("ghostty")),
            Keysym::x => FilterResult::Intercept(Action::SpawnCommand("systemctl suspend")),
            Keysym::y => FilterResult::Intercept(Action::FocusInDirection(WindowPosition::TopLeft)),
            Keysym::u => FilterResult::Intercept(Action::FocusInDirection(WindowPosition::Top)),
            Keysym::i => {
                FilterResult::Intercept(Action::FocusInDirection(WindowPosition::TopRight))
            }
            Keysym::k => FilterResult::Intercept(Action::FocusInDirection(WindowPosition::Right)),
            Keysym::comma => {
                FilterResult::Intercept(Action::FocusInDirection(WindowPosition::BottomRight))
            }
            Keysym::m | Keysym::j => {
                FilterResult::Intercept(Action::FocusInDirection(WindowPosition::Bottom))
            }
            Keysym::n => {
                FilterResult::Intercept(Action::FocusInDirection(WindowPosition::BottomLeft))
            }
            Keysym::h => FilterResult::Intercept(Action::FocusInDirection(WindowPosition::Left)),
            _ => FilterResult::Forward,
        }
    } else {
        FilterResult::Forward
    }
}

pub const fn logical<T>(x: T, y: T) -> Point<T, Logical> {
    return Point::<T, Logical>::new(x, y);
}

pub const PADDING: i32 = 10;
pub const MARGIN: i32 = 50;
pub const MARGIN_POS: Point<i32, Logical> = logical(MARGIN, MARGIN);

/*
fn get_line_intersection(
    gradient0: f64,
    intercept0: f64,
    gradient1: f64,
    intercept1: f64,
) -> Point<f64, Logical> {
    let x = (intercept0 - intercept1) / (gradient1 - gradient0);
    let y = gradient0 * x + intercept0;
    return logical(x, y);
}

fn get_y_intercept(gradient: f64, point_on_line: Point<f64, Logical>) -> f64 {
    return point_on_line.y - (gradient * point_on_line.x);
}

fn get_left_right_top_bottom_for_points(
    a: Point<f64, Logical>,
    b: Point<f64, Logical>,
) -> (
    Point<f64, Logical>,
    Point<f64, Logical>,
    Point<f64, Logical>,
    Point<f64, Logical>,
) {
    let (left, right) = if a.x < b.x { (a, b) } else { (b, a) };
    let (top, bottom) = if a.y < b.y { (a, b) } else { (b, a) };
    (left, right, top, bottom)
}
*/

fn rubber_band_delta(delta: f64, container_size: f64) -> f64 {
    // let out_abs = (delta.abs() + 1.0).powf(0.8) - 1.0;
    // let out_abs = (delta.abs() + 1.0).log10() - 1.0;
    let out_abs = (1.0 - (1.0 / ((delta.abs() * 0.55 / container_size) + 1.0))) * container_size;
    if delta > 0.0 {
        out_abs
    } else {
        -out_abs
    }
}

/*
fn clamp_to_point(
    clamp_to: Point<f64, Logical>,
    point_to_clamp: Point<f64, Logical>,
    container_size: f64,
) -> Point<f64, Logical> {
    let delta = point_to_clamp - clamp_to;
    clamp_to
        + logical(
            rubber_band_delta(delta.x, container_size),
            rubber_band_delta(delta.y, container_size),
        )
}
*/

pub fn distance(a: Point<f64, Logical>, b: Point<f64, Logical>) -> f64 {
    let delta_x = a.x - b.x;
    let delta_y = a.y - b.y;
    (delta_x * delta_x + delta_y * delta_y).sqrt()
}

pub const fn between(
    a: Point<f64, Logical>,
    b: Point<f64, Logical>,
    b_proportion: f64,
) -> Point<f64, Logical> {
    let a_proportion = 1.0 - b_proportion;
    logical(
        a.x * a_proportion + b.x * b_proportion,
        a.y * a_proportion + b.y * b_proportion,
    )
}

fn clamp_to_line(
    start: Point<f64, Logical>,
    end: Point<f64, Logical>,
    portion_along: f64,
) -> Point<f64, Logical> {
    if portion_along < 0.0 {
        error!("portion along: {}", portion_along);
    };
    /*
    let restricted_portion_along = rubber_band_delta(portion_along, 1.0);
    */
    let restricted_portion_along = portion_along.max(0.0).min(1.0);
    info!(
        "Portion along: {}, restricted portion along: {}",
        portion_along, restricted_portion_along
    );
    between(start, end, restricted_portion_along)
    /*
    let (left, right, top, bottom) = get_left_right_top_bottom_for_points(start, end);
    if clamped_to_line.x < left.x {
        clamp_to_point(left, clamped_to_line, distance(start, end))
    } else if clamped_to_line.x > right.x {
        clamp_to_point(right, clamped_to_line, distance(start, end))
    } else if clamped_to_line.y < top.y {
        clamp_to_point(top, clamped_to_line, distance(start, end))
    } else if clamped_to_line.y > bottom.y {
        clamp_to_point(bottom, clamped_to_line, distance(start, end))
    } else {
        clamped_to_line
    }
    */
}

fn get_left(main_window_area: Rectangle<f64, Logical>) -> Point<f64, Logical> {
    main_window_area.loc + logical(-main_window_area.size.w - f64::from(PADDING), 0.0)
}

fn get_top(main_window_area: Rectangle<f64, Logical>) -> Point<f64, Logical> {
    main_window_area.loc + logical(0.0, -main_window_area.size.h - f64::from(PADDING))
}

fn get_right(main_window_area: Rectangle<f64, Logical>) -> Point<f64, Logical> {
    main_window_area.loc + logical(main_window_area.size.w + f64::from(PADDING), 0.0)
}

fn get_bottom(main_window_area: Rectangle<f64, Logical>) -> Point<f64, Logical> {
    main_window_area.loc + logical(0.0, main_window_area.size.h + f64::from(PADDING))
}

fn get_top_left(main_window_area: Rectangle<f64, Logical>) -> Point<f64, Logical> {
    main_window_area.loc
        + logical(
            -main_window_area.size.w - f64::from(PADDING),
            -main_window_area.size.h - f64::from(PADDING),
        )
}

fn get_top_right(main_window_area: Rectangle<f64, Logical>) -> Point<f64, Logical> {
    main_window_area.loc
        + logical(
            main_window_area.size.w + f64::from(PADDING),
            -main_window_area.size.h - f64::from(PADDING),
        )
}

fn get_bottom_left(main_window_area: Rectangle<f64, Logical>) -> Point<f64, Logical> {
    main_window_area.loc
        + logical(
            -main_window_area.size.w - f64::from(PADDING),
            main_window_area.size.h + f64::from(PADDING),
        )
}

fn get_bottom_right(main_window_area: Rectangle<f64, Logical>) -> Point<f64, Logical> {
    main_window_area.loc
        + logical(
            main_window_area.size.w + f64::from(PADDING),
            main_window_area.size.h + f64::from(PADDING),
        )
}

pub fn get_position(
    main_window_area: Rectangle<f64, Logical>,
    pos: Point<f64, Logical>,
) -> Point<f64, Logical> {
    let delta = pos - main_window_area.loc;
    logical(
        delta.x / (main_window_area.size.w + f64::from(PADDING)),
        delta.y / (main_window_area.size.h + f64::from(PADDING)),
    )
}

pub fn position_windows(
    s: &mut Smallvil,
    window_areas: WorkspaceAreas<ProgressAndVelocity>,
    main_window_area: Rectangle<f64, Logical>,
    main_window_area_size: Size<i32, Logical>,
) {
    let left_position = clamp_to_line(
        main_window_area.loc,
        get_left(main_window_area),
        window_areas.left_window.progress,
    )
    .to_i32_round();
    let top_position = clamp_to_line(
        main_window_area.loc,
        get_top(main_window_area),
        window_areas.top_window.progress,
    )
    .to_i32_round();
    let right_position = clamp_to_line(
        main_window_area.loc,
        get_right(main_window_area),
        window_areas.right_window.progress,
    )
    .to_i32_round();
    let bottom_position = clamp_to_line(
        main_window_area.loc,
        get_bottom(main_window_area),
        window_areas.bottom_window.progress,
    )
    .to_i32_round();

    let top_left_position = clamp_to_line(
        main_window_area.loc,
        get_top_left(main_window_area),
        window_areas.top_left_window.progress,
    )
    .to_i32_round();
    let top_right_position = clamp_to_line(
        main_window_area.loc,
        get_top_right(main_window_area),
        window_areas.top_right_window.progress,
    )
    .to_i32_round();
    let bottom_left_position = clamp_to_line(
        main_window_area.loc,
        get_bottom_left(main_window_area),
        window_areas.bottom_left_window.progress,
    )
    .to_i32_round();
    let bottom_right_position = clamp_to_line(
        main_window_area.loc,
        get_bottom_right(main_window_area),
        window_areas.bottom_right_window.progress,
    )
    .to_i32_round();

    // TODO: Find a more efficient way to unmap all elements
    while let Option::Some(element) = s.space.elements().last() {
        s.space.unmap_elem(&element.clone());
    }

    fn handle_window(
        space: &mut Space<Window>,
        window: &Window,
        pos: Point<i32, Logical>,
        size: Size<i32, Logical>,
    ) {
        space.map_element(window.clone(), pos, false);
        if window.geometry().size != size {
            let xdg = window.toplevel().unwrap();
            xdg.with_pending_state(|state| {
                state.states.set(xdg_toplevel::State::Resizing);
                state.size = Option::Some(size);
            });
            xdg.send_pending_configure();
        };
    }

    let workspace = &s.workspaces[s.cur_workspace];
    if let Some(ref left) = workspace.left_window {
        handle_window(&mut s.space, left, left_position, main_window_area_size);
    }
    if let Some(ref top) = workspace.top_window {
        handle_window(&mut s.space, top, top_position, main_window_area_size);
    }
    if let Some(ref right) = workspace.right_window {
        handle_window(&mut s.space, right, right_position, main_window_area_size);
    }
    if let Some(ref bottom) = workspace.bottom_window {
        handle_window(&mut s.space, bottom, bottom_position, main_window_area_size);
    }
    if let Some(ref top_left) = workspace.top_left_window {
        handle_window(
            &mut s.space,
            top_left,
            top_left_position,
            main_window_area_size,
        );
    }
    if let Some(ref top_right) = workspace.top_right_window {
        handle_window(
            &mut s.space,
            top_right,
            top_right_position,
            main_window_area_size,
        );
    }
    if let Some(ref bottom_left) = workspace.bottom_left_window {
        handle_window(
            &mut s.space,
            bottom_left,
            bottom_left_position,
            main_window_area_size,
        );
    }
    if let Some(ref bottom_right) = workspace.bottom_right_window {
        handle_window(
            &mut s.space,
            bottom_right,
            bottom_right_position,
            main_window_area_size,
        );
    }
}

fn spawn_command(state: &mut Smallvil, command: &str) {
    info!("Spawning command {}", command);
    let res = std::process::Command::new(command)
        .env("WAYLAND_DISPLAY", state.socket_name.clone()) // TODO: Do not use clone if possible
        .stdout(std::process::Stdio::null())
        .stdin(std::process::Stdio::null())
        .spawn();
    if let Err(e) = res {
        error!("Failed to spawn {}", e);
    }
}

fn is_focussed_window(window: &Option<smithay::desktop::Window>, focussed: &WlSurface) -> bool {
    window
        .as_ref()
        .map(|window| window.toplevel().unwrap().wl_surface() == focussed)
        .unwrap_or(false)
}

fn close_focussed_window(state: &mut Smallvil) {
    let Some(keyboard) = state.seat.get_keyboard() else {
        error!("Failed to get keyboard");
        return;
    };
    let Some(focussed) = keyboard.current_focus() else {
        info!("No window is focussed");
        return;
    };

    // Find the slot of the current workspace that holds the focussed window.
    let workspace = &mut state.workspaces[state.cur_workspace];
    let closed = if is_focussed_window(&workspace.left_window, &focussed) {
        workspace.left_window.take()
    } else if is_focussed_window(&workspace.top_window, &focussed) {
        workspace.top_window.take()
    } else if is_focussed_window(&workspace.right_window, &focussed) {
        workspace.right_window.take()
    } else if is_focussed_window(&workspace.bottom_window, &focussed) {
        workspace.bottom_window.take()
    } else {
        Option::None
    };

    let Some(window) = closed else {
        warn!("The focussed window is not part of the current workspace");
        return;
    };

    info!("Closing focussed window");
    window.toplevel().unwrap().send_close();
    state.space.unmap_elem(&window);

    // The window is going away, so drop the keyboard focus on it.
    keyboard.set_focus(
        state,
        Option::<WlSurface>::None,
        SERIAL_COUNTER.next_serial(),
    );
}

fn handle_key_action(state: &mut Smallvil, action: Action) {
    info!("Handling key action");
    match action {
        Action::SpawnCommand(command) => spawn_command(state, command),
        Action::FocusInDirection(direction) => match state.cur_workspace_state {
            WorkspaceState::Grabbed(_pos) => {}
            WorkspaceState::Normal => {
                let window_areas = WindowAreas::from_position(state.cur_workspace_focussed_window);
                info!("Animating 1");
                state.cur_workspace_state = WorkspaceState::Animating(AnimationInfo {
                    start_time: SystemTime::now(),
                    start_info: window_areas,
                });
                state.cur_workspace_focussed_window = direction
            }
            WorkspaceState::Animating(ref info) => {
                let (window_areas, _finished) =
                    WindowAreas::from_animation(info, state.cur_workspace_focussed_window);
                state.cur_workspace_state = WorkspaceState::Animating(AnimationInfo {
                    start_time: SystemTime::now(),
                    start_info: window_areas,
                });
                state.cur_workspace_focussed_window = direction;
            }
        },
        Action::CloseFocussedWindow => close_focussed_window(state),
        Action::Quit => {
            info!("Quitting");
            state.running.store(false, Ordering::SeqCst);
            state.loop_signal.stop();
        }
        Action::VtSwitch(vt) => {
            info!("Trying to switch to vt {}", vt);
            if let Some(backend) = state.backend_data.as_mut() {
                if let Err(err) = backend.session.change_vt(vt) {
                    error!("Error switching vt: {}", err);
                }
            }
        }
    }
}

impl Smallvil {
    pub fn process_input_event<I: InputBackend>(&mut self, event: InputEvent<I>) {
        match event {
            InputEvent::Keyboard { event, .. } => {
                let serial = SERIAL_COUNTER.next_serial();
                match self.seat.get_keyboard() {
                    Option::None => {
                        error!("Failed to get keybaord")
                    }
                    Option::Some(keyboard) => {
                        let time = Event::time_msec(&event);
                        let state = event.state();
                        let key_code = event.key_code();
                        let parse_key = match state {
                            KeyState::Pressed => parse_pressed_key,
                            KeyState::Released => parse_released_key,
                        };
                        let (filter_result, mods_changed) =
                            keyboard.input_intercept(self, key_code, state, parse_key);
                        if let FilterResult::Intercept(action) = filter_result {
                            handle_key_action(self, action);
                        } else {
                            keyboard.input_forward(
                                self,
                                key_code,
                                state,
                                serial,
                                time,
                                mods_changed,
                            )
                        }
                    }
                }
            }
            InputEvent::PointerMotion { event, .. } => {
                let serial = SERIAL_COUNTER.next_serial();

                let pointer = self.seat.get_pointer().unwrap();
                let location = self.clamp_coords(pointer.current_location() + event.delta());
                let under = self.surface_under(location);

                self.update_cur_monitor(location);

                pointer.motion(
                    self,
                    under,
                    &MotionEvent {
                        location,
                        serial,
                        time: event.time_msec(),
                    },
                );
                pointer.frame(self);
            }
            InputEvent::PointerMotionAbsolute { event, .. } => {
                let serial = SERIAL_COUNTER.next_serial();

                let max_x = self.space.outputs().fold(0, |acc, o| {
                    acc + self.space.output_geometry(o).unwrap().size.w
                });
                let max_h_output = self
                    .space
                    .outputs()
                    .max_by_key(|o| self.space.output_geometry(o).unwrap().size.h);
                let max_y = max_h_output
                    .and_then(|o| self.space.output_geometry(o).map(|geo| geo.size.h))
                    .unwrap_or(0);

                let location = self
                    .clamp_coords((event.x_transformed(max_x), event.y_transformed(max_y)).into());

                let pointer = self.seat.get_pointer().unwrap();
                let under = self.surface_under(location);

                self.update_cur_monitor(location);

                pointer.motion(
                    self,
                    under,
                    &MotionEvent {
                        location,
                        serial,
                        time: event.time_msec(),
                    },
                );
                pointer.frame(self);
            }
            InputEvent::PointerButton { event, .. } => {
                let pointer = self.seat.get_pointer().unwrap();
                let keyboard = self.seat.get_keyboard().unwrap();

                let serial = SERIAL_COUNTER.next_serial();

                let button = event.button_code();

                let button_state = event.state();

                if ButtonState::Pressed == button_state && !pointer.is_grabbed() {
                    if let Some((window, _loc)) = self
                        .space
                        .element_under(pointer.current_location())
                        .map(|(w, l)| (w.clone(), l))
                    {
                        self.space.raise_element(&window, true);
                        keyboard.set_focus(
                            self,
                            Some(window.toplevel().unwrap().wl_surface().clone()),
                            serial,
                        );
                        self.space.elements().for_each(|window| {
                            window.toplevel().unwrap().send_pending_configure();
                        });
                    } else {
                        self.space.elements().for_each(|window| {
                            window.set_activated(false);
                            window.toplevel().unwrap().send_pending_configure();
                        });
                        keyboard.set_focus(self, Option::<WlSurface>::None, serial);
                    }
                };

                pointer.button(
                    self,
                    &ButtonEvent {
                        button,
                        state: button_state,
                        serial,
                        time: event.time_msec(),
                    },
                );
                pointer.frame(self);
            }
            InputEvent::PointerAxis { event, .. } => {
                let source = event.source();

                let horizontal_amount = event.amount(Axis::Horizontal).unwrap_or_else(|| {
                    event.amount_v120(Axis::Horizontal).unwrap_or(0.0) * 15.0 / 120.
                });
                let vertical_amount = event.amount(Axis::Vertical).unwrap_or_else(|| {
                    event.amount_v120(Axis::Vertical).unwrap_or(0.0) * 15.0 / 120.
                });
                let horizontal_amount_discrete = event.amount_v120(Axis::Horizontal);
                let vertical_amount_discrete = event.amount_v120(Axis::Vertical);

                let mut frame = AxisFrame::new(event.time_msec()).source(source);
                if horizontal_amount != 0.0 {
                    frame = frame.value(Axis::Horizontal, horizontal_amount);
                    if let Some(discrete) = horizontal_amount_discrete {
                        frame = frame.v120(Axis::Horizontal, discrete as i32);
                    }
                }
                if vertical_amount != 0.0 {
                    frame = frame.value(Axis::Vertical, vertical_amount);
                    if let Some(discrete) = vertical_amount_discrete {
                        frame = frame.v120(Axis::Vertical, discrete as i32);
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

            InputEvent::GestureSwipeBegin { event } => self.on_gesture_swipe_begin::<I>(event),
            InputEvent::GestureSwipeUpdate { event } => self.on_gesture_swipe_update::<I>(event),
            InputEvent::GestureSwipeEnd { event } => self.on_gesture_swipe_end::<I>(event),

            _ => {}
        }
    }

    fn clamp_coords(&self, pos: Point<f64, Logical>) -> Point<f64, Logical> {
        if self.space.outputs().next().is_none() {
            return pos;
        }

        let (pos_x, pos_y) = pos.into();
        let max_x = self.space.outputs().fold(0, |acc, o| {
            acc + self.space.output_geometry(o).unwrap().size.w
        });
        let clamped_x = pos_x.clamp(0.0, max_x as f64);
        let max_y = self
            .space
            .outputs()
            .find(|o| {
                let geo = self.space.output_geometry(o).unwrap();
                geo.contains((clamped_x as i32, 0))
            })
            .map(|o| self.space.output_geometry(o).unwrap().size.h);

        if let Some(max_y) = max_y {
            let clamped_y = pos_y.clamp(0.0, max_y as f64);
            (clamped_x, clamped_y).into()
        } else {
            (clamped_x, pos_y).into()
        }
    }

    fn on_gesture_swipe_begin<I: InputBackend>(&mut self, _event: I::GestureSwipeBeginEvent) {
        info!("Gesture swipe begin event");
        self.cur_workspace_state = WorkspaceState::Grabbed(logical(0.0, 0.0));
    }

    fn on_gesture_swipe_update<I: InputBackend>(&mut self, event: I::GestureSwipeUpdateEvent) {
        info!("Gesture swipe event");
        // info!("Gesture swipe event pos: {}, delta: {}", self.pos, event.delta());
        let WorkspaceState::Grabbed(old_pos) = self.cur_workspace_state else {
            error!("Expected state to be grabbed");
            return;
        };
        let delta = event.delta();
        let new_pos = old_pos - logical(delta.x * 3.0, delta.y * 3.0);
        self.cur_workspace_state = WorkspaceState::Grabbed(new_pos);
    }

    fn on_gesture_swipe_end<I: InputBackend>(&mut self, _event: I::GestureSwipeEndEvent) {
        info!("Gesture swipe end event");
        let WorkspaceState::Grabbed(grab_delta) = self.cur_workspace_state else {
            error!("Expected state to be grabbed");
            return;
        };
        let Option::Some((area, _)) = self.main_window_area() else {
            return;
        };
        let pos = get_position(area, grab_delta);
        let start_info = WindowAreas::from_gesture(self.cur_workspace_focussed_window, pos);
        (_, self.cur_workspace_focussed_window) = start_info.min_progress();
        info!("Animating 3");
        self.cur_workspace_state = WorkspaceState::Animating(AnimationInfo {
            start_time: SystemTime::now(),
            start_info: start_info,
        });
    }
}
