use super::{
    keys::{self, Zone},
    overlay::{self, Body, Overlay},
    theme,
};
use crossterm::event::KeyCode;
use ratatui::text::{Line, Span};

const KEY_COLUMN: usize = 14;

fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut lines = vec![String::new()];
    for word in text.split_whitespace() {
        let current = lines.last_mut().expect("never empty");
        if !current.is_empty() && current.chars().count() + 1 + word.chars().count() > width {
            lines.push(word.to_owned());
        } else {
            if !current.is_empty() {
                current.push(' ');
            }
            current.push_str(word);
        }
    }
    lines
}

pub fn body(width: usize) -> Body {
    let text_width = width.saturating_sub(KEY_COLUMN + 2).max(8);
    let mut lines = Vec::new();
    for zone in Zone::HELP_ORDER {
        if !lines.is_empty() {
            lines.push(Line::raw(""));
        }
        lines.push(Line::styled(zone.title(), theme::zone_title(true, None)));
        for hint in keys::zone_keys(zone) {
            let described = if hint.note.is_empty() {
                hint.label.to_owned()
            } else {
                format!("{} — {}", hint.label, hint.note)
            };
            for (index, text) in wrap(&described, text_width).into_iter().enumerate() {
                let key = if index == 0 { hint.keys } else { "" };
                lines.push(Line::from(vec![
                    Span::styled(format!("  {key:<KEY_COLUMN$}"), theme::footer_key()),
                    Span::raw(text),
                ]));
            }
        }
    }
    Body {
        title: "Help".to_owned(),
        lines,
        hint: "↑/↓ PgUp/PgDn scroll · Esc close".to_owned(),
        follow: None,
        fit: false,
    }
}

pub fn key(overlay: &mut Overlay, code: KeyCode) -> bool {
    match code {
        KeyCode::Esc => return false,
        KeyCode::Up => overlay::scroll(overlay, true, 1),
        KeyCode::Down => overlay::scroll(overlay, false, 1),
        KeyCode::PageUp => overlay::scroll(overlay, true, overlay::page(overlay)),
        KeyCode::PageDown => overlay::scroll(overlay, false, overlay::page(overlay)),
        _ => {}
    }
    true
}
