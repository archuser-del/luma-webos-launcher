//! Where the chrome sits: the tabs, the input buttons and the clock.
//!
//! Every layout draws the same pages. A layout only decides where the chrome
//! goes, how far the page content moves out of its way, and which arrow of
//! the remote leads from the content to the chrome and back.
use crate::state::{Direction, Target};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase", from = "String")]
pub enum Layout {
    /// Clock left, tabs in the middle, inputs right, all along the top.
    Classic,
    /// Tabs as an icon rail down the left edge with the inputs at its foot;
    /// the clock moves to the top right.
    Rail,
    /// One glass bar along the bottom: clock, tabs, inputs.
    Dock,
    /// The top bar mirrored: inputs left, clock in the middle, tabs right.
    Flip,
}

impl Default for Layout {
    fn default() -> Self {
        // The existing suite describes the classic bar; each layout's own
        // tests ask for it by name.
        if cfg!(test) {
            Self::Classic
        } else {
            Self::SHIPPED
        }
    }
}

/// Saved settings with a name this build does not know keep working.
impl From<String> for Layout {
    fn from(name: String) -> Self {
        match name.as_str() {
            "classic" => Self::Classic,
            "rail" => Self::Rail,
            "dock" => Self::Dock,
            "flip" => Self::Flip,
            _ => Self::default(),
        }
    }
}

/// Bottom dock: a bar of this height, this far above the screen edge.
pub const DOCK_HEIGHT: f32 = 84.;
pub const DOCK_TOP: f32 = 1080. - 44. - DOCK_HEIGHT;
/// Page content dissolves between these two lines, just above the dock.
pub const DOCK_FADE: [f32; 2] = [DOCK_TOP - 44., DOCK_TOP - 6.];
/// Left rail: its left edge and width; the tab buttons and circles inside.
pub const RAIL_LEFT: f32 = 24.;
pub const RAIL_WIDTH: f32 = 72.;
pub const RAIL_BUTTON: f32 = 56.;
pub const RAIL_GAP: f32 = 6.;

impl Layout {
    /// What a fresh install of this build shows.
    pub const SHIPPED: Self = Self::Classic;
    pub const ALL: [Self; 4] = [Self::Classic, Self::Rail, Self::Dock, Self::Flip];

    pub fn name(self) -> &'static str {
        match self {
            Self::Classic => "Classic",
            Self::Rail => "Rail",
            Self::Dock => "Dock",
            Self::Flip => "Mirror",
        }
    }

    pub fn next(self) -> Self {
        let at = Self::ALL.iter().position(|l| *l == self).unwrap_or(0);
        Self::ALL[(at + 1) % Self::ALL.len()]
    }

    /// How far the page content moves to make room for the chrome.
    pub fn body_offset(self) -> (f32, f32) {
        match self {
            Self::Rail => (40., 0.),
            Self::Dock => (0., -96.),
            Self::Classic | Self::Flip => (0., 0.),
        }
    }

    /// The chrome runs top to bottom rather than left to right.
    pub fn vertical(self) -> bool {
        self == Self::Rail
    }

    /// The arrow that steps to the previous and to the next chrome button.
    pub fn along(self) -> (Direction, Direction) {
        if self.vertical() {
            (Direction::Up, Direction::Down)
        } else {
            (Direction::Left, Direction::Right)
        }
    }

    /// The arrow that leads from the chrome into the page.
    pub fn inward(self) -> Direction {
        match self {
            Self::Classic | Self::Flip => Direction::Down,
            Self::Rail => Direction::Right,
            Self::Dock => Direction::Up,
        }
    }

    /// The arrow that leads from the edge of the page onto the chrome.
    pub fn outward(self) -> Direction {
        match self.inward() {
            Direction::Down => Direction::Up,
            Direction::Up => Direction::Down,
            Direction::Left => Direction::Right,
            Direction::Right => Direction::Left,
        }
    }

    /// Tabs keep the page where it is scrolled to; the top bars scroll it
    /// back up, as the content would otherwise sit under them.
    pub fn keeps_scroll(self) -> bool {
        matches!(self, Self::Rail | Self::Dock)
    }

    /// Page content below this line is hidden: the pointer cannot hit it.
    pub fn pointer_bottom(self, classic: f32) -> f32 {
        match self {
            Self::Dock => classic.min(DOCK_FADE[1]),
            _ => classic,
        }
    }

    /// Lowest line a focused card may reach, in screen coordinates.
    pub fn focus_bottom(self, classic: f32) -> f32 {
        match self {
            Self::Dock => classic.min(DOCK_FADE[0] - 4.),
            _ => classic,
        }
    }
}

/// The parts of Home the remote steps between with Up and Down.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HomeBand {
    /// The music mini-player, while an album plays.
    Mini,
    /// The headline's video caption and the widgets.
    Info,
    /// The Switch to … HDMI buttons.
    Live,
    /// Favorite apps.
    Apps,
    /// Content rows (Jellyfin, Plex, Moonlight …).
    Stacks,
}

pub fn home_band(target: &Target) -> Option<HomeBand> {
    match target {
        Target::MiniCover
        | Target::MiniShuffle
        | Target::MiniPrev
        | Target::MiniPlay
        | Target::MiniNext
        | Target::MiniRepeat => Some(HomeBand::Mini),
        Target::VideoTitle | Target::Widget(_) | Target::WidgetText(_) | Target::WidgetTile(..) => {
            Some(HomeBand::Info)
        }
        Target::LiveInput(_) => Some(HomeBand::Live),
        Target::App(_) => Some(HomeBand::Apps),
        Target::StackCard(..) => Some(HomeBand::Stacks),
        _ => None,
    }
}

/// Home, top to bottom: Favorites at the top under the rail; the stage
/// keeps everything low, above the dock; the mirror keeps the classic order.
pub const RAIL_FAVORITES_TOP: f32 = 150.;
pub const DOCK_FAVORITES_TOP: f32 = 776.;
pub const DOCK_WIDGETS_TOP: f32 = 240.;

impl Layout {
    /// Home's parts in the order the remote meets them going down.
    pub fn home_bands(self) -> [HomeBand; 5] {
        use HomeBand::*;
        match self {
            Self::Rail => [Mini, Live, Apps, Info, Stacks],
            Self::Classic | Self::Dock | Self::Flip => [Mini, Info, Live, Apps, Stacks],
        }
    }
}

/// Tabs, input circles and the TV settings circle.
pub fn is_chrome(target: &Target) -> bool {
    matches!(
        target,
        Target::Nav(_) | Target::Input(_) | Target::TvSettings
    )
}
