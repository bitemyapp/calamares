// SPDX-License-Identifier: GPL-3.0-or-later
use serde::{Deserialize, Serialize};

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
