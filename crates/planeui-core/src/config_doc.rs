//! Lossless wrapper around a `toml_edit` document. Every mutation of the config goes through
//! here so untouched content stays byte-for-byte identical.

use std::fmt;

use toml_edit::DocumentMut;

/// An `aerospace.toml` document that renders back to its exact source text until edited.
#[derive(Debug, Clone)]
pub struct ConfigDoc {
    document: DocumentMut,
    line_ending: LineEnding,
}

/// `toml_edit` always renders `\n`, so the source's line ending is restored on output.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LineEnding {
    Lf,
    Crlf,
}

impl LineEnding {
    /// Only a file that uses CRLF throughout counts as CRLF; a mixed file renders as LF.
    fn detect(source: &str) -> Self {
        let line_feeds = source.matches('\n').count();
        if line_feeds > 0 && source.matches("\r\n").count() == line_feeds {
            Self::Crlf
        } else {
            Self::Lf
        }
    }
}

impl ConfigDoc {
    /// Parses config text.
    ///
    /// # Errors
    ///
    /// Returns [`ParseError`] with the 1-based line and column of the first syntax error.
    pub fn parse(source: &str) -> Result<Self, ParseError> {
        source
            .parse::<DocumentMut>()
            .map(|document| Self {
                document,
                line_ending: LineEnding::detect(source),
            })
            .map_err(|error| ParseError::from_toml(source, &error))
    }
}

impl fmt::Display for ConfigDoc {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.line_ending {
            LineEnding::Lf => self.document.fmt(formatter),
            LineEnding::Crlf => formatter.write_str(&to_crlf(&self.document.to_string())),
        }
    }
}

/// Raw string reprs keep their original `\r\n`, so only bare `\n` is converted.
fn to_crlf(text: &str) -> String {
    let mut output = String::with_capacity(text.len() + text.len() / 16);
    let mut previous = None;
    for character in text.chars() {
        if character == '\n' && previous != Some('\r') {
            output.push('\r');
        }
        output.push(character);
        previous = Some(character);
    }
    output
}

/// A TOML syntax error located in the source text.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("line {line}, column {column}: {message}")]
pub struct ParseError {
    /// The parser's description of the problem.
    pub message: String,
    /// 1-based line of the error.
    pub line: usize,
    /// 1-based column of the error, counted in characters.
    pub column: usize,
}

impl ParseError {
    fn from_toml(source: &str, error: &toml_edit::TomlError) -> Self {
        let offset = error.span().map_or(0, |span| span.start);
        let (line, column) = line_and_column(source, offset);
        Self {
            message: error.message().trim_end().to_owned(),
            line,
            column,
        }
    }
}

fn line_and_column(source: &str, offset: usize) -> (usize, usize) {
    let before = source.get(..offset).unwrap_or(source);
    let line_start = before.rfind('\n').map_or(0, |newline| newline + 1);
    let line = before.matches('\n').count() + 1;
    let column = before
        .get(line_start..)
        .map_or(0, |prefix| prefix.chars().count())
        + 1;
    (line, column)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_error_reports_line_and_column() {
        let error = ConfigDoc::parse("gaps = 1\nkey = = 2\n").unwrap_err();
        assert_eq!((error.line, error.column), (2, 7));
    }

    #[test]
    fn parse_error_counts_columns_in_characters() {
        let error = ConfigDoc::parse("é = 'x' ?\n").unwrap_err();
        assert_eq!((error.line, error.column), (1, 9));
    }

    #[test]
    fn display_reproduces_source() {
        let source = "# comment\na =   1 # trailing\n";
        assert_eq!(ConfigDoc::parse(source).unwrap().to_string(), source);
    }

    #[test]
    fn display_keeps_crlf_line_endings() {
        let source = "a = 1\r\n\r\n[t]\r\nb = '''x\r\ny'''\r\n";
        assert_eq!(ConfigDoc::parse(source).unwrap().to_string(), source);
    }
}
