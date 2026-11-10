use super::theme;
use crate::{favorites::Favorites, library::SessionEntry};
use ratatui::{
    Frame,
    layout::Rect,
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, List, ListItem, ListState, Padding, Paragraph, Wrap},
};
use std::{
    path::Path,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

const STAR_MARK: &str = "★ ";
const NO_STAR_MARK: &str = "  ";
const BADGE_DOT: &str = "● ";
const NO_BADGE: &str = "  ";
const SEPARATOR: &str = " · ";

pub struct ListView<'a> {
    pub title: String,
    pub entries: Vec<&'a SessionEntry>,
    pub selected: Option<usize>,
    pub show_directory: bool,
    pub focused: bool,
    pub empty_message: &'a str,
    pub favorites: &'a Favorites,
    pub now: SystemTime,
}

pub fn draw(frame: &mut Frame, area: Rect, view: &ListView) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(theme::border(view.focused))
        .title(Span::styled(
            format!(" {} ", view.title),
            theme::pane_title(view.focused),
        ));
    if view.entries.is_empty() {
        let message = Paragraph::new(view.empty_message)
            .style(theme::empty_state())
            .wrap(Wrap { trim: true })
            .block(block.padding(Padding::horizontal(1)));
        frame.render_widget(message, area);
        return;
    }
    let inner_width = area.width.saturating_sub(2) as usize;
    let items: Vec<ListItem> = view
        .entries
        .iter()
        .enumerate()
        .map(|(index, entry)| row(entry, view, inner_width, view.selected == Some(index)))
        .collect();
    let list = List::new(items)
        .block(block)
        .highlight_style(theme::row_selected());
    let mut state = ListState::default().with_selected(view.selected);
    frame.render_stateful_widget(list, area, &mut state);
}

fn row(entry: &SessionEntry, view: &ListView, width: usize, selected: bool) -> ListItem<'static> {
    let live = entry.live.as_ref().filter(|live| live.pid_alive);
    let starred = view.favorites.contains(&entry.session_id);
    let prefix_width = STAR_MARK.chars().count() + BADGE_DOT.chars().count();
    let title_line = Line::from(vec![
        if starred {
            Span::styled(STAR_MARK, theme::star())
        } else {
            Span::raw(NO_STAR_MARK)
        },
        match live {
            Some(live) => Span::styled(BADGE_DOT, theme::status_badge(&live.status)),
            None => Span::raw(NO_BADGE),
        },
        Span::styled(
            truncate(&entry.title, width.saturating_sub(prefix_width)),
            theme::row_title(selected),
        ),
    ]);
    let mut detail = vec![Span::raw(" ".repeat(prefix_width))];
    let mut parts: Vec<Span<'static>> = Vec::new();
    if view.show_directory {
        parts.push(Span::styled(directory_name(&entry.cwd), theme::secondary()));
    }
    parts.push(Span::styled(
        relative_time(entry.last_used, view.now),
        theme::secondary(),
    ));
    if let Some(live) = live {
        parts.push(Span::styled(
            live.status.label().to_owned(),
            theme::status_badge(&live.status),
        ));
    }
    for (index, part) in parts.into_iter().enumerate() {
        if index > 0 {
            detail.push(Span::styled(SEPARATOR, theme::secondary()));
        }
        detail.push(part);
    }
    ListItem::new(vec![title_line, Line::from(detail), Line::raw("")])
}

fn directory_name(cwd: &str) -> String {
    Path::new(cwd)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(cwd)
        .to_owned()
}

pub fn truncate(text: &str, width: usize) -> String {
    if text.chars().count() <= width {
        return text.to_owned();
    }
    let kept: String = text.chars().take(width.saturating_sub(1)).collect();
    format!("{kept}…")
}

pub fn relative_time(modified: SystemTime, now: SystemTime) -> String {
    let seconds = now.duration_since(modified).unwrap_or(Duration::ZERO).as_secs();
    match seconds {
        0..60 => "now".to_owned(),
        60..3600 => format!("{}m ago", seconds / 60),
        3600..86_400 => format!("{}h ago", seconds / 3600),
        86_400..604_800 => format!("{}d ago", seconds / 86_400),
        _ => absolute_date(modified),
    }
}

fn absolute_date(time: SystemTime) -> String {
    let days = time
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() / 86_400)
        .unwrap_or(0) as i64;
    let shifted = days + 719_468;
    let era = shifted / 146_097;
    let day_of_era = shifted - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_index + 2) / 5 + 1;
    let month = if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    format!("{year}-{month:02}-{day:02}")
}
