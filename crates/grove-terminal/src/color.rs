//! Token-space color. `grove-terminal` never resolves a theme: it emits the
//! *token* the stream asked for and leaves the palette lookup to the painting
//! layer (see `src/gui/pty.rs`'s `ansi_idx` in the iced app, and Plan 04 in the
//! gpui rewrite).

/// A color as the terminal stream expressed it.
///
/// `Ansi` keeps the raw 0..=255 index — including the bright variants at
/// 8..=15, which the theme layer folds onto the same tokens as 0..=7. Folding
/// here would lose information the painting layer may want later.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum TermColor {
    /// The terminal's default foreground/background.
    #[default]
    Default,
    /// A palette index: 0..=15 named, 16..=231 the 6×6×6 cube, 232..=255 grays.
    Ansi(u8),
    /// A direct 24-bit color.
    Rgb(u8, u8, u8),
}
