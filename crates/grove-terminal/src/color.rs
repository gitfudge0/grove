//! Token-space color: `grove-terminal` never resolves a theme, only emits the token the stream asked for; the painting layer does palette lookup.

/// `Ansi` keeps the raw 0..=255 index (including bright variants 8..=15) rather than folding, since the painting layer may want that information later.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum TermColor {
    #[default]
    Default,
    Ansi(u8),
    Rgb(u8, u8, u8),
}
