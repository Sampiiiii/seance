// Owns terminal link hit-testing and URL normalization for modifier-click browser opening.

use std::ops::Range;

use linkify::{LinkFinder, LinkKind};
use seance_terminal::TerminalRow;
use url::Url;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TerminalDetectedLink {
    pub(crate) col_range: Range<usize>,
    pub(crate) url: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct CellSpan {
    col_range: Range<usize>,
    byte_range: Range<usize>,
}

pub(crate) fn terminal_link_at_column(row: &TerminalRow, col: usize) -> Option<String> {
    terminal_links_for_row(row, row.terminal_width())
        .into_iter()
        .find(|link| link.col_range.contains(&col))
        .map(|link| link.url)
}

pub(crate) fn terminal_links_for_row(
    row: &TerminalRow,
    visible_cols: usize,
) -> Vec<TerminalDetectedLink> {
    if visible_cols == 0 {
        return Vec::new();
    }

    let line = row.plain_text();
    let spans = cell_spans(row);
    if spans.is_empty() {
        return Vec::new();
    }

    let mut finder = LinkFinder::new();
    finder.url_must_have_scheme(false);
    finder.kinds(&[LinkKind::Url]);

    let mut links = Vec::new();
    for candidate in finder.links(&line) {
        let Some(adjusted) = trimmed_link_range(&line, candidate.start(), candidate.end()) else {
            continue;
        };
        let Some(col_range) = byte_range_to_col_range(&spans, adjusted.clone()) else {
            continue;
        };
        let start = col_range.start.min(visible_cols);
        let end = col_range.end.min(visible_cols);
        if start >= end {
            continue;
        }
        let Ok(url) = normalize_openable_url(&line[adjusted]) else {
            continue;
        };

        links.push(TerminalDetectedLink {
            col_range: start..end,
            url,
        });
    }

    links
}

fn cell_spans(row: &TerminalRow) -> Vec<CellSpan> {
    let mut spans = Vec::with_capacity(row.cells.len());
    let mut col_start = 0usize;
    let mut byte_start = 0usize;

    for cell in &row.cells {
        let col_end = col_start + usize::from(cell.width.max(1));
        let byte_end = byte_start + cell.text.len();
        spans.push(CellSpan {
            col_range: col_start..col_end,
            byte_range: byte_start..byte_end,
        });
        col_start = col_end;
        byte_start = byte_end;
    }

    spans
}

fn trimmed_link_range(text: &str, start: usize, end: usize) -> Option<Range<usize>> {
    if start >= end || end > text.len() {
        return None;
    }

    let slice = &text[start..end];
    let leading = slice
        .char_indices()
        .find(|(_, ch)| !is_leading_trim_char(*ch))
        .map(|(idx, _)| idx)
        .unwrap_or(slice.len());
    if leading == slice.len() {
        return None;
    }

    let trailing = slice
        .char_indices()
        .rev()
        .find(|(_, ch)| !is_trailing_trim_char(*ch))
        .map(|(idx, ch)| idx + ch.len_utf8())
        .unwrap_or(0);
    if leading >= trailing {
        return None;
    }

    Some((start + leading)..(start + trailing))
}

fn byte_range_to_col_range(spans: &[CellSpan], bytes: Range<usize>) -> Option<Range<usize>> {
    let mut start_col = None;
    let mut end_col = None;

    for span in spans {
        if span.byte_range.end <= bytes.start || span.byte_range.start >= bytes.end {
            continue;
        }

        start_col.get_or_insert(span.col_range.start);
        end_col = Some(span.col_range.end);
    }

    Some(start_col?..end_col?)
}

fn normalize_openable_url(raw: &str) -> Result<String, url::ParseError> {
    let trimmed = raw.trim();
    let lower = trimmed.to_ascii_lowercase();
    let candidate = if lower.starts_with("www.") {
        format!("https://{trimmed}")
    } else {
        trimmed.to_string()
    };
    let parsed = Url::parse(&candidate)?;
    match parsed.scheme() {
        "http" | "https" | "mailto" => Ok(candidate),
        _ => Err(url::ParseError::RelativeUrlWithoutBase),
    }
}

fn is_leading_trim_char(ch: char) -> bool {
    matches!(ch, '(' | '[' | '{' | '<' | '"' | '\u{27}')
}

fn is_trailing_trim_char(ch: char) -> bool {
    matches!(
        ch,
        '.' | ',' | ':' | ';' | '!' | '?' | ')' | ']' | '}' | '>' | '"' | '\u{27}'
    )
}

#[cfg(test)]
mod tests {
    use seance_terminal::{TerminalCell, TerminalCellStyle, TerminalRow};

    use super::{normalize_openable_url, terminal_link_at_column, terminal_links_for_row};

    fn row(text: &str) -> TerminalRow {
        TerminalRow {
            cells: text
                .chars()
                .map(|ch| TerminalCell {
                    text: ch.to_string(),
                    style: TerminalCellStyle::default(),
                    width: 1,
                })
                .collect(),
        }
    }

    #[test]
    fn click_inside_https_link_returns_url() {
        let row = row("open https://example.com now");

        assert_eq!(
            terminal_link_at_column(&row, 10).as_deref(),
            Some("https://example.com")
        );
    }

    #[test]
    fn click_inside_www_link_normalizes_to_https() {
        let row = row("visit www.example.com today");

        assert_eq!(
            terminal_link_at_column(&row, 8).as_deref(),
            Some("https://www.example.com")
        );
    }

    #[test]
    fn trailing_punctuation_is_excluded() {
        let row = row("visit https://example.com, now");

        assert_eq!(
            terminal_link_at_column(&row, 8).as_deref(),
            Some("https://example.com")
        );
    }

    #[test]
    fn click_outside_link_returns_none() {
        let row = row("visit https://example.com now");

        assert_eq!(terminal_link_at_column(&row, 1), None);
    }

    #[test]
    fn unsupported_schemes_are_rejected() {
        assert!(normalize_openable_url("javascript:alert(1)").is_err());
        assert!(normalize_openable_url("file:///tmp/test").is_err());
    }

    #[test]
    fn wrapped_multi_row_links_are_not_reconstructed() {
        let row = row("https://");

        assert_eq!(terminal_link_at_column(&row, 4), None);
    }

    #[test]
    fn returns_detected_link_spans_with_normalized_url() {
        let row = row("open https://example.com now");

        assert_eq!(
            terminal_links_for_row(&row, row.terminal_width()),
            vec![super::TerminalDetectedLink {
                col_range: 5..24,
                url: "https://example.com".into(),
            }]
        );
    }

    #[test]
    fn visible_columns_clip_link_styling_range() {
        let row = row("open https://example.com");

        assert_eq!(
            terminal_links_for_row(&row, 14),
            vec![super::TerminalDetectedLink {
                col_range: 5..14,
                url: "https://example.com".into(),
            }]
        );
    }
}
