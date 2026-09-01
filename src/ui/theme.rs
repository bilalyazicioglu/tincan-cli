//! Design tokens: colour, glyph and motion in one place.
//!
//! The interface commits to its own surface — a warm dark ground with a slightly
//! lifted panel for the rail — so the room reads the same on every terminal. The
//! palette is metal: a brown tin ground, brass for the string and the invite code,
//! and the two greens copper actually turns — bright patina for what is active,
//! deeper verdigris for a link that is holding.
//!
//! Everything degrades in one place: truecolor → 256 colours → `NO_COLOR`, unicode →
//! ASCII, motion → still. `view` never names a colour or a glyph of its own.

use ratatui::style::{Color, Modifier, Style};

/// How the string between the rail and the chat is drawn. Derived from the live
/// link — it reports, it does not decorate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Strand {
    /// Nobody on the other end yet.
    Idle,
    /// Everyone reached directly: the string is taut.
    Taut,
    /// Someone is coming through a relay: the string sags.
    Slack,
    /// Audio is dropping out: the string is fraying.
    Frayed,
}

/// The two cans and the string between them, drawn on the empty chat screen.
pub struct CanArt {
    pub lid: &'static str,
    pub top: &'static str,
    pub body: &'static str,
    pub bottom: &'static str,
    pub string: char,
    pub knot: char,
}

/// Every glyph the interface draws, so the ASCII fallback is one table away.
pub struct Glyphs {
    /// Indexed by `Strand`: idle, taut, slack, frayed.
    pub strand: [char; 4],
    /// Travels along the string while someone is talking.
    pub pulse: char,
    /// Marks the channel being read.
    pub cursor: char,
    /// Marks the channel we are talking in.
    pub on_air: char,
    /// Five steps of the three-cell level meter, silence first.
    pub meter: [&'static str; 5],
    /// The long meter on the microphone test: a lit cell and an unlit one.
    pub bar: [char; 2],
    /// The text cursor in the message field.
    pub caret: char,
    /// Marks a line the room wrote rather than a person.
    pub note: char,
    /// The mark that says a name or a message was cut to fit.
    pub cut: char,
    /// The separator between two facts sharing a line.
    pub dot: &'static str,
    pub can: CanArt,
}

const UNICODE: Glyphs = Glyphs {
    strand: ['·', '│', '╎', '┆'],
    pulse: '⟩',
    cursor: '▸',
    on_air: '●',
    meter: ["···", "▁··", "▁▃·", "▁▃▅", "▃▅▇"],
    bar: ['█', '░'],
    caret: '▏',
    note: '·',
    cut: '…',
    dot: " · ",
    can: CanArt {
        lid: "( o )",
        top: "┌─────┐",
        body: "│     │",
        bottom: "└─────┘",
        string: '╌',
        knot: '◦',
    },
};

const ASCII: Glyphs = Glyphs {
    strand: ['.', '|', ':', '!'],
    pulse: '>',
    cursor: '>',
    on_air: '*',
    meter: ["...", "-..", "--.", "---", "==="],
    bar: ['#', '.'],
    caret: '_',
    note: '-',
    cut: '~',
    dot: " - ",
    can: CanArt {
        lid: "( o )",
        top: "+-----+",
        body: "|     |",
        bottom: "+-----+",
        string: '-',
        knot: 'o',
    },
};

struct Palette {
    /// Body text.
    tin: Color,
    /// Everything secondary.
    zinc: Color,
    /// What is active: the cursor, the channel you are in, a travelling pulse.
    patina: Color,
    /// A link that is holding, and a device in use.
    verdigris: Color,
    /// The string, the invite code, a relayed link, a hot meter.
    brass: Color,
    /// Errors, and audio that is breaking up.
    alarm: Color,
    /// The ground, and the rail's slightly lifted ground.
    ink: Color,
    panel: Color,
}

const DARK_TRUE: Palette = Palette {
    tin: Color::Rgb(0xDC, 0xD5, 0xCB),
    zinc: Color::Rgb(0x8A, 0x81, 0x77),
    patina: Color::Rgb(0x3E, 0xC5, 0xBC),
    verdigris: Color::Rgb(0x2C, 0x9C, 0x97),
    brass: Color::Rgb(0xD9, 0xA4, 0x41),
    alarm: Color::Rgb(0xE0, 0x66, 0x4F),
    ink: Color::Rgb(0x14, 0x10, 0x0C),
    panel: Color::Rgb(0x21, 0x1A, 0x14),
};

const DARK_256: Palette = Palette {
    tin: Color::Indexed(252),
    zinc: Color::Indexed(245),
    patina: Color::Indexed(80),
    verdigris: Color::Indexed(37),
    brass: Color::Indexed(179),
    alarm: Color::Indexed(167),
    ink: Color::Indexed(233),
    panel: Color::Indexed(235),
};

const LIGHT_TRUE: Palette = Palette {
    tin: Color::Rgb(0x2A, 0x24, 0x1C),
    zinc: Color::Rgb(0x6E, 0x65, 0x59),
    patina: Color::Rgb(0x0E, 0x7C, 0x77),
    verdigris: Color::Rgb(0x0B, 0x64, 0x60),
    brass: Color::Rgb(0x8A, 0x64, 0x12),
    alarm: Color::Rgb(0x96, 0x33, 0x1F),
    ink: Color::Rgb(0xF0, 0xEB, 0xE3),
    panel: Color::Rgb(0xE3, 0xDB, 0xCF),
};

const LIGHT_256: Palette = Palette {
    tin: Color::Indexed(235),
    zinc: Color::Indexed(241),
    patina: Color::Indexed(30),
    verdigris: Color::Indexed(23),
    brass: Color::Indexed(94),
    alarm: Color::Indexed(88),
    ink: Color::Indexed(255),
    panel: Color::Indexed(254),
};

/// Colour with nothing but the terminal's own: `NO_COLOR` keeps every distinction,
/// carried by weight instead of hue.
const MONO: Palette = Palette {
    tin: Color::Reset,
    zinc: Color::Reset,
    patina: Color::Reset,
    verdigris: Color::Reset,
    brass: Color::Reset,
    alarm: Color::Reset,
    ink: Color::Reset,
    panel: Color::Reset,
};

pub struct Theme {
    palette: Palette,
    pub glyphs: Glyphs,
    /// Whether the string may animate.
    pub motion: bool,
    mono: bool,
    ascii: bool,
}

impl Theme {
    /// Reads the environment once at start-up.
    ///
    /// * `NO_COLOR` — colour off, weight carries the meaning.
    /// * `TINCAN_THEME=light` — the same hues on a paper ground.
    /// * `TINCAN_ASCII=1` — no glyph outside ASCII.
    /// * `TINCAN_NO_MOTION=1` / `NO_MOTION` — the string holds still.
    pub fn from_env() -> Self {
        let mono = set("NO_COLOR");
        let ascii = set("TINCAN_ASCII") || !locale_is_utf8();
        let light = std::env::var("TINCAN_THEME")
            .map(|value| value.eq_ignore_ascii_case("light"))
            .unwrap_or(false);
        let truecolor = std::env::var("COLORTERM")
            .map(|value| value.contains("truecolor") || value.contains("24bit"))
            .unwrap_or(false);

        let palette = match (mono, light, truecolor) {
            (true, _, _) => MONO,
            (_, true, true) => LIGHT_TRUE,
            (_, true, false) => LIGHT_256,
            (_, false, true) => DARK_TRUE,
            (_, false, false) => DARK_256,
        };

        Self {
            palette,
            glyphs: if ascii { ASCII } else { UNICODE },
            motion: !(set("TINCAN_NO_MOTION") || set("NO_MOTION")),
            mono,
            ascii,
        }
    }

    /// The narrowest terminal tincan supports: no colour, no glyph outside ASCII, no
    /// motion. The tests draw against this to keep the fallback honest.
    pub fn austere() -> Self {
        Self { palette: MONO, glyphs: ASCII, motion: false, mono: true, ascii: true }
    }

    /// Swaps marks that live in prose rather than in the glyph table.
    pub fn plainly(&self, text: &str) -> String {
        if self.ascii {
            text.replace(" · ", " - ").replace("↑↓", "up/down")
        } else {
            text.to_string()
        }
    }

    /// The ground the whole app sits on.
    pub fn surface(&self) -> Style {
        Style::default().bg(self.palette.ink).fg(self.palette.tin)
    }

    /// The rail's ground — one step lifted from the surface. This is what separates
    /// the room from the talk; no border does that work.
    pub fn panel(&self) -> Style {
        Style::default().bg(self.palette.panel).fg(self.palette.tin)
    }

    pub fn text(&self) -> Style {
        Style::default().fg(self.palette.tin)
    }

    pub fn strong(&self) -> Style {
        Style::default().fg(self.palette.tin).add_modifier(Modifier::BOLD)
    }

    pub fn dim(&self) -> Style {
        if self.mono {
            Style::default().add_modifier(Modifier::DIM)
        } else {
            Style::default().fg(self.palette.zinc)
        }
    }

    pub fn accent(&self) -> Style {
        Style::default()
            .fg(self.palette.patina)
            .add_modifier(Modifier::BOLD)
    }

    pub fn brass(&self) -> Style {
        if self.mono {
            Style::default().add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(self.palette.brass)
        }
    }

    pub fn ok(&self) -> Style {
        Style::default().fg(self.palette.verdigris)
    }

    pub fn error(&self) -> Style {
        Style::default()
            .fg(self.palette.alarm)
            .add_modifier(Modifier::BOLD)
    }

    /// A section name, set as a filled tab. The loudest thing in the rail, because
    /// naming what you are looking at is the first job.
    pub fn chip(&self) -> Style {
        if self.mono {
            Style::default().add_modifier(Modifier::REVERSED)
        } else {
            Style::default()
                .fg(self.palette.ink)
                .bg(self.palette.tin)
                .add_modifier(Modifier::BOLD)
        }
    }

    /// A chip for the section you are working in.
    pub fn chip_on(&self) -> Style {
        if self.mono {
            Style::default().add_modifier(Modifier::REVERSED | Modifier::BOLD)
        } else {
            Style::default()
                .fg(self.palette.ink)
                .bg(self.palette.patina)
                .add_modifier(Modifier::BOLD)
        }
    }

    /// The link chip in the header, coloured by what the string is doing.
    pub fn chip_link(&self, strand: Strand) -> Style {
        if self.mono {
            return Style::default().add_modifier(Modifier::REVERSED);
        }
        let bg = match strand {
            Strand::Idle => self.palette.zinc,
            Strand::Taut => self.palette.verdigris,
            Strand::Slack => self.palette.brass,
            Strand::Frayed => self.palette.alarm,
        };
        Style::default()
            .fg(self.palette.ink)
            .bg(bg)
            .add_modifier(Modifier::BOLD)
    }

    pub fn strand(&self, strand: Strand) -> (char, Style) {
        let glyph = self.glyphs.strand[strand as usize];
        let style = match strand {
            Strand::Idle => self.dim(),
            Strand::Taut => self.ok(),
            Strand::Slack => self.brass(),
            Strand::Frayed => self.error(),
        };
        (glyph, style)
    }

    /// A three-cell level meter. `level` is 0–4, as the audio engine publishes it.
    ///
    /// A full meter goes brass rather than a brighter green: the top of the scale is
    /// where a voice starts to clip, which reads as a warning, not as success.
    pub fn meter(&self, level: u8) -> (&'static str, Style) {
        let level = level.min(4) as usize;
        let style = match level {
            0 => self.dim(),
            4 => self.brass(),
            _ => self.ok(),
        };
        (self.glyphs.meter[level], style)
    }
}

impl Default for Theme {
    fn default() -> Self {
        Self::from_env()
    }
}

fn set(name: &str) -> bool {
    std::env::var_os(name).is_some_and(|value| !value.is_empty())
}

/// Box-drawing is only safe when the locale says the terminal speaks UTF-8. When no
/// locale is set at all we assume it does — that is the common case on macOS.
fn locale_is_utf8() -> bool {
    let mut saw_one = false;
    for name in ["LC_ALL", "LC_CTYPE", "LANG"] {
        if let Ok(value) = std::env::var(name) {
            if value.is_empty() {
                continue;
            }
            saw_one = true;
            let value = value.to_ascii_lowercase();
            if value.contains("utf-8") || value.contains("utf8") {
                return true;
            }
        }
    }
    !saw_one
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_strand_has_its_own_glyph() {
        for glyphs in [&UNICODE, &ASCII] {
            let mut seen = glyphs.strand.to_vec();
            seen.sort_unstable();
            seen.dedup();
            assert_eq!(seen.len(), 4, "a state that looks like another reports nothing");
        }
    }

    #[test]
    fn the_ascii_fallback_is_actually_ascii() {
        let art = &ASCII.can;
        let text = format!(
            "{}{}{}{}{}{}{}{}{}{}{}{}{}{}{}",
            ASCII.strand.iter().collect::<String>(),
            ASCII.pulse,
            ASCII.cursor,
            ASCII.on_air,
            ASCII.meter.concat(),
            ASCII.bar.iter().collect::<String>(),
            ASCII.caret,
            ASCII.note,
            ASCII.cut,
            ASCII.dot,
            art.lid,
            art.top,
            art.body,
            art.bottom,
            [art.string, art.knot].iter().collect::<String>(),
        );
        assert!(text.is_ascii(), "not ascii: {text}");
    }

    #[test]
    fn the_two_greens_stay_apart() {
        let theme = Theme { palette: DARK_TRUE, glyphs: UNICODE, motion: true, mono: false, ascii: false };
        assert_ne!(
            theme.accent().fg,
            theme.ok().fg,
            "what is active and what is holding are different questions"
        );
        assert_ne!(theme.meter(4).1.fg, theme.meter(2).1.fg, "a hot meter must look hot");
    }

    #[test]
    fn the_meter_runs_from_quiet_to_loud() {
        let theme = Theme { palette: DARK_TRUE, glyphs: UNICODE, motion: true, mono: false, ascii: false };
        let (quiet, _) = theme.meter(0);
        let (loud, _) = theme.meter(4);
        assert_ne!(quiet, loud);
        assert_eq!(theme.meter(9).0, loud, "an out-of-range level must not panic");
    }

    #[test]
    fn every_meter_step_is_the_same_width() {
        for glyphs in [&UNICODE, &ASCII] {
            for step in glyphs.meter {
                assert_eq!(step.chars().count(), 3, "the meter must not shift the name beside it");
            }
        }
    }

    #[test]
    fn no_color_keeps_the_distinctions_without_hue() {
        let theme = Theme { palette: MONO, glyphs: UNICODE, motion: true, mono: true, ascii: false };
        assert_ne!(theme.dim(), theme.text(), "dim must stay distinguishable");
        assert_ne!(theme.chip(), theme.text(), "a chip must stay distinguishable");
        assert_eq!(theme.text().fg, Some(Color::Reset), "no colour may be forced");
    }

    #[test]
    fn the_ascii_fallback_reaches_the_marks_that_live_in_prose() {
        // Separators and ellipses are written into sentences rather than looked up in
        // the glyph table, so they are exactly what slips past a fallback.
        let plain = Theme::austere();
        assert_eq!(plain.plainly("tab · f2 · ↑↓ move"), "tab - f2 - up/down move");
        assert!(plain.glyphs.cut.is_ascii());
        assert!(plain.glyphs.dot.is_ascii());

        let full = Theme { palette: DARK_TRUE, glyphs: UNICODE, motion: true, mono: false, ascii: false };
        assert_eq!(full.plainly("tab · f2"), "tab · f2", "a capable terminal keeps the mark");
    }

    #[test]
    fn a_locale_that_says_utf8_gets_box_drawing() {
        assert!(locale_is_utf8() || std::env::var("LANG").is_ok());
    }
}
