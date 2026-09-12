# What the project is

A wayland compositor built in smithay 0.7.0 (crates.io), forked from the anvil example, with a custom workspace model.

# Project architecture

- Logs are appended to `/tmp/wayland_compositor_logs`
- Custom workspace model:
  - State store in the `WorkspaceState` enum
  - 8 window slots around the focused output
  - Pan via gesture swipes
  - `update_workspace_position()` must run every render frame (both backends call it)
- `smithay-drm-extras` is used with `default-features = false` because the system `libdisplay-info` is `0.3.0` (crate needs `<0.3.0`) — so EDID make/model are hardcoded strings.

# Agent instructions

- If any of the information in the `Project architecture` heading changes, please inform the user and suggest that they update this section
- Let the user run the project to test behavior, rather than running the project yourself
