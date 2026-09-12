use std::time::SystemTime;

use smithay::{
    desktop::{Space, Window},
    reexports::wayland_protocols::xdg::shell::server::xdg_toplevel,
    utils::{Coordinate, Logical, Point, Rectangle, Size},
};
use tracing::error;

use crate::{shell::WindowElement, state::Backend, AnvilState};

#[derive(Debug)]
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

pub type AnvilWorkspace = WorkspaceAreas<Option<WindowElement>>;
pub type WindowAreas = WorkspaceAreas<ProgressAndVelocity>;

pub const EMPTY_WORKSPACE: AnvilWorkspace = AnvilWorkspace {
    left_window: Option::None,
    top_window: Option::None,
    right_window: Option::None,
    bottom_window: Option::None,

    top_left_window: Option::None,
    top_right_window: Option::None,
    bottom_left_window: Option::None,
    bottom_right_window: Option::None,
};

#[derive(Debug, Copy, Clone)]
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

pub fn distance(a: Point<f64, Logical>, b: Point<f64, Logical>) -> f64 {
    let delta_x = a.x - b.x;
    let delta_y = a.y - b.y;
    (delta_x * delta_x + delta_y * delta_y).sqrt()
}

pub const fn logical<T>(x: T, y: T) -> Point<T, Logical> {
    return Point::<T, Logical>::new(x, y);
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

#[derive(Debug)]
pub struct AnimationInfo {
    pub start_time: SystemTime,
    pub start_info: WorkspaceAreas<ProgressAndVelocity>,
}

const ANIMATION_MS: f64 = 300.0;

#[derive(Debug, Copy, Clone)]
pub struct ProgressAndVelocity {
    pub progress: f64, // 0.0 - 1.0
    velocity: f64,     // TODO
}

#[derive(Debug)]
pub enum WorkspaceState {
    Normal,
    Animating(AnimationInfo),
    Grabbed(Point<f64, Logical>),
}

const fn between(
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
    // info!(
    //     "Portion along: {}, restricted portion along: {}",
    //     portion_along, restricted_portion_along
    // );
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

pub const PADDING: i32 = 10;
pub const MARGIN: i32 = 50;
pub const MARGIN_POS: Point<i32, Logical> = logical(MARGIN, MARGIN);

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

pub fn position_windows<B: Backend>(
    s: &mut AnvilState<B>,
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
        space: &mut Space<WindowElement>,
        window: &WindowElement,
        pos: Point<i32, Logical>,
        size: Size<i32, Logical>,
    ) {
        let geometry = window.0.geometry();
        if geometry.loc != pos {
            space.map_element(window.clone(), pos, false);
        };
        if geometry.size != size {
            let xdg = window.0.toplevel().unwrap();
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
