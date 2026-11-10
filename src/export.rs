use crate::{
    discover,
    parse::{self, Block, Message, Role, Session},
};
use std::{
    fmt::Write,
    fs,
    path::{Path, PathBuf},
    process::ExitCode,
};

const MAX_RESULT_CHARS: usize = 20_000;
const USAGE: &str =
    "usage: ccview-rs --export <out.html|out.md|out.jsonl> [--file <session.jsonl>]";

const HTML_STYLE: &str = "\
body{background:#12141a;color:#d8dbe2;font:14px/1.5 system-ui,sans-serif;max-width:60rem;margin:0 auto;padding:1rem}\
h1{font-size:1.1rem;color:#9aa3b5;font-weight:600}\
.msg{border-radius:8px;padding:.6rem .9rem;margin:.8rem 0;border-left:4px solid}\
.user{background:#1b2333;border-color:#4c8dff}\
.assistant{background:#1d1f27;border-color:#b58cff}\
.head{font-size:.75rem;color:#8a93a6;margin-bottom:.4rem;text-transform:uppercase;letter-spacing:.05em}\
.head time{margin-left:.6rem;text-transform:none}\
pre{white-space:pre-wrap;word-break:break-word;margin:.3rem 0;font:13px/1.45 ui-monospace,Menlo,monospace}\
details{margin:.4rem 0;color:#9aa3b5}\
summary{cursor:pointer}\
.tool{background:#14261f;border:1px solid #2d6a4f;border-radius:6px;padding:.4rem .7rem;margin:.4rem 0}\
.tool .name{color:#6fdc9e;font-weight:600}\
.note{color:#e0a458;font-style:italic}";

pub fn run(args: &[String]) -> ExitCode {
    match export(args) {
        Ok(summary) => {
            eprintln!("{summary}");
            ExitCode::SUCCESS
        }
        Err(message) => {
            eprintln!("{message}");
            ExitCode::FAILURE
        }
    }
}

fn export(args: &[String]) -> Result<String, String> {
    let mut out = None;
    let mut file = None;
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--export" => out = Some(PathBuf::from(iter.next().ok_or(USAGE)?)),
            "--file" => file = Some(PathBuf::from(iter.next().ok_or(USAGE)?)),
            _ => return Err(USAGE.to_owned()),
        }
    }
    let out = out.ok_or(USAGE)?;
    let format = Format::from_path(&out)?;
    let source = match file {
        Some(path) => path,
        None => newest_session()?,
    };
    match format {
        Format::Jsonl => {
            fs::copy(&source, &out).map_err(|e| format!("{}: {e}", source.display()))?;
        }
        Format::Html | Format::Markdown => {
            let session =
                parse::parse_file(&source).map_err(|e| format!("{}: {e}", source.display()))?;
            let rendered = match format {
                Format::Html => render_html(&session, &source),
                _ => render_markdown(&session),
            };
            fs::write(&out, rendered).map_err(|e| format!("{}: {e}", out.display()))?;
        }
    }
    Ok(format!("{} -> {}", source.display(), out.display()))
}

fn newest_session() -> Result<PathBuf, String> {
    let dir = discover::claude_projects_dir().ok_or("HOME is not set")?;
    discover::find_sessions(&dir)
        .into_iter()
        .next()
        .ok_or_else(|| format!("no sessions under {}", dir.display()))
}

#[derive(Clone, Copy)]
enum Format {
    Html,
    Markdown,
    Jsonl,
}

impl Format {
    fn from_path(path: &Path) -> Result<Self, String> {
        match path.extension().and_then(|e| e.to_str()) {
            Some("html") => Ok(Self::Html),
            Some("md") => Ok(Self::Markdown),
            Some("jsonl") => Ok(Self::Jsonl),
            _ => Err(format!(
                "unsupported export extension for {}: use .html, .md or .jsonl",
                path.display()
            )),
        }
    }
}

fn visible(session: &Session) -> impl Iterator<Item = &Message> {
    session.messages.iter().filter(|m| !m.is_meta)
}

fn role_name(role: Role) -> &'static str {
    match role {
        Role::User => "User",
        Role::Assistant => "Assistant",
    }
}

fn truncate(text: &str) -> (&str, Option<usize>) {
    match text.char_indices().nth(MAX_RESULT_CHARS) {
        Some((cut, _)) => (&text[..cut], Some(text[cut..].chars().count())),
        None => (text, None),
    }
}

fn pretty_json(value: &serde_json::Value) -> String {
    serde_json::to_string_pretty(value).unwrap_or_default()
}

fn escape_html(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&#39;"),
            _ => escaped.push(c),
        }
    }
    escaped
}

fn render_html(session: &Session, source: &Path) -> String {
    let title = escape_html(&source.file_name().unwrap_or_default().to_string_lossy());
    let mut html = String::new();
    let _ = write!(
        html,
        "<!DOCTYPE html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n<title>{title}</title>\n<style>{HTML_STYLE}</style>\n</head>\n<body>\n<h1>{title}</h1>\n"
    );
    for message in visible(session) {
        let class = match message.role {
            Role::User => "user",
            Role::Assistant => "assistant",
        };
        let _ = write!(
            html,
            "<div class=\"msg {class}\">\n<div class=\"head\">{}",
            role_name(message.role)
        );
        if let Some(timestamp) = &message.timestamp {
            let escaped = escape_html(timestamp);
            let _ = write!(html, "<time datetime=\"{escaped}\">{escaped}</time>");
        }
        html.push_str("</div>\n");
        for block in &message.blocks {
            html_block(&mut html, block);
        }
        html.push_str("</div>\n");
    }
    html.push_str("</body>\n</html>\n");
    html
}

fn html_block(html: &mut String, block: &Block) {
    match block {
        Block::Text(text) => {
            let _ = writeln!(html, "<pre>{}</pre>", escape_html(text));
        }
        Block::Thinking(text) => {
            let _ = writeln!(
                html,
                "<details><summary>thinking</summary><pre>{}</pre></details>",
                escape_html(text)
            );
        }
        Block::ToolUse { name, input } => {
            let _ = writeln!(
                html,
                "<div class=\"tool\"><div class=\"name\">{}</div><pre>{}</pre></div>",
                escape_html(name),
                escape_html(&pretty_json(input))
            );
        }
        Block::ToolResult(text) => {
            let (shown, omitted) = truncate(text);
            let _ = write!(
                html,
                "<details><summary>tool result</summary><pre>{}</pre>",
                escape_html(shown)
            );
            if let Some(omitted) = omitted {
                let _ = write!(
                    html,
                    "<div class=\"note\">truncated: {omitted} more characters omitted</div>"
                );
            }
            html.push_str("</details>\n");
        }
    }
}

fn render_markdown(session: &Session) -> String {
    let mut md = String::new();
    for message in visible(session) {
        let _ = write!(md, "## {}", role_name(message.role));
        if let Some(timestamp) = &message.timestamp {
            let _ = write!(md, " ({timestamp})");
        }
        md.push_str("\n\n");
        for block in &message.blocks {
            markdown_block(&mut md, block);
        }
    }
    md
}

fn markdown_block(md: &mut String, block: &Block) {
    match block {
        Block::Text(text) => {
            md.push_str(text);
            md.push_str("\n\n");
        }
        Block::Thinking(_) => {}
        Block::ToolUse { name, input } => {
            let mut quoted = format!("**Tool: {name}**\n\n");
            quoted.push_str(&fenced("json", &pretty_json(input)));
            push_blockquote(md, &quoted);
        }
        Block::ToolResult(text) => {
            let (shown, omitted) = truncate(text);
            let mut quoted = String::from("**Tool result**\n\n");
            quoted.push_str(&fenced("", shown));
            if let Some(omitted) = omitted {
                let _ = write!(quoted, "\n_truncated: {omitted} more characters omitted_\n");
            }
            push_blockquote(md, &quoted);
        }
    }
}

fn push_blockquote(md: &mut String, body: &str) {
    for line in body.trim_end().lines() {
        if line.is_empty() {
            md.push_str(">\n");
        } else {
            let _ = writeln!(md, "> {line}");
        }
    }
    md.push('\n');
}

fn fenced(language: &str, content: &str) -> String {
    let longest_backtick_run = content
        .split(|c| c != '`')
        .map(str::len)
        .max()
        .unwrap_or(0);
    let fence = "`".repeat((longest_backtick_run + 1).max(3));
    format!("{fence}{language}\n{content}\n{fence}\n")
}
