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
}
impl Desktop {
    pub const ALL: [Self; 6] = [
        Self::Plasma,
        Self::Gnome,
        Self::Xfce,
        Self::Cinnamon,
        Self::Mate,
        Self::Lxqt,
    ];
    pub fn label(self) -> &'static str {
        match self {
            Self::Plasma => "KDE Plasma",
            Self::Gnome => "GNOME",
            Self::Xfce => "Xfce",
            Self::Cinnamon => "Cinnamon",
            Self::Mate => "MATE",
            Self::Lxqt => "LXQt",
        }
    }
    pub fn option(self) -> &'static str {
        match self {
            Self::Plasma => "services.desktopManager.plasma6",
            Self::Gnome => "services.desktopManager.gnome",
            Self::Xfce => "services.xserver.desktopManager.xfce",
            Self::Cinnamon => "services.xserver.desktopManager.cinnamon",
            Self::Mate => "services.xserver.desktopManager.mate",
            Self::Lxqt => "services.xserver.desktopManager.lxqt",
        }
    }
    pub fn session(self) -> &'static str {
        match self {
            Self::Plasma => "plasma",
            Self::Gnome => "gnome",
            Self::Xfce => "xfce",
            Self::Cinnamon => "cinnamon",
            Self::Mate => "mate",
            Self::Lxqt => "lxqt",
        }
    }
}
