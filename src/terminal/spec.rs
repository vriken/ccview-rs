use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct CommandSpec {
    pub program: String,
    pub args: Vec<String>,
    pub cwd: Option<PathBuf>,
    pub env: Vec<(String, String)>,
    #[serde(default)]
    pub env_remove: Vec<String>,
}

impl CommandSpec {
    pub fn new(program: impl Into<String>) -> Self {
        Self {
            program: program.into(),
            ..Self::default()
        }
    }

    pub fn args<I, S>(mut self, args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.args.extend(args.into_iter().map(Into::into));
        self
    }

    pub fn cwd(mut self, cwd: impl Into<PathBuf>) -> Self {
        self.cwd = Some(cwd.into());
        self
    }

    pub fn env_remove(mut self, keys: impl IntoIterator<Item = String>) -> Self {
        self.env_remove.extend(keys);
        self
    }

    pub fn env(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.env.push((key.into(), value.into()));
        self
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rgb(pub u8, pub u8, pub u8);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct TerminalOptions {
    pub foreground: Rgb,
    pub background: Rgb,
    pub ansi_palette: Vec<Rgb>,
    pub cell_pixels: (u16, u16),
}

impl Default for TerminalOptions {
    fn default() -> Self {
        Self {
            foreground: Rgb(0xdd, 0xdd, 0xdd),
            background: Rgb(0x1e, 0x1e, 0x1e),
            ansi_palette: XTERM_ANSI.to_vec(),
            cell_pixels: (8, 16),
        }
    }
}

impl TerminalOptions {
    pub fn palette_color(&self, index: u8) -> Rgb {
        match index {
            0..=15 => self
                .ansi_palette
                .get(usize::from(index))
                .copied()
                .unwrap_or(XTERM_ANSI[usize::from(index)]),
            16..=231 => {
                let cube = |component: u8| if component == 0 { 0 } else { 55 + 40 * component };
                let offset = index - 16;
                Rgb(cube(offset / 36), cube(offset / 6 % 6), cube(offset % 6))
            }
            232..=255 => {
                let level = 8 + 10 * (index - 232);
                Rgb(level, level, level)
            }
        }
    }
}

const XTERM_ANSI: [Rgb; 16] = [
    Rgb(0x00, 0x00, 0x00),
    Rgb(0xcd, 0x00, 0x00),
    Rgb(0x00, 0xcd, 0x00),
    Rgb(0xcd, 0xcd, 0x00),
    Rgb(0x00, 0x00, 0xee),
    Rgb(0xcd, 0x00, 0xcd),
    Rgb(0x00, 0xcd, 0xcd),
    Rgb(0xe5, 0xe5, 0xe5),
    Rgb(0x7f, 0x7f, 0x7f),
    Rgb(0xff, 0x00, 0x00),
    Rgb(0x00, 0xff, 0x00),
    Rgb(0xff, 0xff, 0x00),
    Rgb(0x5c, 0x5c, 0xff),
    Rgb(0xff, 0x00, 0xff),
    Rgb(0x00, 0xff, 0xff),
    Rgb(0xff, 0xff, 0xff),
];
