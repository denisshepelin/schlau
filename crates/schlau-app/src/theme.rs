use rune::{Color, oklch};
use rune_kit::{Kit, PaletteSeed};

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
