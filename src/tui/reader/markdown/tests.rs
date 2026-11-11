use super::renderer::render;
use super::theme::Theme;
use super::wrap::spans_width;
use ratatui::text::Line;

fn line_width(line: &Line<'_>) -> usize {
    spans_width(&line.spans)
}

#[test]
fn no_line_exceeds_width_on_hostile_input() {
    let hostile = format!(
        "# {}\n\n> > > {}\n\n- a\n  - b\n    - {}\n\n| a | b |\n|---|---|\n| {} | x |\n\n```rust\n{}\n",
        "w".repeat(500),
        "word ".repeat(100),
        "漢字".repeat(100),
        "z".repeat(300),
        "long ".repeat(200),
    );
    let theme = Theme::default();
    for width in [1u16, 2, 5, 20, 80] {
        for line in render(&hostile, width, &theme, &mut Default::default()) {
            assert!(line_width(&line) <= width as usize, "width {width}");
        }
    }
}

#[test]
fn degenerate_inputs_do_not_panic() {
    let theme = Theme::default();
    assert!(render("", 80, &theme, &mut Default::default()).is_empty());
    assert!(render("text", 0, &theme, &mut Default::default()).is_empty());
    render("```", 1, &theme, &mut Default::default());
    render("|", 3, &theme, &mut Default::default());
}
