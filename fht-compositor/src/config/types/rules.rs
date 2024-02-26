use serde::{Deserialize, Serialize};

use crate::shell::FhtWindow;

const fn default_true() -> bool {
    true
}

#[derive(Default, Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct WindowRulePattern {
    /// The workspace index the window is getting spawned on.
    #[serde(default)]
    workspace: Option<usize>,

    /// The window title regex to match on
    ///
    /// NOTE: The compositor checks before for a title since it's more specific than an app id.
    #[serde(default)]
    title: Option<String>,

    /// The app id regex to match on.
    ///
    /// This is commonly known as the window CLASS, or WM_CLASS on X.org
    #[serde(default)]
    app_id: Option<String>,
}

impl WindowRulePattern {
    pub fn matches(&self, window: &FhtWindow, workspace: usize) -> bool {
        if let Some(&workspace_idx) = self.workspace.as_ref() {
            workspace_idx == workspace
        } else if let Some(title) = self.title.as_ref() {
            &window.title() == title
        } else if let Some(app_id) = self.app_id.as_ref() {
            &window.app_id() == app_id
        } else {
            false
        }
    }
}

/// Initial settings/state for a window when mapping it
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WindowMapSettings {
    /// Should the window be floating?
    #[serde(default)]
    pub floating: bool,

    /// Should the window be fullscreen?
    #[serde(default)]
    pub fullscreen: bool,

    /// If the window is floating, should we center it?
    #[serde(default = "default_true")]
    pub centered: bool,

    /// On which output should we map the window?
    pub output: Option<String>,

    /// On which specific workspace of the output should we map the window?
    ///
    /// NOTE: This is the workspace *index*
    pub workspace: Option<usize>,
}

impl Default for WindowMapSettings {
    fn default() -> Self {
        Self {
            floating: false,
            fullscreen: false,
            centered: true,
            output: None,
            workspace: None,
        }
    }
}
