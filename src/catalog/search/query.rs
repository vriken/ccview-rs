use crate::session::harness_id::HarnessId;

pub(crate) const MAX_INPUT_CHARS: usize = 400;
const MAX_TERMS: usize = 12;
const MIN_PREFIX_CHARS: usize = 2;
const TOOL_COLUMN: &str = "tool";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Term {
    pub(crate) raw: Vec<String>,
    pub(crate) folded: Vec<String>,
    pub(crate) prefix: bool,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Query {
    pub(crate) terms: Vec<Term>,
    pub(crate) excluded: Vec<Term>,
    pub(crate) title: Vec<String>,
    pub(crate) dir: Vec<String>,
    pub(crate) harness: Vec<String>,
    pub(crate) columns: Vec<&'static str>,
}

struct Lexeme {
    negated: bool,
    field: Option<String>,
    text: String,
    quoted: bool,
}

impl Query {
    pub(crate) fn parse(input: &str) -> Self {
        let mut query = Query::default();
        for lexeme in lex(input) {
            query.add(lexeme);
        }
        query
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.terms.is_empty()
            && self.excluded.is_empty()
            && self.title.is_empty()
            && self.dir.is_empty()
            && self.harness.is_empty()
    }

    pub(crate) fn fts(&self) -> Option<String> {
        if self.terms.is_empty() {
            return None;
        }
        let positives = self
            .terms
            .iter()
            .map(phrase)
            .collect::<Vec<_>>()
            .join(" AND ");
        let mut expression = format!("({positives})");
        if !self.excluded.is_empty() {
            let excluded = self
                .excluded
                .iter()
                .map(phrase)
                .collect::<Vec<_>>()
                .join(" OR ");
            expression = format!("{expression} NOT ({excluded})");
        }
        if !self.columns.is_empty() {
            expression = format!("{{{}}} : ({expression})", self.columns.join(" "));
        }
        Some(expression)
    }

    pub(crate) fn scoped_to_columns(&self) -> bool {
        !self.columns.is_empty()
    }

    pub(crate) fn admits(&self, title: &str, cwd: &str, harness: HarnessId) -> bool {
        if self.title.is_empty() && self.dir.is_empty() && self.harness.is_empty() {
            return true;
        }
        let title = fold(title);
        let cwd = fold(cwd);
        self.title
            .iter()
            .all(|wanted| title.contains(wanted.as_str()))
            && self.dir.iter().all(|wanted| cwd.contains(wanted.as_str()))
            && (self.harness.is_empty()
                || self.harness.iter().any(|wanted| wanted == harness.as_str()))
    }

    fn add(&mut self, lexeme: Lexeme) {
        match lexeme.field.as_deref() {
            Some(_) if lexeme.negated => {}
            Some("in") => self.add_columns(&lexeme.text),
            Some(field) => {
                let value = fold(lexeme.text.trim());
                if value.is_empty() {
                    return;
                }
                match field {
                    "title" => self.title.push(value),
                    "dir" => self.dir.push(value),
                    _ => self.harness.push(value),
                }
            }
            None => self.add_term(lexeme),
        }
    }

    fn add_columns(&mut self, names: &str) {
        for name in names.split(',') {
            let column = match name.trim().to_lowercase().as_str() {
                "prompt" => "user",
                "reply" => "assistant",
                "tool" => TOOL_COLUMN,
                _ => continue,
            };
            if !self.columns.contains(&column) {
                self.columns.push(column);
            }
        }
    }

    fn add_term(&mut self, lexeme: Lexeme) {
        let raw = tokens(&lexeme.text);
        let Some(last) = raw.last() else {
            return;
        };
        let prefix = !lexeme.quoted
            && lexeme.text.ends_with('*')
            && last.chars().count() >= MIN_PREFIX_CHARS;
        let term = Term {
            folded: raw.iter().map(|token| fold(token)).collect(),
            raw,
            prefix,
        };
        if lexeme.negated {
            self.excluded.push(term);
        } else if self.terms.len() < MAX_TERMS {
            self.terms.push(term);
        }
    }
}

pub(crate) fn tokens(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|token| !token.is_empty())
        .map(str::to_owned)
        .collect()
}

pub(crate) fn fold(text: &str) -> String {
    text.chars()
        .map(|letter| base_letter(letter.to_lowercase().next().unwrap_or(letter)))
        .collect()
}

fn base_letter(letter: char) -> char {
    match letter {
        'à'..='å' | 'ā' | 'ă' | 'ą' => 'a',
        'ç' | 'ć' | 'ĉ' | 'ċ' | 'č' => 'c',
        'ď' => 'd',
        'è'..='ë' | 'ē' | 'ĕ' | 'ė' | 'ę' | 'ě' => 'e',
        'ĝ' | 'ğ' | 'ġ' | 'ģ' => 'g',
        'ĥ' => 'h',
        'ĵ' => 'j',
        'ķ' => 'k',
        'ĺ' | 'ļ' | 'ľ' => 'l',
        'ŵ' => 'w',
        'ì'..='ï' | 'ĩ' | 'ī' | 'ĭ' | 'į' => 'i',
        'ñ' | 'ń' | 'ņ' | 'ň' => 'n',
        'ò'..='ö' | 'ō' | 'ŏ' | 'ő' => 'o',
        'ŕ' | 'ŗ' | 'ř' => 'r',
        'ś' | 'ŝ' | 'ş' | 'š' | 'ſ' => 's',
        'ţ' | 'ť' => 't',
        'ù'..='ü' | 'ũ' | 'ū' | 'ŭ' | 'ů' | 'ű' | 'ų' => 'u',
        'ý' | 'ÿ' | 'ŷ' => 'y',
        'ź' | 'ż' | 'ž' => 'z',
        other => other,
    }
}

fn phrase(term: &Term) -> String {
    let star = if term.prefix { " *" } else { "" };
    format!("\"{}\"{star}", quote(&term.raw.join(" ")))
}

fn quote(text: &str) -> String {
    text.replace('"', "\"\"")
}

fn lex(input: &str) -> Vec<Lexeme> {
    let chars: Vec<char> = input.chars().take(MAX_INPUT_CHARS).collect();
    let mut lexemes = Vec::new();
    let mut at = 0;
    while at < chars.len() {
        if chars[at].is_whitespace() {
            at += 1;
            continue;
        }
        let negated = chars[at] == '-';
        if negated {
            at += 1;
        }
        let field = field_at(&chars, &mut at);
        let quoted = chars.get(at) == Some(&'"');
        let text = if quoted {
            at += 1;
            take_while(&chars, &mut at, |c| c != '"')
        } else {
            take_while(&chars, &mut at, |c| !c.is_whitespace())
        };
        if quoted && chars.get(at) == Some(&'"') {
            at += 1;
        }
        lexemes.push(Lexeme {
            negated,
            field,
            text,
            quoted,
        });
    }
    lexemes
}

fn field_at(chars: &[char], at: &mut usize) -> Option<String> {
    let start = *at;
    let name = take_while(chars, at, |c| c.is_alphabetic()).to_lowercase();
    let known = matches!(name.as_str(), "title" | "dir" | "harness" | "in");
    if known && chars.get(*at) == Some(&':') {
        *at += 1;
        return Some(name);
    }
    *at = start;
    None
}

fn take_while(chars: &[char], at: &mut usize, keep: impl Fn(char) -> bool) -> String {
    let start = *at;
    while *at < chars.len() && keep(chars[*at]) {
        *at += 1;
    }
    chars[start..*at].iter().collect()
}
