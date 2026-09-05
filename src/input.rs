use std::sync::atomic::Ordering;

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
    utils::{Logical, Point, SERIAL_COUNTER},
};

use crate::state::Smallvil;

/// Possible results of a keyboard action
enum Action {
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
    println!("Parsing keyboard event {:?}", sym.name());
    if modifiers.alt {
        // ctrl+alt+Fx switches to the corresponding virtual terminal
        if modifiers.alt
            && (xkb::KEY_XF86Switch_VT_1..=xkb::KEY_XF86Switch_VT_12).contains(&sym.raw())
        {
            return FilterResult::Intercept(Action::VtSwitch(
                (sym.raw() - xkb::KEY_XF86Switch_VT_1 + 1) as i32,
            ));
        }
        match sym {
            Keysym::Escape => FilterResult::Intercept(Action::Quit),
            Keysym::F => FilterResult::Intercept(Action::SpawnCommand("firefox")),
            Keysym::G => FilterResult::Intercept(Action::SpawnCommand("ghostty")),
            _ => FilterResult::Forward,
        }
    } else {
        FilterResult::Forward
    }
}

fn position_windows(s: &mut Smallvil) {
    let left_position = s.pos;
    let top_position = s.pos;
    let right_position = s.pos;
    let bottom_position = s.pos;

    for workspace in &s.workspaces {
        if let Some(ref left) = workspace.left_window {
            s.space
                .map_element(left.clone(), left_position.to_i32_round(), false);
        }
        if let Some(ref top) = workspace.top_window {
            s.space
                .map_element(top.clone(), top_position.to_i32_round(), false);
        }
        if let Some(ref right) = workspace.right_window {
            s.space
                .map_element(right.clone(), right_position.to_i32_round(), false);
        }
        if let Some(ref bottom) = workspace.bottom_window {
            s.space
                .map_element(bottom.clone(), bottom_position.to_i32_round(), false);
        }
    }
}

fn spawn_command(state: &mut Smallvil, command: &str) {
    println!("Spawning command {}", command);
    let res = std::process::Command::new(command)
        .env("WAYLAND_DISPLAY", state.socket_name.clone()) // TODO: Do not use clone if possible
        .stdout(std::process::Stdio::null())
        .stdin(std::process::Stdio::null())
        .spawn();
    if let Err(e) = res {
        println!("Failed to spawn {}", e);
    }
}

fn handle_key_action(state: &mut Smallvil, action: Action) {
    match action {
        Action::SpawnCommand(command) => spawn_command(state, command),
        Action::Quit => {
            println!("Quitting");
            state.running.store(false, Ordering::SeqCst);
            state.loop_signal.stop();
        }
        Action::VtSwitch(vt) => {
            println!("Trying to switch to vt {}", vt);
            if let Some(backend) = state.backend_data.as_mut() {
                if let Err(err) = backend.session.change_vt(vt) {
                    println!("Error switching vt: {}", err);
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
                        eprintln!("Failed to get keybaord")
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
        println!("Gesture swipe begin event")
    }

    fn on_gesture_swipe_update<I: InputBackend>(&mut self, event: I::GestureSwipeUpdateEvent) {
        self.pos += event.delta();
        println!("Gesture swipe event");
        // println!("Gesture swipe event pos: {}, delta: {}", self.pos, event.delta());

        position_windows(self)
    }

    fn on_gesture_swipe_end<I: InputBackend>(&mut self, _event: I::GestureSwipeEndEvent) {
        println!("Gesture swipe end event")
    }
}
