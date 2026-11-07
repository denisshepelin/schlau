use rune::{Color, Family, TextStyle, oklch};
use rune_kit::{Kit, PaletteSeed};

/// Inter, as rune bundles it, and Newsreader for the list's period headings.
pub const FONTS: &[&[u8]] = &[
    include_bytes!("../assets/fonts/Inter-Regular.ttf"),
    include_bytes!("../assets/fonts/Inter-Medium.ttf"),
    include_bytes!("../assets/fonts/Inter-SemiBold.ttf"),
    include_bytes!("../assets/fonts/Newsreader-SemiBold.ttf"),
];

/// The app's colours: a near-black (or paper) base, an indigo accent, and the kit's derived tokens.
pub fn kit(dark: bool) -> Kit {
    let seed = if dark {
        PaletteSeed { bg: oklch(0.17, 0.01, 270.0), fg: oklch(0.93, 0.005, 270.0), accent: oklch(0.66, 0.16, 275.0) }
    } else {
        PaletteSeed { bg: oklch(0.99, 0.003, 270.0), fg: oklch(0.22, 0.01, 270.0), accent: oklch(0.55, 0.19, 275.0) }
    };
    Kit::from_palette(seed)
}

pub fn accent(dark: bool) -> Color {
    if dark { oklch(0.66, 0.16, 275.0) } else { oklch(0.55, 0.19, 275.0) }
}

/// The selected row's fill: the accent, faint enough that text keeps its colour.
pub fn selection(dark: bool) -> Color {
    accent(dark).alpha(if dark { 0.22 } else { 0.14 })
}

/// A period's heading in the conversation list: a serif display face, as a page of headlines.
pub fn period_heading(base: TextStyle) -> TextStyle {
    TextStyle { size: 25.0, weight: 600.0, family: Family::Named("Newsreader"), line_height: 28.0, tracking: -0.2, ..base }
}

/// A sender's avatar fill and the colour of its initials. The hue comes from the name, so a
/// sender keeps their colour everywhere.
pub fn avatar(name: &str, dark: bool) -> (Color, Color) {
    let hue = name.chars().fold(7u32, |h, c| (h.wrapping_mul(31).wrapping_add(c as u32)) % 360) as f32;
    if dark {
        (oklch(0.42, 0.09, hue), oklch(0.95, 0.02, 270.0))
    } else {
        (oklch(0.86, 0.07, hue), oklch(0.32, 0.06, 270.0))
    }
}
