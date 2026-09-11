use std::time::SystemTime;

use smithay::utils::{Coordinate, Logical, Point};

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

pub type AnvilWorkspace = WorkspaceAreas<Option<smithay::desktop::Window>>;
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
