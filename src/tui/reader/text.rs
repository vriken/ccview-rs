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

pub(super) fn wrap(text: &str, width: usize) -> Vec<String> {
    let indent: String = text.chars().take_while(|c| *c == ' ').collect();
    let mut lines = Vec::new();
    let mut current = String::new();
    let mut current_len = 0;
    for word in text.split(' ').filter(|w| !w.is_empty()) {
        let mut word = word;
        let mut word_len = display_width(word);
        if current_len > 0 && current_len + 1 + word_len > width {
            lines.push(std::mem::take(&mut current));
            current_len = 0;
        }
        while word_len > width {
            let (head, tail) = split_at_width(word, width);
            word = tail;
            word_len = display_width(word);
            if current_len > 0 {
                lines.push(std::mem::take(&mut current));
                current_len = 0;
            }
            lines.push(head.to_owned());
        }
        if current_len > 0 {
            current.push(' ');
            current_len += 1;
        } else {
            current.push_str(&indent);
            current_len = indent.chars().count();
        }
        current.push_str(word);
        current_len += word_len;
    }
    lines.push(current);
    lines
}
