use super::renderer::TableState;
use super::theme::Theme;
use super::wrap::{Piece, flat_width, spans_width, wrap, wrap_lines};
use pulldown_cmark::Alignment;
use ratatui::style::Style;
use ratatui::text::{Line, Span};

const MIN_TABLE_COLUMN_WIDTH: usize = 3;

fn fit_column_widths(natural: &[usize], available: usize) -> Vec<usize> {
    let total: usize = natural.iter().sum();
    if total <= available {
        return natural.to_vec();
    }
    let used_at = |cap: usize| natural.iter().map(|&w| w.min(cap)).sum::<usize>();
    let (mut low, mut high) = (
        MIN_TABLE_COLUMN_WIDTH,
        natural.iter().copied().max().unwrap_or(1),
    );
    while low < high {
        let mid = (low + high).div_ceil(2);
        if used_at(mid) <= available {
            low = mid;
        } else {
            high = mid - 1;
        }
    }
    let mut widths: Vec<usize> = natural.iter().map(|&w| w.min(low)).collect();
    let mut leftover = available.saturating_sub(widths.iter().sum());
    for (width, &wanted) in widths.iter_mut().zip(natural) {
        if leftover == 0 {
            break;
        }
        if *width < wanted {
            *width += 1;
            leftover -= 1;
        }
    }
    widths
}

fn border_line(
    widths: &[usize],
    left: &str,
    mid: &str,
    right: &str,
    style: Style,
) -> Line<'static> {
    let mut text = String::from(left);
    for (index, width) in widths.iter().enumerate() {
        if index > 0 {
            text.push_str(mid);
        }
        text.push_str(&"─".repeat(width + 2));
    }
    text.push_str(right);
    Line::from(Span::styled(text, style))
}

pub(super) fn render_table(table: &TableState, width: usize, theme: &Theme) -> Vec<Line<'static>> {
    let columns = table
        .rows
        .iter()
        .map(Vec::len)
        .max()
        .unwrap_or(0)
        .max(table.alignments.len());
    if columns == 0 {
        return Vec::new();
    }
    let chrome = 3 * columns + 1;
    let available = width.saturating_sub(chrome);
    if available < MIN_TABLE_COLUMN_WIDTH * columns {
        return render_table_as_rows(table, width, theme);
    }

    let mut natural = vec![1; columns];
    for row in &table.rows {
        for (index, cell) in row.iter().enumerate() {
            natural[index] = natural[index].max(flat_width(cell));
        }
    }
    let widths = fit_column_widths(&natural, available);
    let border = theme.table_border;
    let mut lines = vec![border_line(&widths, "┌", "┬", "┐", border)];
    for (row_index, row) in table.rows.iter().enumerate() {
        let wrapped: Vec<Vec<Vec<Span<'static>>>> = (0..columns)
            .map(|column| match row.get(column) {
                Some(cell) => wrap(cell, widths[column]),
                None => Vec::new(),
            })
            .collect();
        let height = wrapped.iter().map(Vec::len).max().unwrap_or(0).max(1);
        for line_index in 0..height {
            let mut spans = vec![Span::styled("│", border)];
            for column in 0..columns {
                let cell_line = wrapped[column].get(line_index).cloned().unwrap_or_default();
                let slack = widths[column] - spans_width(&cell_line).min(widths[column]);
                let (left, right) = match table.alignments.get(column) {
                    Some(Alignment::Right) => (slack, 0),
                    Some(Alignment::Center) => (slack / 2, slack - slack / 2),
                    _ => (0, slack),
                };
                spans.push(Span::raw(" ".repeat(1 + left)));
                spans.extend(cell_line);
                spans.push(Span::raw(" ".repeat(1 + right)));
                spans.push(Span::styled("│", border));
            }
            lines.push(Line::from(spans));
        }
        if row_index + 1 == table.header_rows {
            lines.push(border_line(&widths, "├", "┼", "┤", border));
        }
    }
    lines.push(border_line(&widths, "└", "┴", "┘", border));
    lines
}

fn render_table_as_rows(table: &TableState, width: usize, theme: &Theme) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    for (row_index, row) in table.rows.iter().enumerate() {
        let mut pieces = Vec::new();
        for (index, cell) in row.iter().enumerate() {
            if index > 0 {
                pieces.push(Piece::Text(" │ ".into(), theme.table_border));
            }
            for piece in cell {
                pieces.push(match piece {
                    Piece::Text(text, style) => Piece::Text(text.clone(), *style),
                    Piece::Break => Piece::Break,
                });
            }
        }
        lines.extend(wrap_lines(&pieces, width));
        if row_index + 1 == table.header_rows {
            lines.push(Line::from(Span::styled("─".repeat(width), theme.rule)));
        }
    }
    lines
}
