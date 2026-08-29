use ratatui::style::{Color, Modifier, Style};

/// catppuccin 四套配色（https://catppuccin.com/）。
/// 语义角色映射：bg/面板/边框/正文/次文/强调色/警示色。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flavor {
    Latte,
    Frappe,
    Macchiato,
    Mocha,
}

impl Flavor {
    pub const ALL: [Flavor; 4] = [
        Flavor::Latte,
        Flavor::Frappe,
        Flavor::Macchiato,
        Flavor::Mocha,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Flavor::Latte => "latte",
            Flavor::Frappe => "frappe",
            Flavor::Macchiato => "macchiato",
            Flavor::Mocha => "mocha",
        }
    }

    pub fn from_name(s: &str) -> Option<Flavor> {
        Flavor::ALL.iter().copied().find(|f| f.name() == s)
    }

    pub fn next(self) -> Flavor {
        match self {
            Flavor::Latte => Flavor::Frappe,
            Flavor::Frappe => Flavor::Macchiato,
            Flavor::Macchiato => Flavor::Mocha,
            Flavor::Mocha => Flavor::Latte,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Palette {
    pub bg: Color,
    pub mantle: Color,
    pub text: Color,
    pub subtext: Color,
    pub overlay: Color,
    pub surface: Color,
    pub border: Color,
    pub blue: Color,
    pub teal: Color,
    pub green: Color,
    pub yellow: Color,
    pub peach: Color,
    pub red: Color,
    pub mauve: Color,
    pub pink: Color,
}

impl Palette {
    pub fn of(f: Flavor) -> Palette {
        let rgb = |(r, g, b): (u8, u8, u8)| Color::Rgb(r, g, b);
        match f {
            Flavor::Latte => Palette {
                bg: rgb((0xEF, 0xF1, 0xF5)),
                mantle: rgb((0xE6, 0xE9, 0xEF)),
                text: rgb((0x4C, 0x4F, 0x69)),
                subtext: rgb((0x6C, 0x6F, 0x85)),
                overlay: rgb((0x8C, 0x8F, 0xA3)),
                surface: rgb((0xCC, 0xD0, 0xDA)),
                border: rgb((0x9C, 0xA0, 0xB0)),
                blue: rgb((0x1E, 0x66, 0xF5)),
                teal: rgb((0x17, 0x92, 0x99)),
                green: rgb((0x40, 0xA0, 0x2B)),
                yellow: rgb((0xDF, 0x8E, 0x1D)),
                peach: rgb((0xFE, 0x64, 0x0B)),
                red: rgb((0xD2, 0x0F, 0x39)),
                mauve: rgb((0x88, 0x39, 0xEF)),
                pink: rgb((0xEA, 0x76, 0xCB)),
            },
            Flavor::Frappe => Palette {
                bg: rgb((0x30, 0x34, 0x46)),
                mantle: rgb((0x29, 0x2C, 0x3C)),
                text: rgb((0xC6, 0xD0, 0xF5)),
                subtext: rgb((0xA5, 0xAD, 0xCE)),
                overlay: rgb((0x83, 0x8B, 0xA7)),
                surface: rgb((0x51, 0x57, 0x6D)),
                border: rgb((0x62, 0x68, 0x80)),
                blue: rgb((0x8C, 0xAA, 0xEE)),
                teal: rgb((0x81, 0xC8, 0xBE)),
                green: rgb((0xA6, 0xD1, 0x89)),
                yellow: rgb((0xE5, 0xC8, 0x90)),
                peach: rgb((0xEF, 0x9F, 0x76)),
                red: rgb((0xE7, 0x82, 0x84)),
                mauve: rgb((0xCA, 0x9E, 0xE6)),
                pink: rgb((0xF4, 0xB8, 0xE4)),
            },
            Flavor::Macchiato => Palette {
                bg: rgb((0x24, 0x27, 0x3A)),
                mantle: rgb((0x1E, 0x20, 0x30)),
                text: rgb((0xCA, 0xD3, 0xF5)),
                subtext: rgb((0xA5, 0xAD, 0xCB)),
                overlay: rgb((0x80, 0x87, 0xA2)),
                surface: rgb((0x49, 0x4D, 0x64)),
                border: rgb((0x5B, 0x60, 0x78)),
                blue: rgb((0x8A, 0xAD, 0xF4)),
                teal: rgb((0x8B, 0xD5, 0xCA)),
                green: rgb((0xA6, 0xDA, 0x95)),
                yellow: rgb((0xEE, 0xD4, 0x9F)),
                peach: rgb((0xF5, 0xA9, 0x7F)),
                red: rgb((0xED, 0x87, 0x96)),
                mauve: rgb((0xC6, 0xA0, 0xF6)),
                pink: rgb((0xF5, 0xBD, 0xE6)),
            },
            Flavor::Mocha => Palette {
                bg: rgb((0x1E, 0x1E, 0x2E)),
                mantle: rgb((0x18, 0x18, 0x25)),
                text: rgb((0xCD, 0xD6, 0xF4)),
                subtext: rgb((0xA6, 0xAD, 0xC8)),
                overlay: rgb((0x7F, 0x84, 0x9C)),
                surface: rgb((0x45, 0x47, 0x5A)),
                border: rgb((0x58, 0x5B, 0x70)),
                blue: rgb((0x89, 0xB4, 0xFA)),
                teal: rgb((0x94, 0xE2, 0xD5)),
                green: rgb((0xA6, 0xE3, 0xA1)),
                yellow: rgb((0xF9, 0xE2, 0xAF)),
                peach: rgb((0xFA, 0xB3, 0x87)),
                red: rgb((0xF3, 0x8B, 0xA8)),
                mauve: rgb((0xCB, 0xA6, 0xF7)),
                pink: rgb((0xF5, 0xC2, 0xE7)),
            },
        }
    }

    pub fn bg_style(self) -> Style {
        Style::new().bg(self.bg)
    }

    pub fn text_style(self) -> Style {
        Style::new().fg(self.text).bg(self.bg)
    }

    pub fn sub_style(self) -> Style {
        Style::new().fg(self.subtext).bg(self.bg)
    }

    pub fn title_style(self) -> Style {
        Style::new()
            .fg(self.mauve)
            .bg(self.bg)
            .add_modifier(Modifier::BOLD)
    }

    pub fn border_style(self) -> Style {
        Style::new().fg(self.border).bg(self.bg)
    }

    pub fn block(
        self,
        title: impl Into<ratatui::text::Line<'static>>,
    ) -> ratatui::widgets::Block<'static> {
        ratatui::widgets::Block::bordered()
            .title(title)
            .border_style(self.border_style())
            .title_style(self.title_style())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flavors_are_distinct_and_parseable() {
        let bgs: Vec<Color> = Flavor::ALL.iter().map(|f| Palette::of(*f).bg).collect();
        for i in 0..bgs.len() {
            for j in (i + 1)..bgs.len() {
                assert_ne!(bgs[i], bgs[j], "主题背景色不应重复");
            }
            assert_eq!(
                Flavor::from_name(Flavor::ALL[i].name()),
                Some(Flavor::ALL[i])
            );
        }
    }
}
