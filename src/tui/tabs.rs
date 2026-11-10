use super::theme;
use crate::{
    activity::{self, ActivityMap},
    dismissed::Dismissed,
    favorite_dirs::FavoriteDirs,
    favorites::Favorites,
    library::SessionEntry,
};
use ratatui::{
    style::Style,
    text::{Line, Span},
};
use std::{collections::HashMap, ops::Range, path::Path};

const TAB_PADDING: usize = 2;
const TAB_SEPARATOR: usize = 1;
const OVERFLOW_MARKER_WIDTH: usize = 3;
const PINNED_TABS: usize = 3;
const FOCUS_MARKER: &str = "▶";
const DIR_STAR: &str = "★ ";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TabKey {
    All,
    Favorites,
    Active,
    Dir(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tab {
    pub key: TabKey,
    pub label: String,
}

pub fn build_tabs(
    entries: &[SessionEntry],
    activity: &ActivityMap,
    dismissed: &Dismissed,
    favorite_dirs: &FavoriteDirs,
    visible: &dyn Fn(&SessionEntry) -> bool,
) -> Vec<Tab> {
    let mut directories: Vec<&str> = Vec::new();
    for entry in entries.iter().filter(|entry| visible(entry)) {
        if !directories.contains(&entry.cwd.as_str()) {
            directories.push(&entry.cwd);
        }
    }
    let mut labels = directory_labels(&directories);
    let mut order: Vec<usize> = (0..directories.len()).collect();
    order.sort_by_key(|&index| !favorite_dirs.contains(directories[index]));
    for &index in &order {
        if favorite_dirs.contains(directories[index]) {
            labels[index] = format!("{DIR_STAR}{}", labels[index]);
        }
    }
    let active_count = entries
        .iter()
        .filter(|entry| visible(entry))
        .filter(|entry| entry_is_active(entry, activity, dismissed))
        .count();
    let active_label = match active_count {
        0 => "Active".to_owned(),
        count => format!("Active {count}"),
    };
    let mut tabs = vec![
        Tab { key: TabKey::All, label: "All".to_owned() },
        Tab { key: TabKey::Favorites, label: "★ Favorites".to_owned() },
        Tab { key: TabKey::Active, label: active_label },
    ];
    tabs.extend(order.into_iter().map(|index| Tab {
        key: TabKey::Dir(directories[index].to_owned()),
        label: labels[index].clone(),
    }));
    tabs
}

pub fn entry_is_active(entry: &SessionEntry, activity: &ActivityMap, dismissed: &Dismissed) -> bool {
    !dismissed.contains(&entry.session_id)
        && activity::is_session_active(
            entry.live.as_ref().is_some_and(|live| live.pid_alive),
            activity.get(&entry.session_id),
        )
}

pub fn session_indices(
    key: &TabKey,
    entries: &[SessionEntry],
    favorites: &Favorites,
    activity: &ActivityMap,
    dismissed: &Dismissed,
    visible: &dyn Fn(&SessionEntry) -> bool,
) -> Vec<usize> {
    let mut indices: Vec<usize> = entries
        .iter()
        .enumerate()
        .filter(|(_, entry)| visible(entry))
        .filter(|(_, entry)| match key {
            TabKey::All => true,
            TabKey::Favorites => favorites.contains_entry(entry),
            TabKey::Active => entry_is_active(entry, activity, dismissed),
            TabKey::Dir(cwd) => &entry.cwd == cwd,
        })
        .map(|(index, _)| index)
        .collect();
    if *key == TabKey::Active {
        indices.sort_by_key(|&index| activity::attention_rank(activity.get(&entries[index].session_id)));
    }
    indices
}

fn directory_labels(directories: &[&str]) -> Vec<String> {
    let components: Vec<Vec<&str>> = directories
        .iter()
        .map(|dir| Path::new(dir).iter().filter_map(|c| c.to_str()).collect())
        .collect();
    let mut depth = vec![1usize; directories.len()];
    loop {
        let labels: Vec<String> = components
            .iter()
            .zip(&depth)
            .zip(directories)
            .map(|((parts, &depth), dir)| label_at_depth(parts, depth, dir))
            .collect();
        let mut by_label: HashMap<&str, Vec<usize>> = HashMap::new();
        for (index, label) in labels.iter().enumerate() {
            by_label.entry(label).or_default().push(index);
        }
        let mut deepened = false;
        for indices in by_label.values().filter(|indices| indices.len() > 1) {
            for &index in indices {
                if depth[index] < components[index].len() {
                    depth[index] += 1;
                    deepened = true;
                }
            }
        }
        if !deepened {
            return labels;
        }
    }
}

fn label_at_depth(parts: &[&str], depth: usize, dir: &str) -> String {
    let home = std::env::var("HOME").unwrap_or_default();
    if depth == 1 && !home.is_empty() && dir == home {
        return "~".to_owned();
    }
    let meaningful: Vec<&str> = parts.iter().copied().filter(|p| *p != "/").collect();
    if meaningful.is_empty() {
        return "/".to_owned();
    }
    let start = meaningful.len().saturating_sub(depth);
    meaningful[start..].join("/")
}

fn tab_width(tab: &Tab) -> usize {
    tab.label.chars().count() + TAB_PADDING
}

pub fn visible_directory_range(
    directory_tabs: &[Tab],
    anchor: usize,
    available: usize,
) -> Range<usize> {
    if directory_tabs.is_empty() {
        return 0..0;
    }
    let anchor = anchor.min(directory_tabs.len() - 1);
    let widths: Vec<usize> = directory_tabs.iter().map(tab_width).collect();
    let budget = available.saturating_sub(2 * OVERFLOW_MARKER_WIDTH);
    let mut start = anchor;
    let mut used = widths[anchor];
    while start > 0 && used + TAB_SEPARATOR + widths[start - 1] <= budget {
        start -= 1;
        used += TAB_SEPARATOR + widths[start];
    }
    let mut end = anchor + 1;
    while end < widths.len() && used + TAB_SEPARATOR + widths[end] <= budget {
        used += TAB_SEPARATOR + widths[end];
        end += 1;
    }
    if start == 0 && end == widths.len() {
        let total: usize = widths.iter().sum::<usize>() + TAB_SEPARATOR * (widths.len() - 1);
        if total <= available {
            return 0..widths.len();
        }
    }
    start..end
}

pub fn render_bar(tabs: &[Tab], active: &TabKey, width: usize, bar_focused: bool) -> Line<'static> {
    let pinned = &tabs[..PINNED_TABS.min(tabs.len())];
    let directories = tabs.get(PINNED_TABS..).unwrap_or(&[]);
    let pinned_width: usize = pinned.iter().map(|t| tab_width(t) + TAB_SEPARATOR).sum();
    if pinned_width > width {
        return narrow_bar(tabs, active, bar_focused);
    }
    let anchor = directories
        .iter()
        .position(|tab| &tab.key == active)
        .unwrap_or(0);
    let range = visible_directory_range(directories, anchor, width.saturating_sub(pinned_width));
    let mut spans = Vec::new();
    for tab in pinned {
        spans.push(tab_span(tab, active, bar_focused));
        spans.push(Span::raw(" "));
    }
    if range.start > 0 {
        spans.push(Span::styled(format!("‹{} ", range.start), theme::tab_overflow()));
    }
    for tab in &directories[range.clone()] {
        spans.push(tab_span(tab, active, bar_focused));
        spans.push(Span::raw(" "));
    }
    if range.end < directories.len() {
        spans.push(Span::styled(
            format!("{}›", directories.len() - range.end),
            theme::tab_overflow(),
        ));
    }
    Line::from(spans)
}

fn narrow_bar(tabs: &[Tab], active: &TabKey, bar_focused: bool) -> Line<'static> {
    let position = tabs.iter().position(|tab| &tab.key == active).unwrap_or(0);
    let mut spans = Vec::new();
    if let Some(tab) = tabs.get(position) {
        spans.push(tab_span(tab, active, bar_focused));
    }
    spans.push(Span::styled(
        format!(" {}/{} ‹ ›", position + 1, tabs.len()),
        theme::tab_overflow(),
    ));
    Line::from(spans)
}

fn tab_span(tab: &Tab, active: &TabKey, bar_focused: bool) -> Span<'static> {
    let is_active = &tab.key == active;
    let style: Style = if is_active && bar_focused {
        theme::tab_focused()
    } else if is_active {
        theme::tab_active()
    } else if tab.key == TabKey::Favorites {
        theme::tab_favorites_inactive()
    } else if tab.key == TabKey::Active {
        theme::tab_activity_inactive()
    } else {
        theme::tab_inactive()
    };
    let marker = if is_active && bar_focused { FOCUS_MARKER } else { " " };
    Span::styled(format!("{marker}{} ", tab.label), style)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::TitleSource;
    use std::{path::PathBuf, time::UNIX_EPOCH};

    fn entry(id: &str, cwd: &str) -> SessionEntry {
        SessionEntry {
            path: PathBuf::from(format!("/p/{id}.jsonl")),
            session_id: id.to_owned(),
            cwd: cwd.to_owned(),
            title: id.to_owned(),
            title_source: TitleSource::Prompt,
            last_prompt: None,
            last_used: UNIX_EPOCH,
            live: None,
            ..SessionEntry::stub()
        }
    }

    #[test]
    fn directory_tabs_follow_recency_and_disambiguate_collisions() {
        let entries = [
            entry("1", "/w/alpha/api"),
            entry("2", "/w/beta/api"),
            entry("3", "/w/alpha/api"),
            entry("4", "/w/web"),
        ];
        let labels: Vec<String> = build_tabs(&entries, &ActivityMap::new(), &Dismissed::default(), &FavoriteDirs::default(), &|_| true).into_iter().map(|t| t.label).collect();
        assert_eq!(labels, ["All", "★ Favorites", "Active", "alpha/api", "beta/api", "web"]);
    }

    #[test]
    fn window_keeps_the_active_tab_visible_when_the_bar_overflows() {
        let entries: Vec<SessionEntry> = (0..30)
            .map(|i| entry(&i.to_string(), &format!("/w/project-{i}")))
            .collect();
        let tabs = build_tabs(&entries, &ActivityMap::new(), &Dismissed::default(), &FavoriteDirs::default(), &|_| true);
        let directories = &tabs[PINNED_TABS..];
        for anchor in [0, 15, 29] {
            let range = visible_directory_range(directories, anchor, 60);
            assert!(range.contains(&anchor));
            assert!(range.len() < directories.len());
        }
    }

    #[test]
    fn active_tab_lists_every_running_session_with_blocked_first_then_busy_then_idle() {
        use crate::activity::{Activity, ActivityState};
        use crate::subagents::RunningAgent;
        let entries = [entry("idle", "/w"), entry("busy", "/w"), entry("blocked", "/w"), entry("agents", "/w"), entry("closed", "/w")];
        let mut activity = ActivityMap::new();
        let mut put = |id: &str, state: ActivityState, agents: usize| {
            activity.insert(
                id.to_owned(),
                Activity {
                    state,
                    background: false,
                    attach_id: None,
                    agents: (0..agents)
                        .map(|_| RunningAgent { agent_type: "explore".into(), description: String::new() })
                        .collect(),
                    last_activity: None,
                },
            );
        };
        put("idle", ActivityState::Idle, 0);
        put("busy", ActivityState::Busy, 0);
        put("blocked", ActivityState::Blocked, 0);
        put("agents", ActivityState::Idle, 2);
        let favorites = Favorites::load(std::env::temp_dir().join(format!("ccview-tabs-{}/f.json", std::process::id())));
        let rows = session_indices(&TabKey::Active, &entries, &favorites, &activity, &Dismissed::default(), &|_| true);
        let ids: Vec<&str> = rows.iter().map(|&i| entries[i].session_id.as_str()).collect();
        assert_eq!(ids, ["blocked", "busy", "agents", "idle"]);
        let labels: Vec<String> = build_tabs(&entries, &activity, &Dismissed::default(), &FavoriteDirs::default(), &|_| true).into_iter().map(|t| t.label).collect();
        assert_eq!(labels[2], "Active 4");
    }
}
