// SPDX-License-Identifier: GPL-3.0-or-later
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};

/// A nonempty, unique, compatible selection whose default is a member.
/// The order is retained so configuration output remains stable.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DesktopSelection {
    selected: Vec<Desktop>,
    default: Desktop,
}
impl DesktopSelection {
    pub fn parse(selected: Vec<Desktop>, default: Desktop) -> Result<Self> {
        ensure!(
            !selected.is_empty() && selected.len() <= Desktop::ALL.len(),
            "Select at least one desktop environment"
        );
        for (index, desktop) in selected.iter().enumerate() {
            ensure!(
                !selected[..index].contains(desktop),
                "Duplicate desktop selection"
            );
        }
        ensure!(
            !(selected.contains(&Desktop::Gnome) && selected.contains(&Desktop::Cinnamon)),
            "GNOME and Cinnamon cannot currently be combined: their pinned NixOS modules conflict on GSettings overrides. Select one of those two; other desktops can be combined."
        );
        ensure!(
            selected.contains(&default),
            "The default session must be a selected desktop"
        );
        Ok(Self { selected, default })
    }
    pub fn selected(&self) -> &[Desktop] {
        &self.selected
    }
    pub fn default(&self) -> Desktop {
        self.default
    }
    pub fn contains(&self, desktop: Desktop) -> bool {
        self.selected.contains(&desktop)
    }
    pub(crate) fn into_raw(self) -> (Vec<Desktop>, Desktop) {
        (self.selected, self.default)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Desktop {
    Plasma,
    Gnome,
    Xfce,
    Cinnamon,
    Mate,
    Lxqt,
    Hyprland,
    /// Hyprland with an Omarchy-style configuration, as its own login session.
    Omarchy,
}
impl Desktop {
    pub const ALL: [Self; 8] = [
        Self::Plasma,
        Self::Gnome,
        Self::Xfce,
        Self::Cinnamon,
        Self::Mate,
        Self::Lxqt,
        Self::Hyprland,
        Self::Omarchy,
    ];
    pub fn label(self) -> &'static str {
        match self {
            Self::Plasma => "KDE Plasma",
            Self::Gnome => "GNOME",
            Self::Xfce => "Xfce",
            Self::Cinnamon => "Cinnamon",
            Self::Mate => "MATE",
            Self::Lxqt => "LXQt",
            Self::Hyprland => "Hyprland",
            Self::Omarchy => "Omarchy-style Hyprland",
        }
    }
    pub fn description(self) -> &'static str {
        match self {
            Self::Plasma => "Full-featured and familiar, with deep customization.",
            Self::Gnome => "Focused, modern workflow built around the Activities overview.",
            Self::Xfce => "Lightweight and traditional; easy on older hardware.",
            Self::Cinnamon => "Classic layout with a polished, modern feel.",
            Self::Mate => "The traditional GNOME 2 desktop, steady and simple.",
            Self::Lxqt => "Very lightweight Qt desktop.",
            Self::Hyprland => {
                "Dynamic tiling Wayland compositor with its upstream default configuration."
            }
            Self::Omarchy => {
                "Keyboard-driven Hyprland in the style of Omarchy: Waybar, Walker, Mako and themes."
            }
        }
    }
    /// Value of `calamares.desktops` in the installed configuration.
    pub fn id(self) -> &'static str {
        match self {
            Self::Plasma => "plasma",
            Self::Gnome => "gnome",
            Self::Xfce => "xfce",
            Self::Cinnamon => "cinnamon",
            Self::Mate => "mate",
            Self::Lxqt => "lxqt",
            Self::Hyprland => "hyprland",
            Self::Omarchy => "omarchy",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ids_match_serde_names_and_hyprland_flavors_combine() {
        for desktop in Desktop::ALL {
            assert_eq!(
                serde_json::to_string(&desktop).unwrap(),
                format!("\"{}\"", desktop.id())
            );
        }
        let both =
            DesktopSelection::parse(vec![Desktop::Hyprland, Desktop::Omarchy], Desktop::Omarchy)
                .unwrap();
        assert_eq!(both.default(), Desktop::Omarchy);
        assert!(
            DesktopSelection::parse(vec![Desktop::Gnome, Desktop::Cinnamon], Desktop::Gnome)
                .is_err()
        );
        assert!(DesktopSelection::parse(vec![Desktop::Hyprland], Desktop::Plasma).is_err());
    }
}
