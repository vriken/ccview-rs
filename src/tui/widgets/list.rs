use crate::tui::{
    style::{identity::Identity, theme},
    widgets::{
        search::{self, SearchView},
        tabs::{self, Tab, TabKey},
    },
};
use crate::{
    catalog::{checkout::SharedCheckouts, search::snippet::Snippet},
    session::{
        activity::{self, ASKS_YOU_LABEL, Activity, ActivityMap, ActivityState, Badge, quiet_for},
        entry::SessionEntry,
        live::{LiveSession, Status},
    },
    store::{
        dismissed::Dismissed, favorites::Favorites, hidden::HiddenSessions,
        important::ImportantSessions, on_hold::OnHoldSessions,
    },
};
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::Modifier,
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, List, ListItem, ListState, Padding, Paragraph, Wrap},
};
use std::{
    collections::HashMap,
    ops::Range,
    path::Path,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

pub(crate) const ROW_HEIGHT: u16 = 3;
const IDENTITY_BAR: &str = "▌";
const SELECTED_FOCUSED_MARK: &str = "▶";
const SELECTED_UNFOCUSED_MARK: &str = "›";
const STAR_MARK: &str = "★ ";
const IMPORTANT_MARK: &str = "! ";
const NO_STAR_MARK: &str = "  ";
const BADGE_DOT: &str = "● ";
const NO_BADGE: &str = "  ";
const SEPARATOR: &str = " · ";
const HIDDEN_MARK: &str = "hidden";
const ON_HOLD_MARK: &str = "on hold";
const QUIET_MARK: &str = "quiet";
const SHARED_CHECKOUT_MARK: &str = "shares checkout with";
const SHARED_TITLE_WIDTH: usize = 24;
const COPY_OF_MARK: &str = "copy of";
const MIN_DIRECTORY_WIDTH: usize = 4;
const SHORT_ID_CHARS: usize = 6;
const DISAMBIGUATOR_WIDTH: usize = 28;

#[derive(Debug, Clone, Copy)]
pub(crate) struct DragView {
    pub(crate) row: usize,
    pub(crate) slot: usize,
    pub(crate) offset: usize,
}

pub(crate) struct ListView<'a> {
    pub(crate) title: String,
    pub(crate) tabs: &'a [Tab],
    pub(crate) active: &'a TabKey,
    pub(crate) identity: &'a Identity,
    pub(crate) entries: Vec<&'a SessionEntry>,
    pub(crate) selected: Option<usize>,
    pub(crate) show_directory: bool,
    pub(crate) focused: bool,
    pub(crate) tab_bar_focused: bool,
    pub(crate) hidden: &'a HiddenSessions,
    pub(crate) empty_message: &'a str,
    pub(crate) favorites: &'a Favorites,
    pub(crate) important: &'a ImportantSessions,
    pub(crate) on_hold: &'a OnHoldSessions,
    pub(crate) activity: &'a ActivityMap,
    pub(crate) dismissed: &'a Dismissed,
    pub(crate) shared: &'a SharedCheckouts,
    pub(crate) now: SystemTime,
    pub(crate) drag: Option<DragView>,
    pub(crate) search: Option<SearchView<'a>>,
}

pub(crate) fn draw(frame: &mut Frame, area: Rect, view: &ListView) {
    let title = format!("LIST · {}", view.title);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(theme::zone_border(view.focused, None))
        .title(Span::styled(
            format!(" {}{title} ", if view.focused { "▶ " } else { "" }),
            theme::zone_title(view.focused, None),
        ));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let [tab_area, rows_area] =
        Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).areas(inner);
    let bar = match &view.search {
        Some(search) => search::query_line(search, tab_area.width as usize),
        None => tabs::render_bar(
            view.tabs,
            view.active,
            tab_area.width as usize,
            view.tab_bar_focused,
        ),
    };
    frame.render_widget(
        Paragraph::new(bar).style(theme::zone_text(view.focused)),
        tab_area,
    );
    if view.entries.is_empty() {
        let message = Paragraph::new(view.empty_message)
            .style(theme::empty_state())
            .wrap(Wrap { trim: true })
            .block(Block::default().padding(Padding::horizontal(1)));
        frame.render_widget(message, rows_area);
        return;
    }
    let inner_width = rows_area.width as usize;
    let title_counts = title_counts(&view.entries);
    let highlighted = view.drag.map_or(view.selected, |drag| Some(drag.row));
    let items: Vec<ListItem> = view
        .entries
        .iter()
        .enumerate()
        .map(|(index, entry)| {
            let collides = title_counts
                .get(entry.title.as_str())
                .is_some_and(|&count| count > 1);
            row(
                entry,
                view,
                inner_width,
                highlighted == Some(index),
                collides,
                index,
            )
        })
        .collect();
    let rows_focused = view.focused && !view.tab_bar_focused;
    let list = List::new(items)
        .style(theme::zone_text(view.focused))
        .highlight_style(theme::row_selected(rows_focused));
    let mut state = match view.drag {
        Some(drag) => ListState::default().with_offset(drag.offset),
        None => ListState::default().with_selected(view.selected),
    };
    frame.render_stateful_widget(list, rows_area, &mut state);
    if let Some(drag) = view.drag {
        paint_drop_marker(frame, rows_area, drag);
    }
}

fn paint_drop_marker(frame: &mut Frame, rows_area: Rect, drag: DragView) {
    let lines_below_top = drag.slot.saturating_sub(drag.offset) * usize::from(ROW_HEIGHT);
    let last_line = rows_area.y + rows_area.height.saturating_sub(1);
    let y = (rows_area.y + lines_below_top as u16)
        .saturating_sub(1)
        .min(last_line);
    let area = Rect::new(rows_area.x, y, rows_area.width, 1);
    let rule = "─".repeat(usize::from(rows_area.width));
    frame.render_widget(
        Paragraph::new(Span::styled(rule, theme::drop_marker())),
        area,
    );
}

fn row(
    entry: &SessionEntry,
    view: &ListView,
    width: usize,
    selected: bool,
    collides: bool,
    index: usize,
) -> ListItem<'static> {
    let is_active = tabs::entry_is_active(entry, view.activity, view.dismissed);
    let bar = if is_active && view.identity.enabled() {
        Span::styled(
            IDENTITY_BAR,
            theme::identity_bar(view.identity.session_color(&entry.session_id)),
        )
    } else {
        Span::raw(" ")
    };
    let result = view
        .search
        .as_ref()
        .and_then(|search| search.lines.get(index).copied());
    let title_spans = result.map_or(&[][..], |line| line.title_spans.as_slice());
    let title_line = row_title(entry, view, width, selected, bar.clone(), title_spans);
    let detail_line = match result.and_then(|line| line.hits.map(|hits| (hits, line))) {
        Some((hits, line)) => hit_detail(entry, view.now, width, bar, hits, line.snippet.as_ref()),
        None => row_detail(entry, view, width, collides, bar),
    };
    ListItem::new(vec![title_line, detail_line, Line::raw("")])
}

fn row_title(
    entry: &SessionEntry,
    view: &ListView,
    width: usize,
    selected: bool,
    bar: Span<'static>,
    match_spans: &[Range<usize>],
) -> Line<'static> {
    let live = entry
        .live
        .as_ref()
        .filter(|live| live.pid_alive && !view.dismissed.contains(&entry.session_id));
    let activity = view.activity.get(&entry.session_id);
    let starred = view.favorites.contains_entry(entry);
    let prefix_width = 2 + STAR_MARK.chars().count() + BADGE_DOT.chars().count();
    let hidden = view.hidden.contains(&entry.session_id);
    let shows_as_on_hold = view.on_hold.contains_entry(entry) && is_calm(activity, live);
    let stale = quiet_duration(entry, view).is_some();
    let asking = asks_you(entry, activity, view);
    let rows_focused = view.focused && !view.tab_bar_focused;
    let marker = match (selected, rows_focused) {
        (true, true) => Span::styled(SELECTED_FOCUSED_MARK, theme::selection_marker(true)),
        (true, false) => Span::styled(SELECTED_UNFOCUSED_MARK, theme::selection_marker(false)),
        (false, _) => Span::raw(" "),
    };
    let mut title_spans = vec![
        bar.clone(),
        marker,
        if starred {
            Span::styled(STAR_MARK, theme::star())
        } else {
            Span::raw(NO_STAR_MARK)
        },
        match (activity, live) {
            _ if shows_as_on_hold => Span::styled(BADGE_DOT, theme::on_hold()),
            _ if stale => Span::styled(BADGE_DOT, theme::stale()),
            (Some(_), _) if asking => Span::styled(BADGE_DOT, theme::asks_you()),
            (Some(activity), _) => Span::styled(BADGE_DOT, theme::activity_dot(&activity.state)),
            (None, Some(live)) => Span::styled(BADGE_DOT, theme::status_badge(&live.status)),
            (None, None) => Span::raw(NO_BADGE),
        },
    ];
    let mut title_width = width.saturating_sub(prefix_width);
    if activity.is_some_and(Activity::needs_you) {
        let marker = Badge::NeedsYou.text();
        title_width = title_width.saturating_sub(marker.chars().count() + 3);
        title_spans.push(Span::styled(format!(" {marker} "), theme::needs_you()));
        title_spans.push(Span::raw(" "));
    }
    if view.important.contains_entry(entry) {
        title_width = title_width.saturating_sub(IMPORTANT_MARK.chars().count());
        title_spans.push(Span::styled(IMPORTANT_MARK, theme::important()));
    }
    let title_style = if hidden || entry.copy_of.is_some() {
        theme::row_title(selected).add_modifier(Modifier::DIM)
    } else {
        theme::row_title(selected)
    };
    let shown = truncate(&entry.title, title_width);
    let kept = if shown.len() < entry.title.len() {
        shown.len().saturating_sub('…'.len_utf8())
    } else {
        shown.len()
    };
    let clamped: Vec<Range<usize>> = match_spans
        .iter()
        .map(|span| span.start..span.end.min(kept))
        .collect();
    title_spans.extend(search::highlighted(
        &shown,
        &clamped,
        title_style,
        title_style.patch(theme::search_hit()),
    ));
    Line::from(title_spans)
}

fn hit_detail(
    entry: &SessionEntry,
    now: SystemTime,
    width: usize,
    bar: Span<'static>,
    hits: u32,
    snippet: Option<&Snippet>,
) -> Line<'static> {
    let prefix_width = 2 + STAR_MARK.chars().count() + BADGE_DOT.chars().count();
    let label = search::hits_label(hits);
    let room =
        width.saturating_sub(prefix_width + label.chars().count() + SEPARATOR.chars().count());
    let mut spans = vec![
        bar,
        Span::raw(" ".repeat(prefix_width - 1)),
        Span::styled(label, theme::secondary()),
        Span::styled(SEPARATOR, theme::secondary()),
    ];
    match snippet {
        Some(snippet) => {
            let (text, match_spans) = search::window_on_match(snippet, room);
            spans.extend(search::highlighted(
                &text,
                &match_spans,
                theme::secondary(),
                theme::search_hit(),
            ));
        }
        None => spans.push(Span::styled(
            format!(
                "{}{SEPARATOR}{}",
                directory_name(&entry.cwd),
                relative_time(entry.last_used, now)
            ),
            theme::secondary(),
        )),
    }
    Line::from(search::clip_spans(spans, width))
}

fn row_detail(
    entry: &SessionEntry,
    view: &ListView,
    width: usize,
    collides: bool,
    bar: Span<'static>,
) -> Line<'static> {
    let live = entry
        .live
        .as_ref()
        .filter(|live| live.pid_alive && !view.dismissed.contains(&entry.session_id));
    let activity = view.activity.get(&entry.session_id);
    let prefix_width = 2 + STAR_MARK.chars().count() + BADGE_DOT.chars().count();
    let hidden = view.hidden.contains(&entry.session_id);
    let mut detail = vec![bar, Span::raw(" ".repeat(prefix_width - 1))];
    let mut parts: Vec<Span<'static>> = Vec::new();
    let on_hold = view.on_hold.contains_entry(entry);
    let shows_as_on_hold = on_hold && is_calm(activity, live);
    match (activity, live) {
        (Some(activity), _) => {
            if shows_as_on_hold {
                parts.push(Span::styled(ON_HOLD_MARK, theme::on_hold()));
            } else if asks_you(entry, Some(activity), view) {
                parts.push(Span::styled(ASKS_YOU_LABEL, theme::asks_you()));
            } else {
                parts.push(Span::styled(
                    activity.state.label().to_owned(),
                    theme::activity_dot(&activity.state),
                ));
            }
            for badge in activity::badges(activity)
                .into_iter()
                .filter(|badge| *badge != Badge::NeedsYou)
            {
                parts.push(Span::styled(badge.text(), theme::badge(&badge)));
            }
        }
        (None, Some(live)) if !shows_as_on_hold => parts.push(Span::styled(
            live.status.label().to_owned(),
            theme::status_badge(&live.status),
        )),
        _ if shows_as_on_hold => parts.push(Span::styled(ON_HOLD_MARK, theme::on_hold())),
        (None, Some(_)) | (None, None) => {}
    }
    if on_hold && !shows_as_on_hold {
        parts.push(Span::styled(ON_HOLD_MARK, theme::on_hold()));
    }
    if let Some(quiet) = quiet_label(entry, view) {
        parts.push(Span::styled(quiet, theme::stale()));
    }
    if let Some(shared) = view.shared.for_entry(entry) {
        let more = if shared.more > 0 {
            format!(" +{}", shared.more)
        } else {
            String::new()
        };
        parts.push(Span::styled(
            format!(
                "{SHARED_CHECKOUT_MARK} {}{more}",
                truncate(&shared.first_other, SHARED_TITLE_WIDTH)
            ),
            theme::shared_checkout(),
        ));
    }
    if hidden {
        parts.push(Span::styled(HIDDEN_MARK, theme::secondary()));
    }
    let directory_slot = view.show_directory.then_some(parts.len());
    if view.show_directory {
        parts.push(Span::styled(directory_name(&entry.cwd), theme::secondary()));
    }
    parts.push(Span::styled(
        relative_time(entry.last_used, view.now),
        theme::secondary(),
    ));
    if !entry.copies.is_empty() {
        parts.push(Span::styled(
            format!("+{}", entry.copies.len()),
            theme::secondary(),
        ));
    }
    if let Some(visible) = &entry.copy_of {
        let short: String = visible.chars().take(SHORT_ID_CHARS).collect();
        parts.push(Span::styled(
            format!("{COPY_OF_MARK} {short}"),
            theme::secondary(),
        ));
    }
    if collides {
        parts.push(Span::styled(disambiguator(entry), theme::secondary()));
    }
    let separators = SEPARATOR.chars().count() * parts.len().saturating_sub(1);
    let total: usize = parts.iter().map(Span::width).sum::<usize>() + separators;
    let overflow = (prefix_width + total).saturating_sub(width);
    if let Some(slot) = directory_slot.filter(|_| overflow > 0) {
        let room = parts[slot]
            .width()
            .saturating_sub(overflow)
            .max(MIN_DIRECTORY_WIDTH);
        parts[slot] = Span::styled(truncate(&parts[slot].content, room), theme::secondary());
    }
    for (index, part) in parts.into_iter().enumerate() {
        if index > 0 {
            detail.push(Span::styled(SEPARATOR, theme::secondary()));
        }
        detail.push(part);
    }
    Line::from(detail)
}

fn quiet_duration(entry: &SessionEntry, view: &ListView) -> Option<Duration> {
    let activity = view.activity.get(&entry.session_id)?;
    quiet_for(activity, view.on_hold.contains_entry(entry), view.now)
}

fn quiet_label(entry: &SessionEntry, view: &ListView) -> Option<String> {
    let quiet = quiet_duration(entry, view)?;
    Some(format!("{QUIET_MARK} {}", short_duration(quiet)))
}

fn short_duration(duration: Duration) -> String {
    match duration.as_secs() {
        seconds @ 0..3600 => format!("{}m", seconds / 60),
        seconds @ 3600..86_400 => format!("{}h", seconds / 3600),
        seconds => format!("{}d", seconds / 86_400),
    }
}

fn asks_you(entry: &SessionEntry, activity: Option<&Activity>, view: &ListView) -> bool {
    activity::asks_you(entry.asks_user, activity) && !view.dismissed.contains(&entry.session_id)
}

fn is_calm(activity: Option<&Activity>, live: Option<&LiveSession>) -> bool {
    match (activity, live) {
        (Some(activity), _) => activity.state == ActivityState::Idle,
        (None, Some(live)) => live.status == Status::Idle,
        (None, None) => true,
    }
}

fn title_counts<'a>(entries: &[&'a SessionEntry]) -> HashMap<&'a str, usize> {
    let mut counts = HashMap::new();
    for entry in entries {
        *counts.entry(entry.title.as_str()).or_insert(0) += 1;
    }
    counts
}

pub(crate) fn disambiguator(entry: &SessionEntry) -> String {
    match entry
        .last_prompt
        .as_deref()
        .filter(|prompt| *prompt != entry.title)
    {
        Some(prompt) => format!("“{}”", truncate(prompt, DISAMBIGUATOR_WIDTH)),
        None => entry.session_id.chars().take(SHORT_ID_CHARS).collect(),
    }
}

fn directory_name(cwd: &str) -> String {
    Path::new(cwd)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(cwd)
        .to_owned()
}

pub(crate) fn truncate(text: &str, width: usize) -> String {
    if text.chars().count() <= width {
        return text.to_owned();
    }
    let kept: String = text.chars().take(width.saturating_sub(1)).collect();
    format!("{kept}…")
}

pub(crate) fn relative_time(modified: SystemTime, now: SystemTime) -> String {
    let seconds = now
        .duration_since(modified)
        .unwrap_or(Duration::ZERO)
        .as_secs();
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::entry::TitleSource;
    use std::path::PathBuf;

    fn entry(id: &str, title: &str, last_prompt: Option<&str>) -> SessionEntry {
        SessionEntry {
            path: PathBuf::from(format!("/p/{id}.jsonl")),
            session_id: id.to_owned(),
            cwd: "/w".to_owned(),
            title: title.to_owned(),
            title_source: TitleSource::Prompt,
            last_prompt: last_prompt.map(str::to_owned),
            last_used: UNIX_EPOCH,
            live: None,
            ..SessionEntry::stub()
        }
    }

    #[test]
    fn only_rows_sharing_a_title_are_counted_as_colliding() {
        let a = entry("a1", "same", None);
        let b = entry("b2", "same", None);
        let c = entry("c3", "other", None);
        let counts = title_counts(&[&a, &b, &c]);
        assert_eq!(counts["same"], 2);
        assert_eq!(counts["other"], 1);
    }

    #[test]
    fn suffix_is_the_last_prompt_or_else_the_short_id_never_both() {
        let with_prompt = entry("abcdef123456", "t", Some("add the tests"));
        assert_eq!(disambiguator(&with_prompt), "“add the tests”");
        let without = entry("abcdef123456", "t", None);
        assert_eq!(disambiguator(&without), "abcdef");
        let same_as_title = entry("abcdef123456", "t", Some("t"));
        assert_eq!(disambiguator(&same_as_title), "abcdef");
    }

    #[test]
    fn a_hit_line_fits_the_pane_with_wide_characters_and_still_shows_the_match() {
        let entry = entry("a1", "t", None);
        let lead = "日本語の長い前置きの文章がここに続きます ".repeat(4);
        let matched = "テキスト";
        let text = format!("…{lead}{matched} と、その後ろの長い文章が続きます ASCII also");
        let start = '…'.len_utf8() + lead.len();
        let matched_span = start..start + matched.len();
        let snippet = Snippet {
            text,
            spans: vec![matched_span],
        };
        for width in 6..80 {
            let line = hit_detail(
                &entry,
                UNIX_EPOCH,
                width,
                Span::raw(" "),
                12,
                Some(&snippet),
            );
            assert!(line.width() <= width, "{width}: {line:?}");
        }
        let narrow = hit_detail(&entry, UNIX_EPOCH, 40, Span::raw(" "), 12, Some(&snippet));
        assert!(narrow.to_string().contains(matched), "{narrow:?}");
    }
}
