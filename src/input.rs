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
    input::{
        keyboard::{keysyms as xkb, FilterResult, Keysym},
        pointer::{AxisFrame, ButtonEvent, MotionEvent},
    },
    reexports::wayland_server::protocol::wl_surface::WlSurface,
    utils::{Logical, Point, Rectangle, SERIAL_COUNTER},
};
use tracing::{error, info, warn};

use crate::state::{get_pos_and_velocity, AnimationInfo, Smallvil, WindowPosition, WorkspaceState};

/// Possible results of a keyboard action
enum Action {
    FocusInDirection(WindowPosition),
    SpawnCommand(&'static str), // Spawn a command
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
            Keysym::h => FilterResult::Intercept(Action::FocusInDirection(WindowPosition::Left)),
            Keysym::j => FilterResult::Intercept(Action::FocusInDirection(WindowPosition::Bottom)),
            Keysym::k => FilterResult::Intercept(Action::FocusInDirection(WindowPosition::Top)),
            Keysym::l => FilterResult::Intercept(Action::FocusInDirection(WindowPosition::Right)),
            _ => FilterResult::Forward,
        }
    } else {
        FilterResult::Forward
    }
}

pub const fn logical<T>(x: T, y: T) -> Point<T, Logical> {
    return Point::<T, Logical>::new(x, y);
}

const PADDING: Point<i32, Logical> = logical(50, 50);

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

fn distance_from_points(a: Point<f64, Logical>, b: Point<f64, Logical>) -> f64 {
    distance_from_delta(a.x - b.x, a.y - b.y)
}

fn distance_from_delta(delta_x: f64, delta_y: f64) -> f64 {
    return (delta_x * delta_x + delta_y * delta_y).sqrt();
}

fn clamp_to_line(
    line_start: Point<f64, Logical>,
    line_end: Point<f64, Logical>,
    point_to_clamp: Point<f64, Logical>,
) -> Point<f64, Logical> {
    let delta_y = line_start.y - line_end.y;
    let delta_x = line_start.x - line_end.x;
    let clamped_to_line = if delta_y == 0.0 {
        logical(point_to_clamp.x, line_start.y)
    } else if delta_x == 0.0 {
        logical(line_start.x, point_to_clamp.y)
    } else {
        let gradiant0 = delta_y / delta_x;
        let gradient1 = -1.0 / gradiant0;
        get_line_intersection(
            gradiant0,
            get_y_intercept(gradiant0, line_start),
            gradient1,
            get_y_intercept(gradient1, point_to_clamp),
        )
    };
    let (left, right, top, bottom) = get_left_right_top_bottom_for_points(line_start, line_end);
    if clamped_to_line.x < left.x {
        clamp_to_point(left, clamped_to_line, distance_from_delta(delta_x, delta_y))
    } else if clamped_to_line.x > right.x {
        clamp_to_point(
            right,
            clamped_to_line,
            distance_from_delta(delta_x, delta_y),
        )
    } else if clamped_to_line.y < top.y {
        clamp_to_point(top, clamped_to_line, distance_from_delta(delta_x, delta_y))
    } else if clamped_to_line.y > bottom.y {
        clamp_to_point(
            bottom,
            clamped_to_line,
            distance_from_delta(delta_x, delta_y),
        )
    } else {
        clamped_to_line
    }
}

fn get_left_top_right_bottom(
    output_geometry: Rectangle<i32, Logical>,
) -> (
    Point<f64, Logical>,
    Point<f64, Logical>,
    Point<f64, Logical>,
    Point<f64, Logical>,
) {
    let left_pos = (output_geometry.loc + logical(-output_geometry.size.w, 0) + PADDING).to_f64();
    let top_pos = (output_geometry.loc + logical(0, -output_geometry.size.h) + PADDING).to_f64();
    let right_pos = (output_geometry.loc + logical(output_geometry.size.w, 0) + PADDING).to_f64();
    let bottom_pos = (output_geometry.loc + logical(0, output_geometry.size.h) + PADDING).to_f64();

    (left_pos, top_pos, right_pos, bottom_pos)
}

pub fn get_pos(
    output_geoemetry: Rectangle<i32, Logical>,
    direction: &WindowPosition,
) -> Point<f64, Logical> {
    let (left, top, right, bottom) = get_left_top_right_bottom(output_geoemetry);
    match direction {
        WindowPosition::Left => left,
        WindowPosition::Top => top,
        WindowPosition::Right => right,
        WindowPosition::Bottom => bottom,
    }
}

fn position_windows(s: &mut Smallvil, pos: Point<f64, Logical>) {
    let output_geometry = match s.focussed_output_geometry() {
        Option::Some(o) => o,
        Option::None => {
            warn!("Failed to get output geometry");
            return;
        }
    };
    let center_pos = (output_geometry.loc + PADDING).to_f64();
    let (left_pos, top_pos, right_pos, bottom_pos) = get_left_top_right_bottom(output_geometry);

    let left_position = clamp_to_line(center_pos, left_pos, pos).to_i32_round();
    let top_position = clamp_to_line(center_pos, top_pos, pos).to_i32_round();
    let right_position = clamp_to_line(center_pos, right_pos, pos).to_i32_round();
    let bottom_position = clamp_to_line(center_pos, bottom_pos, pos).to_i32_round();

    // TODO: Find a more efficient way to unmap all elements
    while let Option::Some(element) = s.space.elements().last() {
        s.space.unmap_elem(&element.clone());
    }

    let workspace = &s.workspaces[s.cur_workspace];
    if let Some(ref left) = workspace.left_window {
        s.space.map_element(left.clone(), left_position, false);
    }
    if let Some(ref top) = workspace.top_window {
        s.space.map_element(top.clone(), top_position, false);
    }
    if let Some(ref right) = workspace.right_window {
        s.space.map_element(right.clone(), right_position, false);
    }
    if let Some(ref bottom) = workspace.bottom_window {
        s.space.map_element(bottom.clone(), bottom_position, false);
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

fn handle_key_action(state: &mut Smallvil, action: Action) {
    match action {
        Action::SpawnCommand(command) => spawn_command(state, command),
        Action::FocusInDirection(direction) => {
            let Option::Some(output_geometry) = state.focussed_output_geometry() else {
                error!("Failed to get focussed output geometry");
                return;
            };
            match state.cur_workspace_state {
                WorkspaceState::Grabbed(_pos) => {}
                WorkspaceState::WindowFocussed(ref window) => {
                    let pos = get_pos(output_geometry, window);
                    state.cur_workspace_state = WorkspaceState::Animating(AnimationInfo {
                        start_time: SystemTime::now(),
                        start_pos: pos,
                        start_velocity: logical(0.0, 0.0),
                        end_pos: direction,
                    });
                }
                WorkspaceState::Animating(ref info) => {
                    let (pos, velocity) = get_pos_and_velocity(info, output_geometry);
                    state.cur_workspace_state = WorkspaceState::Animating(AnimationInfo {
                        start_time: SystemTime::now(),
                        start_pos: pos,
                        start_velocity: velocity,
                        end_pos: direction,
                    })
                }
            }
        }
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
        let new_pos = old_pos + event.delta();
        self.cur_workspace_state = WorkspaceState::Grabbed(new_pos);
        position_windows(self, new_pos);
    }

    fn on_gesture_swipe_end<I: InputBackend>(&mut self, _event: I::GestureSwipeEndEvent) {
        let WorkspaceState::Grabbed(pos) = self.cur_workspace_state else {
            error!("Expected state to be grabbed");
            return;
        };
        let (left, top, right, bottom) =
            get_left_top_right_bottom(self.focussed_output_geometry().unwrap());
        let left_distance = distance_from_points(pos, left);
        let top_distance = distance_from_points(pos, top);
        let right_distance = distance_from_points(pos, right);
        let bottom_distance = distance_from_points(pos, bottom);
        let end_pos = if left_distance <= top_distance
            && left_distance <= right_distance
            && left_distance <= bottom_distance
        {
            WindowPosition::Left
        } else if top_distance <= right_distance && top_distance <= bottom_distance {
            WindowPosition::Top
        } else if right_distance <= bottom_distance {
            WindowPosition::Right
        } else {
            WindowPosition::Bottom
        };
        self.cur_workspace_state = WorkspaceState::Animating(AnimationInfo {
            start_time: SystemTime::now(),
            start_pos: pos,
            start_velocity: logical(0.0, 0.0), // TODO: Put in proper velocity
            end_pos: end_pos,
        });
        info!("Gesture swipe end event")
    }
}
