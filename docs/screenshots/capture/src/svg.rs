use std::fmt::Write;

const CELL_W: f64 = 8.4;
const CELL_H: f64 = 18.0;
const FONT_SIZE: f64 = 14.0;
const PAD: f64 = 14.0;
const BASELINE: f64 = 13.0;
const FONT_STACK: &str = "SFMono-Regular, Menlo, Consolas, 'DejaVu Sans Mono', 'Liberation Mono', monospace";
const BACKGROUND: (u8, u8, u8) = (0x1b, 0x1d, 0x23);
const FOREGROUND: (u8, u8, u8) = (0xd4, 0xd7, 0xdd);
const ANSI: [(u8, u8, u8); 16] = [
    (0x1b, 0x1d, 0x23), (0xe0, 0x6c, 0x75), (0x98, 0xc3, 0x79), (0xe5, 0xc0, 0x7b),
    (0x61, 0xaf, 0xef), (0xc6, 0x78, 0xdd), (0x56, 0xb6, 0xc2), (0xd4, 0xd7, 0xdd),
    (0x5c, 0x63, 0x70), (0xff, 0x7b, 0x86), (0xb0, 0xe0, 0x90), (0xf0, 0xd0, 0x8a),
    (0x7c, 0xc0, 0xff), (0xd8, 0x8c, 0xee), (0x6c, 0xd0, 0xdc), (0xff, 0xff, 0xff),
];
const DIM_BLEND: f64 = 0.55;

type Rgb = (u8, u8, u8);

fn palette(index: u8) -> Rgb {
    match index {
        0..=15 => ANSI[index as usize],
        16..=231 => {
            let n = index - 16;
            let level = |v: u8| if v == 0 { 0 } else { 55 + v * 40 };
            (level(n / 36), level((n / 6) % 6), level(n % 6))
        }
        _ => {
            let gray = 8 + (index - 232) * 10;
            (gray, gray, gray)
        }
    }
}

fn resolve(color: vt100::Color, default: Rgb) -> Rgb {
    match color {
        vt100::Color::Default => default,
        vt100::Color::Idx(index) => palette(index),
        vt100::Color::Rgb(r, g, b) => (r, g, b),
    }
}

fn blend(from: Rgb, to: Rgb, amount: f64) -> Rgb {
    let mix = |a: u8, b: u8| (a as f64 * amount + b as f64 * (1.0 - amount)).round() as u8;
    (mix(from.0, to.0), mix(from.1, to.1), mix(from.2, to.2))
}

fn hex(color: Rgb) -> String {
    format!("#{:02x}{:02x}{:02x}", color.0, color.1, color.2)
}

#[derive(Clone, PartialEq)]
struct Style {
    fg: Rgb,
    bg: Rgb,
    bold: bool,
    italic: bool,
    underline: bool,
}

struct Cell {
    ch: String,
    style: Style,
    wide: bool,
}

fn cell_at(screen: &vt100::Screen, row: u16, col: u16) -> Option<Cell> {
    let cell = screen.cell(row, col)?;
    if cell.is_wide_continuation() {
        return None;
    }
    let (mut fg, mut bg) = (resolve(cell.fgcolor(), FOREGROUND), resolve(cell.bgcolor(), BACKGROUND));
    if cell.inverse() {
        std::mem::swap(&mut fg, &mut bg);
    }
    if cell.dim() {
        fg = blend(fg, bg, DIM_BLEND);
    }
    Some(Cell {
        ch: cell.contents().to_owned(),
        style: Style { fg, bg, bold: cell.bold(), italic: cell.italic(), underline: cell.underline() },
        wide: cell.is_wide(),
    })
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

fn line_glyph(ch: char, x: f64, y: f64, color: &str) -> Option<String> {
    let (cx, cy) = (x + CELL_W / 2.0, y + CELL_H / 2.0);
    let (right, bottom) = (x + CELL_W, y + CELL_H);
    let radius = CELL_W / 2.0;
    let path = match ch {
        '─' => format!("M{x} {cy}H{right}"),
        '│' => format!("M{cx} {y}V{bottom}"),
        '╭' => format!("M{right} {cy}H{} Q{cx} {cy} {cx} {} V{bottom}", cx + radius, cy + radius),
        '╮' => format!("M{x} {cy}H{} Q{cx} {cy} {cx} {} V{bottom}", cx - radius, cy + radius),
        '╰' => format!("M{right} {cy}H{} Q{cx} {cy} {cx} {} V{y}", cx + radius, cy - radius),
        '╯' => format!("M{x} {cy}H{} Q{cx} {cy} {cx} {} V{y}", cx - radius, cy - radius),
        _ => return None,
    };
    Some(format!("<path d=\"{path}\" stroke=\"{color}\" fill=\"none\" stroke-width=\"1.2\"/>"))
}

pub fn render(screen: &vt100::Screen) -> String {
    let (rows, cols) = screen.size();
    let (rows, cols) = (rows.get(), cols.get());
    let width = cols as f64 * CELL_W + 2.0 * PAD;
    let height = rows as f64 * CELL_H + 2.0 * PAD;
    let mut out = String::new();
    let _ = write!(
        out,
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{width:.0}\" height=\"{height:.0}\" viewBox=\"0 0 {width:.0} {height:.0}\" font-family=\"{FONT_STACK}\" font-size=\"{FONT_SIZE}\">\n"
    );
    let _ = write!(out, "<rect width=\"100%\" height=\"100%\" rx=\"8\" fill=\"{}\"/>\n", hex(BACKGROUND));
    let mut backgrounds = String::new();
    let mut glyphs = String::new();
    let mut texts = String::new();
    for row in 0..rows {
        let y = PAD + row as f64 * CELL_H;
        let mut run: Option<(u16, Style, String, u16)> = None;
        let mut bg_run: Option<(u16, Rgb, u16)> = None;
        let flush_text = |texts: &mut String, run: &mut Option<(u16, Style, String, u16)>| {
            if let Some((start, style, content, span)) = run.take() {
                if content.trim().is_empty() && !style.underline {
                    return;
                }
                let x = PAD + start as f64 * CELL_W;
                let mut attrs = format!("fill=\"{}\"", hex(style.fg));
                if style.bold {
                    attrs.push_str(" font-weight=\"700\"");
                }
                if style.italic {
                    attrs.push_str(" font-style=\"italic\"");
                }
                if style.underline {
                    attrs.push_str(" text-decoration=\"underline\"");
                }
                let _ = writeln!(
                    texts,
                    "<text x=\"{x:.1}\" y=\"{:.1}\" textLength=\"{:.1}\" lengthAdjust=\"spacing\" xml:space=\"preserve\" {attrs}>{}</text>",
                    y + BASELINE,
                    span as f64 * CELL_W,
                    escape(&content)
                );
            }
        };
        let flush_bg = |backgrounds: &mut String, bg_run: &mut Option<(u16, Rgb, u16)>| {
            if let Some((start, color, span)) = bg_run.take() {
                if color != BACKGROUND {
                    let _ = writeln!(
                        backgrounds,
                        "<rect x=\"{:.1}\" y=\"{y:.1}\" width=\"{:.1}\" height=\"{CELL_H}\" fill=\"{}\"/>",
                        PAD + start as f64 * CELL_W,
                        span as f64 * CELL_W,
                        hex(color)
                    );
                }
            }
        };
        let mut col = 0;
        while col < cols {
            let Some(cell) = cell_at(screen, row, col) else {
                col += 1;
                continue;
            };
            let span = if cell.wide { 2 } else { 1 };
            match &mut bg_run {
                Some((_, color, length)) if *color == cell.style.bg => *length += span,
                _ => {
                    flush_bg(&mut backgrounds, &mut bg_run);
                    bg_run = Some((col, cell.style.bg, span));
                }
            }
            let x = PAD + col as f64 * CELL_W;
            let ch = cell.ch.chars().next().unwrap_or(' ');
            let fg = hex(cell.style.fg);
            if ch == '▌' {
                let _ = writeln!(glyphs, "<rect x=\"{x:.1}\" y=\"{y:.1}\" width=\"{:.1}\" height=\"{CELL_H}\" fill=\"{fg}\"/>", CELL_W / 2.0);
                flush_text(&mut texts, &mut run);
            } else if let Some(glyph) = line_glyph(ch, x, y, &fg) {
                let _ = writeln!(glyphs, "{glyph}");
                flush_text(&mut texts, &mut run);
            } else {
                let content = if cell.ch.is_empty() { " ".to_owned() } else { cell.ch.clone() };
                match &mut run {
                    Some((_, style, text, length)) if *style == cell.style => {
                        text.push_str(&content);
                        *length += span;
                    }
                    _ => {
                        flush_text(&mut texts, &mut run);
                        run = Some((col, cell.style.clone(), content, span));
                    }
                }
            }
            col += span;
        }
        flush_bg(&mut backgrounds, &mut bg_run);
        flush_text(&mut texts, &mut run);
    }
    out.push_str(&backgrounds);
    out.push_str(&glyphs);
    out.push_str(&texts);
    out.push_str("</svg>\n");
    out
}
