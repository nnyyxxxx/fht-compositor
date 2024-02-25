pub mod cursor;
pub mod decorations;
pub mod focus_target;
pub mod grabs;
pub mod window;
pub mod workspaces;

use smithay::desktop::{
    find_popup_root_surface, get_popup_toplevel_coords, layer_map_for_output, PopupKind,
    WindowSurfaceType,
};
use smithay::input::pointer::Focus;
use smithay::output::Output;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::reexports::wayland_server::Resource;
use smithay::utils::{Logical, Point, Serial};
use smithay::wayland::seat::WaylandFocus;
use smithay::wayland::shell::wlr_layer::Layer;
use smithay::wayland::shell::xdg::PopupSurface;

pub use self::focus_target::FocusTarget;
use self::grabs::MoveSurfaceGrab;
pub use self::window::FhtWindow;
pub use self::workspaces::FullscreenSurface;
use self::workspaces::{Workspace, WorkspaceSwitchAnimation};
use crate::config::CONFIG;
use crate::state::{Fht, State};
use crate::utils::geometry::{PointExt, PointGlobalExt, RectGlobalExt};
use crate::utils::output::OutputExt;

impl Fht {
    /// Get the [`FocusTarget`] under the cursor.
    ///
    /// It checks the surface under the cursor using the following order:
    /// - [`Overlay`] layer shells.
    /// - [`Fullscreen`] windows on the active workspace.
    /// - [`Top`] layer shells.
    /// - Normal/Maximized windows on the active workspace.
    /// - [`Bottom`] layer shells.
    /// - [`Background`] layer shells.
    pub fn focus_target_under(
        &self,
        point: Point<f64, Logical>,
    ) -> Option<(FocusTarget, Point<i32, Logical>)> {
        let output = self.focus_state.output.as_ref()?;
        let active_ws = self.wset_for(output).active();
        let output_geometry = output.geometry().as_logical();
        let layer_map = layer_map_for_output(output);

        let mut under = None;

        if let Some(layer) = layer_map.layer_under(Layer::Overlay, point) {
            let layer_loc = layer_map.layer_geometry(layer).unwrap().loc;
            under = Some((layer.clone().into(), output_geometry.loc + layer_loc))
        } else if let Some(fullscreen) = active_ws.fullscreen.as_ref().map(|f| &f.inner) {
            under = Some((
                fullscreen.clone().into(),
                output.geometry().loc.as_logical(),
            ))
        } else if let Some(layer) = layer_map.layer_under(Layer::Top, point) {
            let layer_loc = layer_map.layer_geometry(layer).unwrap().loc;
            under = Some((layer.clone().into(), output_geometry.loc + layer_loc))
        } else if let Some((window, loc)) = active_ws.window_under(point) {
            under = Some((window.clone().into(), loc))
        } else if let Some(layer) = layer_map
            .layer_under(Layer::Bottom, point)
            .or_else(|| layer_map.layer_under(Layer::Background, point))
        {
            let layer_loc = layer_map.layer_geometry(layer).unwrap().loc;
            under = Some((layer.clone().into(), output_geometry.loc + layer_loc))
        }

        under
    }

    /// Find the window associated with this [`WlSurface`]
    pub fn find_window(&self, surface: &WlSurface) -> Option<&FhtWindow> {
        self.workspaces()
            .find_map(|(_, wset)| wset.find_window(surface))
    }

    /// Find the window associated with this [`WlSurface`], and the output the window is mapped
    /// onto
    pub fn find_window_and_output(&self, surface: &WlSurface) -> Option<(&FhtWindow, &Output)> {
        self.workspaces()
            .find_map(|(_, wset)| wset.find_window(surface).map(|w| (w, &wset.output)))
    }

    /// Get a reference to the workspace holding this window
    pub fn ws_for(&self, window: &FhtWindow) -> Option<&Workspace> {
        self.workspaces().find_map(|(_, wset)| wset.ws_for(window))
    }

    /// Get a mutable reference to the workspace holding this window
    pub fn ws_mut_for(&mut self, window: &FhtWindow) -> Option<&mut Workspace> {
        self.workspaces_mut()
            .find_map(|(_, wset)| wset.ws_mut_for(window))
    }

    /// Arrange the shell elements.
    ///
    /// This should be called whenever output geometry changes.
    pub fn arrange(&self) {
        self.workspaces().for_each(|(output, wset)| {
            layer_map_for_output(output).arrange();
            wset.arrange();
        });
    }

    /// Find the first output where this [`WlSurface`] is visible.
    ///
    /// This checks everything from layer shells to windows to override redirect windows etc.
    pub fn visible_output_for_surface(&self, surface: &WlSurface) -> Option<&Output> {
        self.outputs()
            .find(|o| {
                // Is the surface a layer shell?
                let layer_map = layer_map_for_output(o);
                layer_map
                    .layer_for_surface(surface, WindowSurfaceType::ALL)
                    .is_some()
            })
            .or_else(|| {
                // Pending layer_surface?
                self.pending_layers.iter().find_map(|(l, output)| {
                    let mut found = false;
                    l.with_surfaces(|s, _| {
                        if s == surface {
                            found = true;
                        }
                    });
                    found.then_some(output)
                })
            })
            .or_else(|| {
                // Mapped window?
                self.workspaces().find_map(|(o, wset)| {
                    let active = wset.active();
                    if active
                        .windows
                        .iter()
                        .any(|w| w.has_surface(surface, WindowSurfaceType::ALL))
                    {
                        return Some(o);
                    }

                    if active
                        .fullscreen
                        .as_ref()
                        .is_some_and(|f| f.inner.has_surface(surface, WindowSurfaceType::ALL))
                    {
                        return Some(o);
                    }

                    None
                })
            })
    }

    /// Find every output where this window (and it's subsurfaces) is displayed.
    pub fn visible_outputs_for_window(&self, window: &FhtWindow) -> impl Iterator<Item = &Output> {
        let window_geo = window.global_geometry();
        self.outputs()
            .filter(move |o| o.geometry().intersection(window_geo).is_some())
    }

    /// Find every window that is curently displayed on this output
    #[profiling::function]
    pub fn visible_windows_for_output(
        &self,
        output: &Output,
    ) -> Box<dyn Iterator<Item = &FhtWindow> + '_> {
        let wset = self.wset_for(output);

        if let Some(WorkspaceSwitchAnimation { target_idx, .. }) = wset.switch_animation.as_ref() {
            let active = wset.active();
            let target = &wset.workspaces[*target_idx];
            if let Some(fullscreen) = active
                .fullscreen
                .as_ref()
                .map(|f| &f.inner)
                .or_else(|| target.fullscreen.as_ref().map(|f| &f.inner))
            {
                return Box::new(std::iter::once(fullscreen))
                    as Box<dyn Iterator<Item = &FhtWindow>>;
            } else {
                return Box::new(active.windows.iter().chain(target.windows.iter()))
                    as Box<dyn Iterator<Item = &FhtWindow>>;
            }
        } else {
            let active = wset.active();
            if let Some(fullscreen) = active.fullscreen.as_ref().map(|f| &f.inner) {
                return Box::new(std::iter::once(fullscreen))
                    as Box<dyn Iterator<Item = &FhtWindow>>;
            } else {
                return Box::new(active.windows.iter()) as Box<dyn Iterator<Item = &FhtWindow>>;
            }
        }
    }

    /// Map a pending window (if found)
    pub fn map_window(&mut self, window: &FhtWindow) {
        let Some(idx) = self.pending_windows.iter().position(|(w, _)| w == window) else {
            warn!("Tried to map an invalid window!");
            return;
        };

        let (window, mut output) = self.pending_windows.remove(idx);
        // TODO: Implement this in user config
        let dummy_settings = WindowMapSettings {
            floating: false,
            fullscreen: false,
            output: None,
            workspace: None,
        };
        window.set_tiled(!dummy_settings.floating);

        let client = self
            .display_handle
            .get_client(window.wl_surface().unwrap().id())
            .unwrap();
        let mut wl_output = None;
        for o in output.client_outputs(&client) {
            wl_output = Some(o);
        }
        window.set_fullscreen(dummy_settings.fullscreen, wl_output);

        if let Some(target_output) = dummy_settings
            .output
            .and_then(|name| self.outputs().find(|o| o.name() == name))
            .cloned()
        {
            output = target_output;
        }

        let wset = self.wset_mut_for(&output);
        let workspace = match dummy_settings.workspace {
            Some(idx) => {
                let idx = idx.clamp(0, 8);
                &mut wset.workspaces[idx]
            }
            None => wset.active_mut(),
        };

        // Fullscreening logic in each workspace:
        //
        // If we insert a new window, then take it out and put it at it's last known idx.
        // If we want another window to be fullscreened, then remove the current one and put the
        // new one there
        if workspace.fullscreen.is_some() {
            let FullscreenSurface {
                inner,
                last_known_idx,
            } = workspace.fullscreen.take().unwrap();
            let last_known_idx = last_known_idx.clamp(0, workspace.windows.len().saturating_sub(1));
            workspace.windows.insert(last_known_idx, inner);
        }

        workspace.insert_window(window.clone());
        if CONFIG.general.focus_new_windows {
            self.focus_state.focus_target = Some(window.into());
        }
    }

    /// Unconstraint a popup.
    ///
    /// Basically changes its geometry and location so that it doesn't overflow outside of the
    /// parent window's output.
    pub fn unconstrain_popup(&self, popup: &PopupSurface) {
        let Ok(root) = find_popup_root_surface(&PopupKind::Xdg(popup.clone())) else {
            return;
        };
        let Some(window) = self.find_window(&root) else {
            return;
        };

        let mut outputs_for_window = self.visible_outputs_for_window(window);
        if outputs_for_window.next().is_none() {
            return;
        }

        let mut outputs_geo = outputs_for_window.next().unwrap().geometry();
        for output in outputs_for_window {
            outputs_geo = outputs_geo.merge(output.geometry());
        }

        // The target (aka the popup) geometry should be relative to the parent (aka the window's)
        // geometry, based on the xdg_shell protocol requirements.
        let mut target = outputs_geo;
        target.loc -= get_popup_toplevel_coords(&PopupKind::Xdg(popup.clone())).as_global();
        target.loc -= window.global_geometry().loc;

        popup.with_pending_state(|state| {
            state.geometry = state
                .positioner
                .get_unconstrained_geometry(target.as_logical());
        });
    }
}

impl State {
    /// Process a move request for this given window.
    pub fn handle_move_request(&mut self, window: FhtWindow, serial: Serial) {
        // NOTE: About internal handling.
        // ---
        // Even though `XdgShellHandler::move_request` has a seat argument, we only advertise one
        // single seat to clients (why would we support multi-seat for a standalone compositor?)
        // So the only pointer we have is the advertised seat pointer.
        let pointer = self.fht.pointer.clone();
        if !pointer.has_grab(serial) {
            return;
        }
        let Some(start_data) = pointer.grab_start_data() else {
            return;
        };

        let Some(wl_surface) = window.wl_surface() else {
            return;
        };
        // Make sure we are moving the same window
        if start_data.focus.is_none()
            || !start_data
                .focus
                .as_ref()
                .unwrap()
                .0
                .same_client_as(&wl_surface.id())
        {
            return;
        }

        let window_geo = window.global_geometry();
        let mut initial_window_location = window_geo.loc;

        // Unmaximize/Unfullscreen if it already is.
        let is_maximized = window.is_maximized();
        let is_fullscreen = window.is_fullscreen();
        if is_maximized || is_fullscreen {
            window.set_maximized(false);
            window.set_fullscreen(false, None);
            if let Some(toplevel) = window.0.toplevel() {
                toplevel.send_configure();
            }

            // let pos = pointer.current_location().as_global();
            // let mut window_pos = pos - window_geo.to_f64().loc;
            // window_pos.x = window_pos.x.clamp(0.0, window_geo.size.w.to_f64());
            //
            // match window_pos.x / window_geo.size.w.to_f64() {
            //     x if x < 0.5
            // }
            let pos = pointer.current_location();
            initial_window_location = (pos.x as i32, pos.y as i32).into();
        }

        window.set_fullscreen(false, None);

        let grab = MoveSurfaceGrab {
            start_data,
            window,
            initial_window_location,
        };

        pointer.set_grab(self, grab, serial, Focus::Clear);
    }
}

/// Initial settings/state for a window when mapping it
struct WindowMapSettings {
    /// Should the window be floating?
    floating: bool,
    /// Should the window be fullscreen?
    fullscreen: bool,
    /// On which output should we map the window?
    output: Option<String>,
    /// On which specific workspace of the output should we map the window?
    workspace: Option<usize>,
}
