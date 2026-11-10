const MAX_TABLE_CELLS: usize = 4_000_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Change<'a> {
    Same(&'a str),
    Removed(&'a str),
    Added(&'a str),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shown<'a> {
    Line(Change<'a>),
    Gap,
}

pub fn line_diff<'a>(old: &'a str, new: &'a str) -> Vec<Change<'a>> {
    let old: Vec<&str> = old.lines().collect();
    let new: Vec<&str> = new.lines().collect();
    let prefix = old.iter().zip(&new).take_while(|(a, b)| a == b).count();
    let suffix = old[prefix..]
        .iter()
        .rev()
        .zip(new[prefix..].iter().rev())
        .take_while(|(a, b)| a == b)
        .count();
    let old_mid = &old[prefix..old.len() - suffix];
    let new_mid = &new[prefix..new.len() - suffix];

    let mut changes: Vec<Change> = old[..prefix].iter().map(|l| Change::Same(l)).collect();
    changes.extend(diff_middle(old_mid, new_mid));
    changes.extend(old[old.len() - suffix..].iter().map(|l| Change::Same(l)));
    changes
}

fn diff_middle<'a>(old: &[&'a str], new: &[&'a str]) -> Vec<Change<'a>> {
    if old.is_empty() || new.is_empty() || old.len() * new.len() > MAX_TABLE_CELLS {
        return old
            .iter()
            .map(|l| Change::Removed(l))
            .chain(new.iter().map(|l| Change::Added(l)))
            .collect();
    }
    let columns = new.len() + 1;
    let mut common = vec![0u32; (old.len() + 1) * columns];
    for i in (0..old.len()).rev() {
        for j in (0..new.len()).rev() {
            common[i * columns + j] = if old[i] == new[j] {
                common[(i + 1) * columns + j + 1] + 1
            } else {
                common[(i + 1) * columns + j].max(common[i * columns + j + 1])
            };
        }
    }
    let (mut i, mut j) = (0, 0);
    let mut changes = Vec::new();
    while i < old.len() && j < new.len() {
        if old[i] == new[j] {
            changes.push(Change::Same(old[i]));
            i += 1;
            j += 1;
        } else if common[(i + 1) * columns + j] >= common[i * columns + j + 1] {
            changes.push(Change::Removed(old[i]));
            i += 1;
        } else {
            changes.push(Change::Added(new[j]));
            j += 1;
        }
    }
    changes.extend(old[i..].iter().map(|l| Change::Removed(l)));
    changes.extend(new[j..].iter().map(|l| Change::Added(l)));
    changes
}

pub fn additions<'a>(text: &'a str) -> Vec<Change<'a>> {
    text.lines().map(Change::Added).collect()
}

pub fn count(changes: &[Change]) -> (usize, usize) {
    changes
        .iter()
        .fold((0, 0), |(added, removed), change| match change {
            Change::Added(_) => (added + 1, removed),
            Change::Removed(_) => (added, removed + 1),
            Change::Same(_) => (added, removed),
        })
}

pub fn with_context<'a>(changes: &[Change<'a>], context: usize) -> Vec<Shown<'a>> {
    let changed: Vec<usize> = changes
        .iter()
        .enumerate()
        .filter(|(_, c)| !matches!(c, Change::Same(_)))
        .map(|(i, _)| i)
        .collect();
    let mut shown = Vec::new();
    let mut next_visible = 0;
    for (index, change) in changes.iter().enumerate() {
        let near_change = changed.iter().any(|&c| c.abs_diff(index) <= context);
        if near_change {
            if index > next_visible && !shown.is_empty() {
                shown.push(Shown::Gap);
            }
            shown.push(Shown::Line(*change));
            next_visible = index + 1;
        }
    }
    shown
}

#[cfg(test)]
mod tests {
    use super::*;
    use Change::{Added, Removed, Same};

    #[test]
    fn inserted_line_is_kept_aligned_with_surrounding_lines() {
        assert_eq!(
            line_diff("a\nc\nd", "a\nb\nc\nd"),
            vec![Same("a"), Added("b"), Same("c"), Same("d")]
        );
    }

    #[test]
    fn oversized_inputs_fall_back_to_remove_all_then_add_all() {
        let old: Vec<String> = (0..2100).map(|i| format!("o{i}")).collect();
        let new: Vec<String> = (0..2100).map(|i| format!("n{i}")).collect();
        let (old, new) = (old.join("\n"), new.join("\n"));
        let changes = line_diff(&old, &new);
        assert_eq!(count(&changes), (2100, 2100));
        assert!(matches!(changes[0], Removed(_)) && matches!(changes[2100], Added(_)));
    }
}
