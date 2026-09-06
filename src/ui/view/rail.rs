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
/// How much of the right-hand tag the rail can hold. Exactly wide enough for the two
/// longest things it ever says — "deafened" and "silenced" — and no wider, because the
/// column the cursor needs had to come from somewhere.
const TAG_ROOM: usize = 8;

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

    // Lit while the cursor is on somebody, the same way the settings screen lights the
    // section its keys are pointed at.
    let holding = app.selected_peer.is_some();
    // And until you have used it, the chip names its own key, the way AUDIO carries F6.
    // Once you are in the list the footer is saying more than this could, so the hint
    // gets out of the way rather than repeating itself.
    let heading = match holding {
        true => format!("ON THE LINE{}{}", theme.glyphs.dot, app.peers.len()),
        false => format!(
            "ON THE LINE{}{}{}{}",
            theme.glyphs.dot,
            app.peers.len(),
            theme.glyphs.dot,
            theme.plainly("↑↓")
        ),
    };
    section(frame, people, theme, &heading, holding, people_rows(area.width, app, theme));

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
            let unread = app.unread.contains(&id);
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
                // Brass is the palette's "look at this" colour, and it is deliberately
                // not the turquoise that marks where you already are: being here and
                // having something waiting are different questions.
                Span::styled(
                    clip(name, NAME_ROOM, theme),
                    match (reading, unread) {
                        (true, _) => theme.strong(),
                        (false, true) => theme.brass(),
                        (false, false) => theme.text(),
                    },
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
            let (level, at_full) = theme.meter(app.level_of(peer.id));
            let talking = app.speaking.contains(&peer.id);
            let gain = app.gain_of(peer.id);
            // Silencing someone takes their meter away and puts a mark in its place —
            // the column beside the name is the one the eye is already on, and a word
            // at the far edge was too easy to miss. Nothing is lost by it: the name
            // still goes bold while they speak, so you can see you are cutting off
            // somebody who is talking.
            //
            // Short of that, a voice you have merely turned down keeps its meter and
            // its movement but is drawn in the grey a silent one uses, so it reads as
            // not reaching you whole. That grey is what carries the setting once the
            // number has gone away.
            let (meter, meter_style) = match gain {
                0.0 => (theme.glyphs.silenced, theme.dim()),
                g if g < 1.0 => (level, theme.dim()),
                _ => (level, at_full),
            };

            let selected = app.selected_peer == Some(peer.id);
            let mut left = vec![
                Span::raw("  "),
                // The same cursor the channel list uses, in the same column, because
                // it answers the same question: this is the row your keys act on.
                Span::styled(
                    if selected { theme.glyphs.cursor.to_string() } else { " ".to_string() },
                    theme.accent(),
                ),
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
            // Being deafened is the bigger fact about someone than being muted, and
            // both matter more than which channel they are sitting in.
            //
            // Silencing is binary and consequential, so it says so whatever the cursor
            // is doing. A volume merely moved reads as an exact number only while you
            // are on that row moving it — the rest of the time the meter carries it
            // and the column goes back to answering where the person is, which is what
            // it is for.
            let tag = if peer.deafened {
                "deafened".to_string()
            } else if peer.muted {
                "muted".to_string()
            } else if gain == 0.0 {
                "silenced".to_string()
            } else if selected && gain != 1.0 {
                format!("{}%", (gain * 100.0).round())
            } else {
                peer.channel
                    .map(|channel| app.channel_name(channel).to_string())
                    .unwrap_or_default()
            };
            let right = vec![Span::styled(clip(&tag, TAG_ROOM, theme), theme.dim()), Span::raw(" ")];
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
    fn a_channel_with_something_waiting_is_set_apart_from_the_one_you_are_in() {
        let mut app = room();
        app.unread.insert(ChannelId(1));
        let theme = Theme::from_env();
        let rows = channel_rows_of(28, &app, &theme);

        let waiting = rows[1].spans.iter().find(|span| span.content.contains("gaming")).unwrap();
        let quiet = rows[0].spans.iter().find(|span| span.content.contains("general")).unwrap();
        assert_ne!(waiting.style, quiet.style, "an unread channel has to look different");
        assert_ne!(
            waiting.style, theme.accent(),
            "and different from the colour that means you are already there"
        );
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

    /// Someone who is talking normally — the only kind of row a volume can show on,
    /// since being muted or deafened is the louder fact and takes the column.
    fn plain_peer() -> PeerInfo {
        PeerInfo {
            id: PeerId([3; 32]),
            name: "cem".into(),
            channel: Some(ChannelId(0)),
            muted: false,
            deafened: false,
        }
    }

    #[test]
    fn both_lists_put_their_cursor_in_the_same_column() {
        let mut app = room();
        app.selected_peer = Some(PeerId([2; 32]));
        let theme = Theme::from_env();

        let channel = text(&channel_rows_of(28, &app, &theme)[0]);
        let person = text(&people_rows(28, &app, &theme)[1]);
        let at = |row: &str| row.chars().position(|c| c == theme.glyphs.cursor);

        assert_eq!(
            at(&channel),
            at(&person),
            "the cursor says \"your keys act here\"; it has to be in one place\n{channel}\n{person}"
        );
    }

    #[test]
    fn the_row_you_are_on_is_marked_and_says_where_its_volume_sits() {
        let mut app = room();
        app.peers.push(plain_peer());
        app.selected_peer = Some(PeerId([3; 32]));
        app.peer_gains.insert(PeerId([3; 32]), 0.5);
        let theme = Theme::from_env();
        let rows = people_rows(28, &app, &theme);

        let selected = text(&rows[2]);
        assert!(
            selected.contains(theme.glyphs.cursor),
            "the selected row must carry the same cursor the channel list uses: {selected}"
        );
        assert!(selected.contains("50%"), "and say where the volume now sits: {selected}");

        let other = text(&rows[0]);
        assert!(
            !other.contains(theme.glyphs.cursor),
            "only one row at a time: {other}"
        );
    }

    #[test]
    fn the_exact_volume_replaces_the_channel_only_while_you_are_on_that_row() {
        let mut app = room();
        app.peers.push(plain_peer());
        app.peer_gains.insert(PeerId([3; 32]), 0.5);
        let theme = Theme::from_env();

        let resting = text(&people_rows(28, &app, &theme)[2]);
        assert!(
            resting.contains("general"),
            "with the cursor elsewhere the row goes back to saying where they are: {resting}"
        );
        assert!(
            !resting.contains("50%"),
            "a number nobody is adjusting is noise: {resting}"
        );

        app.selected_peer = Some(PeerId([3; 32]));
        let adjusting = text(&people_rows(28, &app, &theme)[2]);
        assert!(
            adjusting.contains("50%"),
            "while you are moving it, the number is the whole point: {adjusting}"
        );
    }

    /// Draws the rail and reports how the named section's chip came out — its ground
    /// and its weight, which is where a lit chip differs from a resting one in colour
    /// and in mono alike. The chip is built inside `draw`, so this is the only way to
    /// ask whether a section is lit.
    fn chip_look(app: &App, label: &str) -> (ratatui::style::Color, ratatui::style::Modifier) {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let theme = Theme::from_env();
        let mut terminal = Terminal::new(TestBackend::new(28, 20)).unwrap();
        terminal
            .draw(|frame| draw(frame, frame.area(), app, &theme))
            .unwrap();
        let buffer = terminal.backend().buffer().clone();

        let row = (0..20)
            .find(|y| {
                (0..28)
                    .map(|x| buffer[(x, *y)].symbol())
                    .collect::<String>()
                    .contains(label)
            })
            .unwrap_or_else(|| panic!("the rail never drew a {label} chip"));
        // The chip's first cell is the blank the label sits on, one in from the edge.
        let cell = &buffer[(1, row)];
        (cell.bg, cell.modifier)
    }

    /// The rendered text of the row carrying `label`.
    fn chip_row(app: &App, label: &str) -> String {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let theme = Theme::from_env();
        let mut terminal = Terminal::new(TestBackend::new(28, 20)).unwrap();
        terminal
            .draw(|frame| draw(frame, frame.area(), app, &theme))
            .unwrap();
        let buffer = terminal.backend().buffer().clone();

        (0..20)
            .map(|y| (0..28).map(|x| buffer[(x, y)].symbol()).collect::<String>())
            .find(|row| row.contains(label))
            .unwrap_or_else(|| panic!("the rail never drew a {label} chip"))
    }

    #[test]
    fn the_roster_names_its_key_until_you_have_used_it() {
        let mut app = room();
        let idle = chip_row(&app, "ON THE LINE");
        assert!(
            idle.contains("↑↓"),
            "the same way AUDIO carries F6 — nothing else on screen says the roster can be steered: {idle}"
        );

        app.selected_peer = Some(PeerId([2; 32]));
        let holding = chip_row(&app, "ON THE LINE");
        assert!(
            !holding.contains("↑↓"),
            "once you are in it the footer says more than this could, so the hint gets out of the way: {holding}"
        );
    }

    #[test]
    fn the_people_section_lights_up_while_the_cursor_is_in_it() {
        let mut app = room();
        let resting = chip_look(&app, "ON THE LINE");

        app.selected_peer = Some(PeerId([2; 32]));
        let holding = chip_look(&app, "ON THE LINE");

        assert_ne!(
            resting, holding,
            "the settings screen lights its focused section; the rail has to say the same thing the same way"
        );
    }

    fn meter_style(row: &TextLine<'_>, theme: &Theme) -> ratatui::style::Style {
        row.spans
            .iter()
            .find(|span| theme.glyphs.meter.contains(&span.content.as_ref()))
            .expect("every roster row draws a meter")
            .style
    }

    #[test]
    fn a_voice_you_turned_down_reads_dim_even_while_it_is_moving() {
        let mut app = room();
        app.peers.push(plain_peer());
        app.peer_levels.insert(PeerId([3; 32]), 3);
        let theme = Theme::from_env();

        let at_full = meter_style(&people_rows(28, &app, &theme)[2], &theme);

        app.peer_gains.insert(PeerId([3; 32]), 0.5);
        let quieted = meter_style(&people_rows(28, &app, &theme)[2], &theme);

        assert_ne!(
            at_full, quieted,
            "once the number is gone the meter is the only thing left carrying the volume"
        );
        assert_eq!(
            quieted,
            theme.dim(),
            "it still moves, because they are still talking — it is just not reaching you whole"
        );
    }

    #[test]
    fn a_silenced_person_is_marked_beside_their_name_not_only_at_the_edge() {
        let mut app = room();
        app.peers.push(plain_peer());
        app.peer_levels.insert(PeerId([3; 32]), 3);
        let theme = Theme::from_env();

        let heard = text(&people_rows(28, &app, &theme)[2]);
        assert!(heard.contains(theme.glyphs.meter[3]), "sanity: {heard}");

        app.peer_gains.insert(PeerId([3; 32]), 0.0);
        let silenced = text(&people_rows(28, &app, &theme)[2]);

        assert!(
            !silenced.contains(theme.glyphs.meter[3]),
            "a level meter means nothing once none of it is reaching you: {silenced}"
        );
        assert!(
            silenced.contains(theme.glyphs.silenced),
            "the mark belongs in the column the eye already watches, not only at the far edge: {silenced}"
        );
    }

    #[test]
    fn silencing_someone_reads_as_silenced_rather_than_as_a_number() {
        let mut app = room();
        app.peers.push(plain_peer());
        app.peer_gains.insert(PeerId([3; 32]), 0.0);
        let rows = people_rows(28, &app, &Theme::from_env());
        assert!(
            text(&rows[2]).contains("silenced"),
            "zero is not a volume, it is a decision: {}",
            text(&rows[2])
        );
    }

    #[test]
    fn someone_who_muted_themselves_says_that_over_the_volume_you_gave_them() {
        let mut app = room();
        app.peer_gains.insert(PeerId([2; 32]), 0.5);
        let rows = people_rows(28, &app, &Theme::from_env());
        assert!(
            text(&rows[1]).contains("muted"),
            "why you cannot hear them right now beats what you set earlier: {}",
            text(&rows[1])
        );
    }

    #[test]
    fn unknown_hardware_still_says_something_true() {
        let app = room();
        let rows = audio_rows_of(28, &app, &Theme::from_env());
        assert!(text(&rows[0]).contains("system default"), "{}", text(&rows[0]));
    }
}
