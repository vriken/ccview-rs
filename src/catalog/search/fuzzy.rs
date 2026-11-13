const MATCH: i32 = 16;
const CONSECUTIVE: i32 = 12;
const WORD_START: i32 = 10;
const GAP: i32 = 3;
const MIN_PER_CHARACTER: i32 = 20;

pub(crate) fn score(needle: &[char], haystack: &[char]) -> Option<i32> {
    if needle.is_empty() {
        return None;
    }
    let mut previous: Vec<Option<i32>> = vec![None; haystack.len()];
    for (index, &wanted) in needle.iter().enumerate() {
        let mut current: Vec<Option<i32>> = vec![None; haystack.len()];
        let mut best_gapped: Option<i32> = None;
        for column in 0..haystack.len() {
            if column > 0
                && let Some(score) = previous[column - 1]
            {
                best_gapped = best_gapped.max(Some(score + GAP * (column as i32 - 1)));
            }
            if haystack[column] != wanted {
                continue;
            }
            let before = if index == 0 {
                Some(0)
            } else {
                let gapped = best_gapped.map(|best| best - GAP * (column as i32 - 1));
                let adjacent = column
                    .checked_sub(1)
                    .and_then(|left| previous[left])
                    .map(|score| score + CONSECUTIVE);
                gapped.max(adjacent)
            };
            let word_start = column == 0 || !haystack[column - 1].is_alphanumeric();
            let bonus = MATCH + if word_start { WORD_START } else { 0 };
            current[column] = before.map(|score| score + bonus);
        }
        previous = current;
    }
    previous
        .into_iter()
        .flatten()
        .max()
        .filter(|score| *score >= MIN_PER_CHARACTER * needle.len() as i32)
}
