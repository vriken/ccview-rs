use ratatui::style::Color;

const HUES: [(u8, u8, u8); 14] = [
    (0xff, 0x77, 0xa2),
    (0x34, 0xc8, 0xfd),
    (0xa9, 0x92, 0xc7),
    (0xbc, 0x7e, 0x8c),
    (0x1a, 0x97, 0xf4),
    (0xc6, 0xaa, 0xdf),
    (0x5c, 0xa2, 0x8b),
    (0x6c, 0x96, 0xc2),
    (0xba, 0x6d, 0xd7),
    (0x07, 0xc7, 0xa4),
    (0x85, 0xab, 0xf0),
    (0xda, 0x60, 0xa4),
    (0x76, 0xc8, 0xc8),
    (0x80, 0x82, 0xf7),
];

pub const SIZE: usize = HUES.len();

pub fn color(index: usize) -> Color {
    let (r, g, b) = HUES[index % SIZE];
    Color::Rgb(r, g, b)
}

pub fn index_for_seed(seed: &str) -> usize {
    let hash = seed.bytes().fold(0xcbf2_9ce4_8422_2325_u64, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
    });
    (hash % SIZE as u64) as usize
}

#[cfg(test)]
mod tests {
    use super::*;

    fn luminance((r, g, b): (u8, u8, u8)) -> f64 {
        let channel = |value: u8| {
            let value = f64::from(value) / 255.0;
            if value <= 0.04045 { value / 12.92 } else { ((value + 0.055) / 1.055).powf(2.4) }
        };
        0.2126 * channel(r) + 0.7152 * channel(g) + 0.0722 * channel(b)
    }

    fn contrast(a: (u8, u8, u8), b: (u8, u8, u8)) -> f64 {
        let (high, low) = (luminance(a).max(luminance(b)), luminance(a).min(luminance(b)));
        (high + 0.05) / (low + 0.05)
    }

    #[test]
    fn chips_and_bars_are_readable_on_the_dark_theme() {
        const CHIP_TEXT: (u8, u8, u8) = (0, 0, 0);
        const SELECTION_BACKGROUND: (u8, u8, u8) = (0x30, 0x30, 0x30);
        for hue in HUES {
            assert!(contrast(hue, CHIP_TEXT) >= 4.5, "{hue:?}");
            assert!(contrast(hue, SELECTION_BACKGROUND) >= 3.0, "{hue:?}");
        }
    }

    #[test]
    fn hues_are_unique() {
        let mut sorted = HUES.to_vec();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), SIZE);
    }
}
