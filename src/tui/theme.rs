use crate::{
    activity::{ActivityState, Badge},
    registry::Status,
};
use crate::markdown;
use ratatui::style::{Color, Modifier, Style};

const ACCENT: Color = Color::Indexed(110);
const DIM: Color = Color::Indexed(244);
const FAINT: Color = Color::Indexed(240);
const SELECTION_BG: Color = Color::Indexed(236);
const STAR: Color = Color::Indexed(179);
const BUSY: Color = Color::Indexed(214);
const IDLE: Color = Color::Indexed(78);
const OTHER: Color = Color::Indexed(176);
const ATTENTION: Color = Color::Indexed(196);
const AGENTS: Color = Color::Indexed(75);
const BACKGROUND: Color = Color::Indexed(141);
const SHELL: Color = Color::Indexed(180);
const TOOL: Color = Color::Indexed(137);
const WARNING: Color = Color::Indexed(209);
const CODE_FG: Color = Color::Indexed(252);
const CODE_BG: Color = Color::Indexed(235);
const INLINE_CODE_FG: Color = Color::Indexed(216);
const DIFF_ADDED_FG: Color = Color::Indexed(120);
const DIFF_ADDED_BG: Color = Color::Indexed(22);
const DIFF_REMOVED_FG: Color = Color::Indexed(217);
const DIFF_REMOVED_BG: Color = Color::Indexed(52);
const TOOL_ARGS: Color = Color::Indexed(246);

pub fn tab_active() -> Style {
    Style::default()
        .fg(Color::Black)
        .bg(ACCENT)
        .add_modifier(Modifier::BOLD)
}

pub fn tab_inactive() -> Style {
    Style::default().fg(DIM)
}

pub fn tab_favorites_inactive() -> Style {
    Style::default().fg(STAR)
}

pub fn tab_overflow() -> Style {
    Style::default().fg(FAINT)
}

pub fn border(focused: bool) -> Style {
    Style::default().fg(if focused { ACCENT } else { FAINT })
}

pub fn pane_title(focused: bool) -> Style {
    if focused {
        Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(DIM)
    }
}

pub fn row_selected() -> Style {
    Style::default().bg(SELECTION_BG)
}

pub fn row_title(selected: bool) -> Style {
    if selected {
        Style::default().add_modifier(Modifier::BOLD)
    } else {
        Style::default()
    }
}

pub fn secondary() -> Style {
    Style::default().fg(DIM)
}

pub fn star() -> Style {
    Style::default().fg(STAR)
}

pub fn status_badge(status: &Status) -> Style {
    let color = match status {
        Status::Busy => BUSY,
        Status::Idle => IDLE,
        Status::Other(_) | Status::Unknown => OTHER,
    };
    Style::default().fg(color)
}

pub fn activity_dot(state: &ActivityState) -> Style {
    match state {
        ActivityState::Blocked => Style::default().fg(ATTENTION),
        ActivityState::Busy => Style::default().fg(BUSY),
        ActivityState::Idle => Style::default().fg(IDLE),
        ActivityState::Shell => Style::default().fg(SHELL),
        ActivityState::Other(_) | ActivityState::Unknown => Style::default().fg(OTHER),
    }
}

pub fn badge(badge: &Badge) -> Style {
    match badge {
        Badge::NeedsYou => needs_you(),
        Badge::Agents(_) => Style::default().fg(AGENTS),
        Badge::Background => Style::default().fg(BACKGROUND),
        Badge::Shell => Style::default().fg(SHELL),
    }
}

pub fn needs_you() -> Style {
    Style::default()
        .fg(Color::Black)
        .bg(ATTENTION)
        .add_modifier(Modifier::BOLD)
}

pub fn summary(attention: bool) -> Style {
    if attention {
        Style::default().fg(ATTENTION).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(DIM)
    }
}

pub fn tab_activity_inactive() -> Style {
    Style::default().fg(BUSY)
}

pub fn footer_key() -> Style {
    Style::default().fg(ACCENT)
}

pub fn footer_text() -> Style {
    Style::default().fg(DIM)
}

pub fn notice() -> Style {
    Style::default().fg(WARNING)
}

pub fn empty_state() -> Style {
    Style::default().fg(DIM)
}

pub fn user_label() -> Style {
    Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)
}

pub fn user_text() -> Style {
    Style::default().fg(ACCENT)
}

pub fn assistant_label() -> Style {
    Style::default().fg(IDLE).add_modifier(Modifier::BOLD)
}

pub fn assistant_text() -> Style {
    Style::default()
}

pub fn tool_result() -> Style {
    Style::default().fg(FAINT)
}

pub fn tool_bullet() -> Style {
    Style::default().fg(DIM)
}

pub fn tool_name() -> Style {
    Style::default().fg(TOOL).add_modifier(Modifier::BOLD)
}

pub fn tool_args() -> Style {
    Style::default().fg(TOOL_ARGS)
}

pub fn tool_connector() -> Style {
    Style::default().fg(FAINT)
}

pub fn thinking() -> Style {
    Style::default().fg(DIM).add_modifier(Modifier::ITALIC)
}

pub fn system_label() -> Style {
    Style::default().fg(DIM).add_modifier(Modifier::BOLD)
}

pub fn day_divider() -> Style {
    Style::default().fg(FAINT)
}

pub fn diff_added() -> Style {
    Style::default().fg(DIFF_ADDED_FG).bg(DIFF_ADDED_BG)
}

pub fn diff_removed() -> Style {
    Style::default().fg(DIFF_REMOVED_FG).bg(DIFF_REMOVED_BG)
}

pub fn diff_context() -> Style {
    Style::default().fg(DIM)
}

pub fn diff_header() -> Style {
    Style::default().fg(DIM)
}

pub fn markdown() -> markdown::Theme {
    let heading = Style::default().add_modifier(Modifier::BOLD);
    markdown::Theme {
        h1: heading.fg(ACCENT).add_modifier(Modifier::UNDERLINED),
        h2: heading.fg(ACCENT),
        h3: heading.fg(IDLE),
        h4: heading,
        strong: Style::default().add_modifier(Modifier::BOLD),
        emphasis: Style::default().add_modifier(Modifier::ITALIC),
        strikethrough: Style::default().add_modifier(Modifier::CROSSED_OUT),
        inline_code: Style::default().fg(INLINE_CODE_FG).bg(SELECTION_BG),
        code_block: Style::default().fg(CODE_FG).bg(CODE_BG),
        quote: Style::default().fg(DIM),
        link: Style::default().fg(ACCENT).add_modifier(Modifier::UNDERLINED),
        list_marker: Style::default().fg(ACCENT),
        rule: Style::default().fg(FAINT),
        table_border: Style::default().fg(FAINT),
        dim: Style::default().fg(DIM),
    }
}
