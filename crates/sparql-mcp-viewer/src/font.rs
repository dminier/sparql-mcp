//! The viewer's UI font.
//!
//! Bevy's built-in default is `FiraMono-subset`: a monospace subset with no
//! accents, which is hard to read at small sizes and drops characters in
//! French labels. This plugin replaces it, for **all** viewer text, with
//! Inter, a sans-serif designed for screens. Inter is a variable font, so one
//! file gives every weight; `TextFont::weight` picks one (labels use
//! semi-bold, which holds up better than regular on a dark background).
//!
//! Inter is licensed under the SIL Open Font License 1.1; the license text is
//! in `assets/fonts/OFL.txt`, next to the font.

use bevy::prelude::*;
use bevy::text::Font;

/// Inter, variable (weight and optical size axes), embedded in the binary.
static INTER_VARIABLE: &[u8] = include_bytes!("../assets/fonts/Inter-Variable.ttf");

pub struct FontPlugin;

impl Plugin for FontPlugin {
    fn build(&self, app: &mut App) {
        // `Assets<Font>` exists once the text plugin is added; without it
        // (a headless or test app) there is no text to style.
        let Some(mut fonts) = app.world_mut().get_resource_mut::<Assets<Font>>() else {
            return;
        };
        // Text that names no font uses the default handle, so putting Inter
        // there restyles every label, panel and feed at once.
        let _ = fonts.insert(
            Handle::<Font>::default().id(),
            Font::from_bytes(INTER_VARIABLE.to_vec()),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_embedded_font_is_a_truetype_file_with_a_weight_axis() {
        // `00 01 00 00` is the TrueType magic; `fvar` is the variation table.
        assert_eq!(&INTER_VARIABLE[..4], &[0, 1, 0, 0]);
        assert!(
            INTER_VARIABLE.windows(4).any(|w| w == b"fvar"),
            "no variation table: weights would not work"
        );
    }
}
