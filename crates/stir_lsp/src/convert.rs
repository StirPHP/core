//! Offsets ↔ LSP positions.

use line_index::{LineCol, LineIndex, WideEncoding, WideLineCol};
use ls_types::{Position, Range};
use stir_syntax::{TextRange, TextSize};

/// An LSP position (0-based line and column in the negotiated encoding).
pub fn position(line: u32, character: u32) -> Position {
    Position::new(line, character)
}

/// The position encoding negotiated with the client.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Encoding {
    Utf8,
    Utf16,
}

/// Line/column conversion for one file text.
pub struct LineMap {
    index: LineIndex,
    encoding: Encoding,
}

impl LineMap {
    pub fn new(text: &str, encoding: Encoding) -> Self {
        LineMap {
            index: LineIndex::new(text),
            encoding,
        }
    }

    pub fn position(&self, offset: TextSize) -> Position {
        let offset = offset.min(self.index.len());
        let lc = self.index.line_col(offset);
        match self.encoding {
            Encoding::Utf8 => Position::new(lc.line, lc.col),
            Encoding::Utf16 => {
                let w = self
                    .index
                    .to_wide(WideEncoding::Utf16, lc)
                    .unwrap_or(WideLineCol {
                        line: lc.line,
                        col: lc.col,
                    });
                Position::new(w.line, w.col)
            }
        }
    }

    /// The offset of `pos`, clamped to the end of its line (and of the file).
    pub fn offset(&self, pos: Position) -> TextSize {
        let lc = match self.encoding {
            Encoding::Utf8 => LineCol {
                line: pos.line,
                col: pos.character,
            },
            Encoding::Utf16 => self
                .index
                .to_utf8(
                    WideEncoding::Utf16,
                    WideLineCol {
                        line: pos.line,
                        col: pos.character,
                    },
                )
                .unwrap_or(LineCol {
                    line: pos.line,
                    col: pos.character,
                }),
        };
        let Some(line) = self.index.line(lc.line) else {
            return self.index.len();
        };
        // `line` includes its newline
        let end = if self.index.line(lc.line + 1).is_some() {
            line.end() - TextSize::from(1)
        } else {
            line.end()
        };
        (line.start() + TextSize::from(lc.col)).min(end)
    }

    pub fn range(&self, range: TextRange) -> Range {
        Range::new(self.position(range.start()), self.position(range.end()))
    }

    pub fn text_range(&self, range: Range) -> TextRange {
        let start = self.offset(range.start);
        let end = self.offset(range.end).max(start);
        TextRange::new(start, end)
    }

    /// Line ranges covered by `range` (for splitting multi-line tokens).
    pub fn lines(&self, range: TextRange) -> impl Iterator<Item = TextRange> + '_ {
        self.index.lines(range)
    }

    /// The length of `text` in the negotiated encoding.
    pub fn measure(&self, text: &str) -> u32 {
        match self.encoding {
            Encoding::Utf8 => text.len() as u32,
            Encoding::Utf16 => text.encode_utf16().count() as u32,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf16_round_trip() {
        let text = "val a = \"é𝄞\"\nval b = 1\n";
        let map = LineMap::new(text, Encoding::Utf16);
        let off = TextSize::from(text.find("\"\n").unwrap() as u32);
        let pos = map.position(off);
        assert_eq!(pos, Position::new(0, 12));
        assert_eq!(map.offset(pos), off);
        assert_eq!(
            map.offset(Position::new(1, 99)),
            TextSize::from(text.len() as u32 - 1)
        );
        let utf8 = LineMap::new(text, Encoding::Utf8);
        assert_eq!(utf8.position(off), Position::new(0, 15));
    }
}
