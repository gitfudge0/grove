//! Presentation-only source replacements. The editor buffer remains literal.
use std::ops::Range;

#[derive(Clone, Debug, PartialEq)]
pub struct TextReplacement {
    pub range: Range<usize>,
    pub text: String,
    pub font_scale: f32,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct TextProjection {
    pub text: String,
    pub font_scale: f32,
    source_len: usize,
    edits: Vec<(Range<usize>, Range<usize>)>,
}

impl TextProjection {
    pub fn new(source: &str, replacements: &[TextReplacement], base: usize) -> Self {
        let mut result = Self {
            source_len: source.len(),
            font_scale: 1.,
            ..Self::default()
        };
        let mut previous = 0;
        for replacement in replacements {
            if replacement.range.start < base || replacement.range.end > base + source.len() {
                continue;
            }
            let range = replacement.range.start - base..replacement.range.end - base;
            if range.start < previous || source.get(range.clone()).is_none() {
                continue;
            }
            result.font_scale = result.font_scale.max(replacement.font_scale);
            result.text.push_str(&source[previous..range.start]);
            let start = result.text.len();
            result.text.push_str(&replacement.text);
            result.edits.push((range.clone(), start..result.text.len()));
            previous = range.end;
        }
        result.text.push_str(&source[previous..]);
        result
    }

    pub fn to_display(&self, offset: usize) -> usize {
        let mut source_end = 0;
        let mut display_end = 0;
        for (source, display) in &self.edits {
            if offset < source.start {
                break;
            }
            if offset < source.end {
                return display.start;
            }
            source_end = source.end;
            display_end = display.end;
        }
        display_end + offset.saturating_sub(source_end)
    }

    /// A collapsed marker belongs to the following editable content.
    pub fn to_source(&self, offset: usize) -> usize {
        let mut source_end = 0;
        let mut display_end = 0;
        for (source, display) in &self.edits {
            if offset < display.start {
                break;
            }
            if offset < display.end {
                return source.start;
            }
            source_end = source.end;
            display_end = display.end;
        }
        (source_end + offset.saturating_sub(display_end)).min(self.source_len)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn hidden_and_replaced_syntax_keeps_unicode_source_coordinates() {
        let source = "## 🦀 **é**";
        let p = TextProjection::new(
            source,
            &[
                TextReplacement {
                    range: 0..3,
                    text: String::new(),
                    font_scale: 1.,
                },
                TextReplacement {
                    range: 8..10,
                    text: String::new(),
                    font_scale: 1.,
                },
                TextReplacement {
                    range: 12..14,
                    text: String::new(),
                    font_scale: 1.,
                },
            ],
            0,
        );
        assert_eq!(p.text, "🦀 é");
        assert_eq!(p.to_display(3), 0);
        assert_eq!(p.to_source(0), 3);
        assert_eq!(p.to_source(5), 10);
        assert_eq!(p.to_display(source.len()), p.text.len());
        let p = TextProjection::new(
            "- list",
            &[TextReplacement {
                range: 0..2,
                text: "• ".into(),
                font_scale: 1.,
            }],
            0,
        );
        assert_eq!(p.text, "• list");
        assert_eq!(p.to_display(2), 4);
        assert_eq!(p.to_source(4), 2);
    }
}
