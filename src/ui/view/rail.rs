//! The rail: the channels, who is on the line, and what hardware is open.
//!
//! It sits on its own ground, one step lifted from the surface, and each section is
//! named by a filled chip. Between the ground and the chips there is nothing left for
//! a border to do.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::text::{Line as TextLine, Span};
use ratatui::widgets::{Block, Paragraph};

use super::{clip, spread};
use crate::proto::ChannelId;
use crate::ui::state::{App, ViewMode};
use crate::ui::theme::Theme;

/// The rail only names the audio hardware when there is room to spare for it.
const AUDIO_NEEDS: u16 = 14;
/// A chip row plus the blank line under the section.
const CHROME_ROWS: u16 = 2;
/// How much of a name the rail can hold before it has to cut it.
const NAME_ROOM: usize = 12;

pub fn draw(frame: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    frame.render_widget(Block::default().style(theme.panel()), area);
    if area.height < 2 || area.width < 8 {
        return;
    }

    let audio_rows = if area.height >= AUDIO_NEEDS && app.voice_available { 4 } else { 0 };
    let channel_rows = (app.channels.len() as u16).saturating_add(CHROME_ROWS);
    let [channels, people, audio] = Layout::vertical([
        Constraint::Max(channel_rows),
        Constraint::Min(2),
        Constraint::Length(audio_rows),
    ])
    .areas(area);

    section(frame, channels, theme, "CHANNELS", false, channel_rows_of(area.width, app, theme));

    let heading = format!("ON THE LINE{}{}", theme.glyphs.dot, app.peers.len());
    section(frame, people, theme, &heading, false, people_rows(area.width, app, theme));

    if audio.height > 0 {
        let focused = app.view_mode == ViewMode::Settings;
        let heading = format!("AUDIO{}F6", theme.glyphs.dot);
        section(frame, audio, theme, &heading, focused, audio_rows_of(area.width, app, theme));
    }
}

/// A named section: the chip, then its rows, then whatever space is left.
fn section(
    frame: &mut Frame,
    area: Rect,
    theme: &Theme,
    label: &str,
    focused: bool,
    rows: Vec<TextLine<'static>>,
) {
    if area.height == 0 {
        return;
    }
    let style = if focused { theme.chip_on() } else { theme.chip() };
    let mut lines = vec![TextLine::from(vec![
        Span::raw(" "),
        Span::styled(format!(" {label} "), style),
    ])];
    lines.extend(rows.into_iter().take(area.height.saturating_sub(1) as usize));
    frame.render_widget(Paragraph::new(lines), area);
}

/// The cursor marks what you are reading; the filled dot marks where your voice is.
/// They are different questions, so they get different columns.
fn channel_rows_of(width: u16, app: &App, theme: &Theme) -> Vec<TextLine<'static>> {
    app.channels
        .iter()
        .enumerate()
        .map(|(index, name)| {
            let id = ChannelId(index as u8);
            let reading = id == app.viewing && app.view_mode == ViewMode::Chat;
            let talking = app.voice == Some(id);
            let here = app.peers_in(id).len();

            let mark = |on: bool, glyph: char| {
                Span::styled(
                    if on { glyph.to_string() } else { " ".to_string() },
                    theme.accent(),
                )
            };

            let left = vec![
                Span::raw("  "),
                mark(reading, theme.glyphs.cursor),
                Span::raw(" "),
                mark(talking, theme.glyphs.on_air),
                Span::raw(" "),
                Span::styled(
                    clip(name, NAME_ROOM, theme),
                    if reading { theme.strong() } else { theme.text() },
                ),
            ];
            let right = match here {
                0 => vec![Span::raw(" ")],
                count => vec![Span::styled(format!("{count} "), theme.dim())],
            };
            spread(width, left, right)
        })
        .collect()
}

/// A meter, a name, and where that person is. Nothing else earns a column.
fn people_rows(width: u16, app: &App, theme: &Theme) -> Vec<TextLine<'static>> {
    app.peers
        .iter()
        .map(|peer| {
            let (meter, meter_style) = theme.meter(app.level_of(peer.id));
            let talking = app.speaking.contains(&peer.id);

            let mut left = vec![
                Span::raw("  "),
                Span::styled(meter, meter_style),
                Span::raw(" "),
                Span::styled(
                    clip(&peer.name, NAME_ROOM, theme),
                    if talking { theme.strong() } else { theme.text() },
                ),
            ];
            if peer.id == app.me {
                left.push(Span::styled(" you", theme.dim()));
            }

            // Being deafened is the bigger fact about someone than being muted, and
            // both matter more than which channel they are sitting in.
            let tag = if peer.deafened {
                "deafened".to_string()
            } else if peer.muted {
                "muted".to_string()
            } else {
                peer.channel
                    .map(|channel| app.channel_name(channel).to_string())
                    .unwrap_or_default()
            };
            let right = vec![Span::styled(clip(&tag, 9, theme), theme.dim()), Span::raw(" ")];
            spread(width, left, right)
        })
        .collect()
}

/// What you are actually speaking into and listening through. Worth a permanent
/// corner: it is the first thing anyone checks when a call goes wrong.
fn audio_rows_of(width: u16, app: &App, theme: &Theme) -> Vec<TextLine<'static>> {
    let room = (width as usize).saturating_sub(8);
    let row = |label: &'static str, name: &Option<String>| {
        let text = name.clone().unwrap_or_else(|| "system default".to_string());
        TextLine::from(vec![
            Span::raw("  "),
            Span::styled(label, theme.dim()),
            Span::raw("  "),
            Span::styled(clip(&text, room, theme), theme.text()),
        ])
    };
    vec![
        row("mic", &app.active_input_name),
        row("out", &app.active_output_name),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proto::{PeerId, PeerInfo};

    fn text(line: &TextLine<'_>) -> String {
        line.spans.iter().map(|span| span.content.as_ref()).collect()
    }

    fn room() -> App {
        let mut app = App::new(PeerId([1; 32]), "code".into());
        app.channels = vec!["general".into(), "gaming".into()];
        app.peers = vec![
            PeerInfo {
                id: PeerId([1; 32]),
                name: "alice".into(),
                channel: Some(ChannelId(0)),
                muted: false,
                deafened: false,
            },
            PeerInfo {
                id: PeerId([2; 32]),
                name: "bob".into(),
                channel: None,
                muted: true,
                deafened: false,
            },
        ];
        app
    }

    #[test]
    fn reading_a_channel_and_talking_in_it_are_marked_separately() {
        let mut app = room();
        app.viewing = ChannelId(0);
        app.voice = Some(ChannelId(1));
        let theme = Theme::from_env();
        let rows = channel_rows_of(28, &app, &theme);

        let general = text(&rows[0]);
        let gaming = text(&rows[1]);
        assert!(general.contains(theme.glyphs.cursor), "the read channel keeps the cursor: {general}");
        assert!(!general.contains(theme.glyphs.on_air), "we are not talking there: {general}");
        assert!(gaming.contains(theme.glyphs.on_air), "the voice channel is marked: {gaming}");
    }

    #[test]
    fn the_roster_says_where_someone_is_or_why_they_are_quiet() {
        let app = room();
        let theme = Theme::from_env();
        let rows = people_rows(28, &app, &theme);

        assert!(text(&rows[0]).contains("general"), "{}", text(&rows[0]));
        assert!(text(&rows[0]).contains("you"), "we are marked in our own roster");
        assert!(text(&rows[1]).contains("muted"), "{}", text(&rows[1]));
    }

    #[test]
    fn a_deafened_peer_reports_that_over_being_muted() {
        let mut app = room();
        app.peers[1].deafened = true;
        let rows = people_rows(28, &app, &Theme::from_env());
        assert!(text(&rows[1]).contains("deafened"), "{}", text(&rows[1]));
    }

    #[test]
    fn every_row_fits_the_rail() {
        let mut app = room();
        app.peers[0].name = "a-really-long-nickname".into();
        app.channels = vec!["a-really-long-channel-name".into()];
        let theme = Theme::from_env();

        for row in channel_rows_of(28, &app, &theme) {
            assert!(row.width() <= 28, "channel row overflowed: {}", text(&row));
        }
        for row in people_rows(28, &app, &theme) {
            assert!(row.width() <= 28, "roster row overflowed: {}", text(&row));
        }
    }

    #[test]
    fn unknown_hardware_still_says_something_true() {
        let app = room();
        let rows = audio_rows_of(28, &app, &Theme::from_env());
        assert!(text(&rows[0]).contains("system default"), "{}", text(&rows[0]));
    }
}
