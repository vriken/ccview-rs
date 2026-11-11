use crate::tui::reader::links::{self, LinkRange};
use unicode_width::UnicodeWidthChar;

pub(super) fn display_width(text: &str) -> usize {
    text.chars().map(|c| c.width().unwrap_or(0)).sum()
}

pub(super) fn split_at_width(word: &str, width: usize) -> (&str, &str) {
    let mut used = 0;
    for (index, c) in word.char_indices() {
        let char_width = c.width().unwrap_or(0);
        if used + char_width > width && index > 0 {
            return word.split_at(index);
        }
        used += char_width;
    }
    (word, "")
}

#[derive(Default)]
pub(super) struct Wrapped {
    pub(crate) text: String,
    pub(crate) links: Vec<LinkRange>,
}

pub(super) fn wrap(text: &str, width: usize) -> Vec<Wrapped> {
    let indent: String = text.chars().take_while(|c| *c == ' ').collect();
    let mut rows = Vec::new();
    let mut current = Wrapped::default();
    let mut current_len = 0;
    for word in text.split(' ').filter(|w| !w.is_empty()) {
        let urls = links::link_ranges(word);
        let mut consumed = 0;
        let mut word = word;
        let mut word_len = display_width(word);
        if current_len > 0 && current_len + 1 + word_len > width {
            rows.push(std::mem::take(&mut current));
            current_len = 0;
        }
        while word_len > width {
            let (head, tail) = split_at_width(word, width);
            let head_chars = head.chars().count();
            word = tail;
            word_len = display_width(word);
            if current_len > 0 {
                rows.push(std::mem::take(&mut current));
                current_len = 0;
            }
            rows.push(Wrapped {
                text: head.to_owned(),
                links: links::clip(&urls, consumed, consumed + head_chars, 0),
            });
            consumed += head_chars;
        }
        if current_len > 0 {
            current.text.push(' ');
            current_len += 1;
        } else {
            current.text.push_str(&indent);
            current_len = indent.chars().count();
        }
        let offset = current.text.chars().count();
        current
            .links
            .extend(links::clip(&urls, consumed, usize::MAX, offset));
        current.text.push_str(word);
        current_len += word_len;
    }
    rows.push(current);
    rows
}
