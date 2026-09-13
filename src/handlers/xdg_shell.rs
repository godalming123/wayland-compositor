use std::time::SystemTime;

use smithay::{
    delegate_xdg_shell,
    desktop::{
        find_popup_root_surface, get_popup_toplevel_coords, PopupKind, PopupManager, Space, Window,
    },
    reexports::wayland_server::protocol::{wl_seat, wl_surface::WlSurface},
    utils::Serial,
    wayland::{
        compositor::with_states,
        shell::xdg::{
            PopupSurface, PositionerState, ToplevelSurface, XdgShellHandler, XdgShellState,
            XdgToplevelSurfaceData,
        },
    },
};
use tracing::info;

use crate::{
    state::{AnimationInfo, WindowAreas, WindowPosition, WorkspaceState, EMPTY_WORKSPACE},
    Smallvil,
};

impl XdgShellHandler for Smallvil {
    fn xdg_shell_state(&mut self) -> &mut XdgShellState {
        &mut self.xdg_shell_state
    }

    fn new_toplevel(&mut self, surface: ToplevelSurface) {
        let window = Window::new_wayland_window(surface);
        let workspace = &mut self.workspaces[self.cur_workspace];
        if workspace.top_left_window.is_none() {
            workspace.top_left_window = Some(window.clone());
            self.focus_in_direction(WindowPosition::TopLeft);
        } else if workspace.top_window.is_none() {
            workspace.top_window = Some(window.clone());
            self.focus_in_direction(WindowPosition::Top);
        } else if workspace.top_right_window.is_none() {
            workspace.top_right_window = Some(window.clone());
            self.focus_in_direction(WindowPosition::TopRight);
        } else if workspace.right_window.is_none() {
            workspace.right_window = Some(window.clone());
            self.focus_in_direction(WindowPosition::Right);
        } else if workspace.bottom_right_window.is_none() {
            workspace.bottom_right_window = Some(window.clone());
            self.focus_in_direction(WindowPosition::BottomRight);
        } else if workspace.bottom_window.is_none() {
            workspace.bottom_window = Some(window.clone());
            self.focus_in_direction(WindowPosition::Bottom);
        } else if workspace.bottom_left_window.is_none() {
            workspace.bottom_left_window = Some(window.clone());
            self.focus_in_direction(WindowPosition::BottomLeft);
        } else if workspace.left_window.is_none() {
            workspace.left_window = Some(window.clone());
            self.focus_in_direction(WindowPosition::Left);
        } else {
            self.cur_workspace += 1;
            self.workspaces.insert(self.cur_workspace, EMPTY_WORKSPACE);
            self.workspaces[self.cur_workspace].top_left_window = Some(window.clone());
        }
    }

    fn new_popup(&mut self, surface: PopupSurface, _positioner: PositionerState) {
        self.unconstrain_popup(&surface);
        let _ = self.popups.track_popup(PopupKind::Xdg(surface));
    }

    fn reposition_request(
        &mut self,
        surface: PopupSurface,
        positioner: PositionerState,
        token: u32,
    ) {
        surface.with_pending_state(|state| {
            let geometry = positioner.get_geometry();
            state.geometry = geometry;
            state.positioner = positioner;
        });
        self.unconstrain_popup(&surface);
        surface.send_repositioned(token);
    }

    fn grab(&mut self, _surface: PopupSurface, _seat: wl_seat::WlSeat, _serial: Serial) {
        // TODO popup grabs
    }
}

// Xdg Shell
delegate_xdg_shell!(Smallvil);

/// Should be called on `WlSurface::commit`
pub fn handle_commit(popups: &mut PopupManager, space: &Space<Window>, surface: &WlSurface) {
    // Handle toplevel commits.
    if let Some(window) = space
        .elements()
        .find(|w| w.toplevel().unwrap().wl_surface() == surface)
        .cloned()
    {
        let initial_configure_sent = with_states(surface, |states| {
            states
                .data_map
                .get::<XdgToplevelSurfaceData>()
                .unwrap()
                .lock()
                .unwrap()
                .initial_configure_sent
        });

        if !initial_configure_sent {
            window.toplevel().unwrap().send_configure();
        }
    }

    // Handle popup commits.
    popups.commit(surface);
    if let Some(popup) = popups.find_popup(surface) {
        match popup {
            PopupKind::Xdg(ref xdg) => {
                if !xdg.is_initial_configure_sent() {
                    // NOTE: This should never fail as the initial configure is always
                    // allowed.
                    xdg.send_configure().expect("initial configure failed");
                }
            }
            PopupKind::InputMethod(ref _input_method) => {}
        }
    }
}

impl Smallvil {
    pub fn focus_in_direction(&mut self, direction: WindowPosition) {
        match self.cur_workspace_state {
            WorkspaceState::Grabbed(_pos) => {}
            WorkspaceState::Normal => {
                let window_areas = WindowAreas::from_position(self.cur_workspace_focussed_window);
                info!("Animating 1");
                self.cur_workspace_state = WorkspaceState::Animating(AnimationInfo {
                    start_time: SystemTime::now(),
                    start_info: window_areas,
                });
                self.cur_workspace_focussed_window = direction
            }
            WorkspaceState::Animating(ref info) => {
                let (window_areas, _finished) =
                    WindowAreas::from_animation(info, self.cur_workspace_focussed_window);
                self.cur_workspace_state = WorkspaceState::Animating(AnimationInfo {
                    start_time: SystemTime::now(),
                    start_info: window_areas,
                });
                self.cur_workspace_focussed_window = direction;
            }
        }
    }

    fn unconstrain_popup(&self, popup: &PopupSurface) {
        let Ok(root) = find_popup_root_surface(&PopupKind::Xdg(popup.clone())) else {
            return;
        };
        let Some(window) = self
            .space
            .elements()
            .find(|w| w.toplevel().unwrap().wl_surface() == &root)
        else {
            return;
        };

        let output = self.space.outputs().next().unwrap();
        let output_geo = self.space.output_geometry(output).unwrap();
        let window_geo = self.space.element_geometry(window).unwrap();

        // The target geometry for the positioner should be relative to its parent's geometry, so
        // we will compute that here.
        let mut target = output_geo;
        target.loc -= get_popup_toplevel_coords(&PopupKind::Xdg(popup.clone()));
        target.loc -= window_geo.loc;

        popup.with_pending_state(|state| {
            state.geometry = state.positioner.get_unconstrained_geometry(target);
        });
    }
}
