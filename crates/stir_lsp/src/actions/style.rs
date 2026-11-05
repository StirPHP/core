//! How a file is written: what generated code has to look like to fit in.

use stir_syntax::TextSize;

use crate::imports;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Style {
    pub nl: &'static str,
    /// One level of indentation.
    pub unit: String,
}

impl Style {
    pub fn of(text: &str) -> Style {
        let crlf = text.matches("\r\n").count();
        let lf = text.matches('\n').count() - crlf;
        let nl = if crlf > lf { "\r\n" } else { "\n" };
        let mut tabs = 0;
        let mut spaces = 0;
        let mut smallest = usize::MAX;
        for line in text.lines().take(500) {
            let rest = line.trim_start_matches([' ', '\t']);
            // what's in a doc comment is a space further in than the comment
            if rest.is_empty() || rest.starts_with('*') {
                continue;
            }
            let lead = &line[..line.len() - rest.len()];
            if lead.starts_with('\t') {
                tabs += 1;
            } else if !lead.is_empty() {
                spaces += 1;
                smallest = smallest.min(lead.len());
            }
        }
        let unit = if tabs > spaces {
            "\t".to_owned()
        } else if spaces > 0 && smallest >= 2 {
            " ".repeat(smallest)
        } else {
            "    ".to_owned()
        };
        Style { nl, unit }
    }

    /// `code`, a piece written with `\n` and a tab for each level, as this file has it: at
    /// `indent`, with blank lines left blank.
    pub fn block(&self, code: &str, indent: &str) -> String {
        let lines: Vec<String> = code
            .split('\n')
            .map(|line| {
                if line.is_empty() {
                    return String::new();
                }
                let rest = line.trim_start_matches('\t');
                let depth = line.len() - rest.len();
                format!("{indent}{}{rest}", self.unit.repeat(depth))
            })
            .collect();
        lines.join(self.nl)
    }
}

/// The whitespace the line of `at` starts with.
pub fn indent_at(text: &str, at: TextSize) -> String {
    imports::line_indent(text, at)
}

/// Is there nothing but whitespace before `at` on its line?
pub fn starts_line(text: &str, at: TextSize) -> bool {
    let start = usize::from(imports::line_start(text, at));
    text[start..usize::from(at)]
        .chars()
        .all(|c| c == ' ' || c == '\t')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_style_of_a_file() {
        let s =
            Style::of("<?php\r\nclass A {\r\n\tfunction f() {\r\n\t\treturn 1;\r\n\t}\r\n}\r\n");
        assert_eq!(s.nl, "\r\n");
        assert_eq!(s.unit, "\t");
        let s = Style::of("<?php\nclass A {\n  /**\n   * doc\n   */\n  function f() {}\n}\n");
        assert_eq!(s.nl, "\n");
        assert_eq!(s.unit, "  ");
        assert_eq!(Style::of("<?php\n").unit, "    ");
        assert_eq!(s.block("a\n\tb\n\nc", "x"), "xa\nx  b\n\nxc");
    }
}
