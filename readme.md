# Improve the mental model

- [ ] Figure out how workspaces work
- [ ] Figure out how multi monitor support should work
- [ ] Maybe the keybindings to focus one of the eight windows visible should be based on what kind of window your focusing
  - For example, `w` to focus the window position where a web browser goes, `t` to focus the window position where a terminal goes, ETC

# Improve the implementation of the mental model

- [ ] I think it would be better if there are 4 keybindings/trackpad gestures to switch windows within a workspace and you just press the keybinding twice to access some of the windows
- [ ] Maybe windows should continue being added to the workspace forever, and the gesture just gradually becomes more specific
- [ ] Maybe the there should be 2 windows per edge rather than 1 window per edge and 1 window per corner
- [ ] Improve navigating between windows
  - [x] The gesture should be exactly the same regardless of what window you are on
- [ ] Improve the animations
  - [ ] GPU accelerated animations?
  - [ ] There should be a rubber band effect when you pull too far on the trackpad
  - [x] The top middle window should not come down when you switch between the top left window and the top right window
    - Also applies to the bottom windows
  - [x] There should not be any vertical or horizontal gap when you switch between the left and right window or between the top and bottom window
  - [ ] Ideally the windows would never overlap
  - [ ] Ease-in-out animations when you use keybindings
  - [ ] Take into account the inertia when you use the gesture
    - [ ] Inertia effects which window you go to
    - [ ] Inertia effects the initial speed of the animation
- [x] There should be 8 windows rather than 4 per workspace
- [ ] Focus a window when it opens
- [ ] Focus a window when you move to it

# Fixes

- [ ] Fix ghostty not working
- [ ] Fix the weird black bars around firefox
- [ ] Remove a window from it's workspace when it unmaps itself

# Wayland support

- [ ] Support drag icons
- [ ] Support layer shell maybe
