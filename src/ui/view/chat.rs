//! What is being said, and the field you say it in.
//!
//! Messages are set in columns — time, who, what — with the names set flush against
//! what they said rather than against the gutter, so a short name does not sit a hand's
//! width from its own message. A run of messages from one person keeps only the first
//! heading, so a conversation reads as blocks instead of a wall of repeated names. The
//! whole transcript hangs from the bottom of the pane, where the newest line is.
//!
//! Before anyone has spoken the screen is not a logo: it is the connection itself,
//! drawn as two cans and the string between them.

use chrono::{Local, TimeZone};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::text::{Line as TextLine, Span};
use ratatui::widgets::{Block, Paragraph};

use super::{clip, strand};
use crate::proto::PeerId;
use crate::ui::state::{App, Line};
use crate::ui::theme::{Strand, Theme};

/// Messages from one person inside this window are one block.
const GROUPING_WINDOW: u64 = 300;
/// The time column, and the most a name column may grow to, on a roomy terminal.
const COLUMNS_WIDE: (usize, usize) = (7, 14);
/// The same where a wide gutter would eat the message.
const COLUMNS_TIGHT: (usize, usize) = (6, 10);
/// The gap between a name and what it said.
const AFTER_NAME: usize = 2;
/// However short the names get, the column keeps this much so the eye has an edge to
/// run down.
const NAME_FLOOR: usize = 4;
/// Under this the gutter takes more than the words do.
const TIGHT_UNDER: u16 = 68;
/// Under this the two cans will not fit side by side.
const DIAGRAM_NEEDS_WIDTH: u16 = 48;
const DIAGRAM_NEEDS_HEIGHT: u16 = 9;
/// The drawing stops widening past this; a string half a screen long says no more
/// than a short one.
const DIAGRAM_MAX: usize = 68;
/// The tin can, as wide as its own drawing.
const CAN_WIDTH: usize = 7;
/// How many rows the drawing takes.
const CANS_ROWS: usize = 5;
/// The air the drawing needs around it before it will share a pane with a
/// conversation. Below this the pane belongs to the words.
const CANS_GAP: usize = 4;

pub fn draw(frame: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    if area.height == 0 || area.width == 0 {
        return;
    }
    let [talk, field] = Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).areas(area);

    let height = talk.height as usize;

    // An empty screen is the drawing, placed where the eye lands.
    if app.visible_lines().is_empty() {
        frame.render_widget(Paragraph::new(diagram(talk, app, theme)), talk);
        draw_field(frame, field, app, theme);
        return;
    }

    // Otherwise the conversation hangs from the bottom, where the newest line is —
    // and while it is short enough to leave the top of a big terminal empty, the
    // drawing stays up there rather than the room going blank the moment someone
    // says hello.
    let lines = transcript(talk.width, app, theme);
    let start = lines.len().saturating_sub(height);
    let mut shown: Vec<TextLine> = Vec::with_capacity(height);

    let spare = height.saturating_sub(lines.len());
    if spare >= CANS_ROWS + CANS_GAP && talk.width >= DIAGRAM_NEEDS_WIDTH {
        // Centred in the air it is filling, rather than pinned to the top of it.
        shown.resize((spare - CANS_ROWS) / 2, TextLine::from(""));
        shown.extend(cans(talk.width, app, theme));
    }
    shown.resize(spare, TextLine::from(""));
    shown.extend_from_slice(&lines[start..]);
    frame.render_widget(Paragraph::new(shown), talk);

    draw_field(frame, field, app, theme);
}

/// The message field. No box: its own ground is enough to say "type here".
fn draw_field(frame: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    if area.height == 0 {
        return;
    }
    frame.render_widget(Block::default().style(theme.panel()), area);

    let prompt = format!(" #{} ", app.channel_name(app.viewing));
    // Everything but the prompt and the caret belongs to what you are typing.
    let room = (area.width as usize).saturating_sub(prompt.chars().count() + 1);

    let mut spans = vec![Span::styled(prompt, theme.dim())];
    if app.input.is_empty() {
        spans.push(Span::styled("say something", theme.dim()));
    } else {
        // Keep the end of a long message in view — that is where the caret is.
        let typed: String = app.input.chars().skip(app.input.chars().count().saturating_sub(room)).collect();
        spans.push(Span::styled(typed, theme.text()));
    }
    spans.push(Span::styled(theme.glyphs.caret.to_string(), theme.accent()));

    frame.render_widget(Paragraph::new(TextLine::from(spans)), area);
}

/// How much of the line goes to the time, and the ceiling for the name column. A
/// gutter that is right on a wide terminal squeezes the words out of a narrow one.
fn columns(width: u16) -> (usize, usize) {
    if width >= TIGHT_UNDER { COLUMNS_WIDE } else { COLUMNS_TIGHT }
}

/// The name column is only as wide as the names in this conversation. A room of
/// five-letter names should not be indented for a twelve-letter one that never
/// speaks.
fn name_column(visible: &[&Line], app: &App, ceiling: usize) -> usize {
    visible
        .iter()
        .filter_map(|line| match line {
            Line::Chat(chat) => Some(app.name_of(chat.from).chars().count()),
            Line::Notice { .. } => None,
        })
        .max()
        .unwrap_or(NAME_FLOOR)
        .clamp(NAME_FLOOR, ceiling)
}

fn transcript(width: u16, app: &App, theme: &Theme) -> Vec<TextLine<'static>> {
    let (time_column, ceiling) = columns(width);
    let visible = app.visible_lines();
    let names = name_column(&visible, app, ceiling);
    let gutter = time_column + names + AFTER_NAME;
    let room = (width as usize).saturating_sub(gutter + 1).max(8);
    let mut lines = Vec::new();
    let mut previous: Option<(PeerId, u64)> = None;

    for line in visible {
        match line {
            Line::Chat(chat) => {
                let grouped = previous.is_some_and(|(who, when)| {
                    who == chat.from && chat.at.saturating_sub(when) < GROUPING_WINDOW
                });
                let mine = chat.from == app.me;
                let heading = if grouped {
                    vec![Span::raw(" ".repeat(gutter))]
                } else {
                    vec![
                        Span::styled(pad(&clock(chat.at), time_column), theme.dim()),
                        // Flush right: the name belongs to the message, not to the
                        // margin.
                        Span::styled(
                            pad_left(&clip(&app.name_of(chat.from), names, theme), names),
                            if mine { theme.accent() } else { theme.strong() },
                        ),
                        Span::raw(" ".repeat(AFTER_NAME)),
                    ]
                };

                for (index, part) in fold(&chat.text, room).into_iter().enumerate() {
                    let mut spans = if index == 0 {
                        heading.clone()
                    } else {
                        vec![Span::raw(" ".repeat(gutter))]
                    };
                    spans.push(Span::styled(part, theme.text()));
                    lines.push(TextLine::from(spans));
                }
                previous = Some((chat.from, chat.at));
            }
            Line::Notice { text, at } => {
                // A notice is the room talking, not a person; it breaks any block.
                previous = None;
                for (index, part) in fold(text, room).into_iter().enumerate() {
                    let head = if index == 0 { pad(&clock(*at), time_column) } else { " ".repeat(time_column) };
                    lines.push(TextLine::from(vec![
                        Span::styled(head, theme.dim()),
                        Span::styled(" ".repeat(names.saturating_sub(1)), theme.dim()),
                        Span::styled(
                            if index == 0 {
                                format!("{}  ", theme.glyphs.note)
                            } else {
                                "   ".into()
                            },
                            theme.dim(),
                        ),
                        Span::styled(part, theme.dim()),
                    ]));
                }
            }
        }
    }
    lines
}

/// Two cans and the string between them: what the room actually is.
///
/// This replaces the usual empty-state logo because on the first run there is exactly
/// one question worth answering — am I really connected to my friend — and a drawing
/// of the live link answers it without being read.
fn diagram(area: Rect, app: &App, theme: &Theme) -> Vec<TextLine<'static>> {
    if area.width < DIAGRAM_NEEDS_WIDTH || area.height < DIAGRAM_NEEDS_HEIGHT {
        return compact(area.width, app, theme, &others_of(app));
    }
    let mut rows = cans(area.width, app, theme);
    rows.push(TextLine::from(""));
    rows.extend(closing(&margin_for(area.width), app, theme, &others_of(app)));

    // Sit the drawing a little above centre, where the eye looks first.
    let padding = (area.height as usize).saturating_sub(rows.len()) / 3;
    let mut lines = vec![TextLine::from(""); padding];
    lines.extend(rows);
    lines
}

fn others_of(app: &App) -> Vec<&str> {
    app.peers
        .iter()
        .filter(|peer| peer.id != app.me)
        .map(|peer| peer.name.as_str())
        .collect()
}

fn margin_for(width: u16) -> String {
    let span = (width as usize).saturating_sub(4).min(DIAGRAM_MAX);
    " ".repeat((width as usize).saturating_sub(span) / 2)
}

/// The drawing on its own: two cans, and the string with the link written over it.
/// Exactly `CANS_ROWS` rows, so a caller can budget for it.
fn cans(width: u16, app: &App, theme: &Theme) -> Vec<TextLine<'static>> {
    let others = others_of(app);
    let strand = strand::of(app);
    let can = &theme.glyphs.can;
    let span = (width as usize).saturating_sub(4).min(DIAGRAM_MAX);
    let margin = margin_for(width);
    let gap = span.saturating_sub(CAN_WIDTH * 2);

    let far = others.first().copied().unwrap_or("");
    let label = link_label(app, theme);
    let knot = matches!(strand, Strand::Slack);

    let over = |name: &str| -> String {
        let name = clip(name, CAN_WIDTH, theme);
        let left = (CAN_WIDTH.saturating_sub(name.chars().count())) / 2;
        format!("{}{}", " ".repeat(left), name)
    };
    let lid = format!(" {} ", can.lid);
    let plain = |text: String, theme: &Theme| Span::styled(text, theme.dim());

    let mut rows = vec![
        TextLine::from(vec![
            Span::raw(margin.clone()),
            plain(pad(&over("you"), CAN_WIDTH + gap), theme),
            plain(over(far), theme),
        ]),
        TextLine::from(vec![
            Span::raw(margin.clone()),
            Span::styled(pad(&lid, CAN_WIDTH + gap), theme.brass()),
            Span::styled(if far.is_empty() { String::new() } else { lid.clone() }, theme.brass()),
        ]),
        TextLine::from(vec![
            Span::raw(margin.clone()),
            Span::styled(can.top.to_string(), theme.text()),
            Span::styled(centre(&label, gap, theme), theme.dim()),
            Span::styled(if far.is_empty() { String::new() } else { can.top.to_string() }, theme.text()),
        ]),
    ];

    let mut middle = vec![Span::raw(margin.clone()), Span::styled(can.body.to_string(), theme.text())];
    middle.extend(string_of(gap, knot, far.is_empty(), theme, strand));
    if !far.is_empty() {
        middle.push(Span::styled(can.body.to_string(), theme.text()));
    }
    rows.push(TextLine::from(middle));

    rows.push(TextLine::from(vec![
        Span::raw(margin.clone()),
        Span::styled(pad(can.bottom, CAN_WIDTH + gap), theme.text()),
        Span::styled(if far.is_empty() { String::new() } else { can.bottom.to_string() }, theme.text()),
    ]));
    rows
}

/// The same reading for a terminal too small to draw in.
fn compact(width: u16, app: &App, theme: &Theme, others: &[&str]) -> Vec<TextLine<'static>> {
    let far = others.first().copied().unwrap_or("nobody yet");
    let string = theme.glyphs.can.string;
    let text = format!("you {string} {} {string} {far}", link_label(app, theme));
    vec![
        TextLine::from(""),
        TextLine::from(Span::styled(clip(&text, width as usize, theme), theme.dim())),
    ]
}

/// The string itself, with a knot where a relay sits in the middle of it.
fn string_of(
    gap: usize,
    knot: bool,
    hanging: bool,
    theme: &Theme,
    strand: Strand,
) -> Vec<Span<'static>> {
    let glyph = theme.glyphs.can.string;
    let (_, style) = theme.strand(strand);

    if hanging {
        // Nobody on the other end: the string just runs out.
        let mut text: String = std::iter::repeat_n(glyph, gap.saturating_sub(1)).collect();
        text.push(theme.glyphs.cut);
        return vec![Span::styled(clip(&text, gap, theme), theme.dim())];
    }
    if !knot || gap < 5 {
        return vec![Span::styled(std::iter::repeat_n(glyph, gap).collect::<String>(), style)];
    }
    let half = (gap - 1) / 2;
    vec![
        Span::styled(std::iter::repeat_n(glyph, half).collect::<String>(), style),
        Span::styled(theme.glyphs.can.knot.to_string(), theme.brass()),
        Span::styled(std::iter::repeat_n(glyph, gap - half - 1).collect::<String>(), style),
    ]
}

/// What to do next, which depends entirely on whether anyone is there.
fn closing(margin: &str, app: &App, theme: &Theme, others: &[&str]) -> Vec<TextLine<'static>> {
    if others.is_empty() {
        return vec![
            TextLine::from(vec![
                Span::raw(margin.to_string()),
                Span::styled("nobody on the line yet. send them this code:", theme.dim()),
            ]),
            TextLine::from(vec![
                Span::raw(margin.to_string()),
                Span::styled(app.invite_code.clone(), theme.brass()),
            ]),
        ];
    }
    let mut lines = Vec::new();
    if others.len() > 1 {
        lines.push(TextLine::from(vec![
            Span::raw(margin.to_string()),
            Span::styled(format!("and {} more on the line", others.len() - 1), theme.dim()),
        ]));
    }
    lines.push(TextLine::from(vec![
        Span::raw(margin.to_string()),
        Span::styled(
            format!("f2 talks in #{}", app.channel_name(app.viewing)),
            theme.dim(),
        ),
    ]));
    lines
}

fn link_label(app: &App, theme: &Theme) -> String {
    let words = strand::label(app).to_lowercase();
    match app.link.worst_rtt {
        Some(rtt) if app.link.peers() > 0 => format!("{}ms{}{words}", rtt.as_millis(), theme.glyphs.dot),
        _ => words,
    }
}

/// Wraps at spaces, and breaks a word that is longer than the column rather than
/// letting it push the layout around.
fn fold(text: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut lines = Vec::new();
    let mut current = String::new();

    for word in text.split_whitespace() {
        let mut word = word;
        while word.chars().count() > width {
            if !current.is_empty() {
                lines.push(std::mem::take(&mut current));
            }
            let head: String = word.chars().take(width).collect();
            let taken = head.len();
            lines.push(head);
            word = &word[taken..];
        }
        if word.is_empty() {
            continue;
        }
        let extra = if current.is_empty() { 0 } else { 1 };
        if current.chars().count() + extra + word.chars().count() > width {
            lines.push(std::mem::take(&mut current));
        } else if extra == 1 {
            current.push(' ');
        }
        current.push_str(word);
    }
    if !current.is_empty() || lines.is_empty() {
        lines.push(current);
    }
    lines
}

fn pad(text: &str, width: usize) -> String {
    let short = width.saturating_sub(text.chars().count());
    format!("{text}{}", " ".repeat(short))
}

fn pad_left(text: &str, width: usize) -> String {
    let short = width.saturating_sub(text.chars().count());
    format!("{}{text}", " ".repeat(short))
}

fn centre(text: &str, width: usize, theme: &Theme) -> String {
    let text = clip(text, width, theme);
    let left = width.saturating_sub(text.chars().count()) / 2;
    pad(&format!("{}{}", " ".repeat(left), text), width)
}

fn clock(at: u64) -> String {
    match Local.timestamp_opt(at as i64, 0) {
        chrono::LocalResult::Single(time) => time.format("%H:%M").to_string(),
        _ => "--:--".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proto::{ChannelId, ChatLine, PeerInfo};

    fn text(line: &TextLine<'_>) -> String {
        line.spans.iter().map(|span| span.content.as_ref()).collect()
    }

    fn screen(width: u16, height: u16, app: &App) -> String {
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(width, height)).unwrap();
        let theme = Theme::from_env();
        terminal
            .draw(|frame| draw(frame, Rect::new(0, 0, width, height), app, &theme))
            .unwrap();
        terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<Vec<_>>()
            .chunks(width as usize)
            .map(|row| row.concat().trim_end().to_string())
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn said(app: &mut App, from: u8, at: u64, what: &str) {
        app.apply(crate::net::Event::Chat(ChatLine {
            channel: ChannelId(0),
            from: PeerId([from; 32]),
            text: what.into(),
            at,
        }));
    }

    fn room() -> App {
        let mut app = App::new(PeerId([1; 32]), "n73w-kuqc-uog2".into());
        // Names reach the transcript through the roster, so the room has to be built
        // the way the coordinator builds it.
        app.apply(crate::net::Event::Welcome {
            me: PeerId([1; 32]),
            room: crate::proto::RoomSnapshot {
                room_name: "lobby".into(),
                channels: vec!["general".into()],
                peers: vec![
                    PeerInfo { id: PeerId([1; 32]), name: "alice".into(), channel: None, muted: false, deafened: false },
                    PeerInfo { id: PeerId([2; 32]), name: "bob".into(), channel: None, muted: false, deafened: false },
                ],
                recent_chat: vec![],
            },
        });
        app
    }

    #[test]
    fn a_run_of_messages_keeps_only_the_first_heading() {
        let mut app = room();
        said(&mut app, 2, 1000, "hey");
        said(&mut app, 2, 1010, "you there");
        said(&mut app, 1, 1020, "here");

        let rows = transcript(70, &app, &Theme::from_env());
        assert!(text(&rows[0]).contains("bob"), "{}", text(&rows[0]));
        assert!(!text(&rows[1]).contains("bob"), "a repeated name is noise: {}", text(&rows[1]));
        assert!(text(&rows[1]).contains("you there"));
        assert!(text(&rows[2]).contains("alice"), "a new speaker starts a new block");
    }

    #[test]
    fn a_long_gap_starts_a_new_block() {
        let mut app = room();
        said(&mut app, 2, 1000, "hey");
        said(&mut app, 2, 1000 + GROUPING_WINDOW + 1, "still there?");

        let rows = transcript(70, &app, &Theme::from_env());
        assert!(text(&rows[1]).contains("bob"), "an hour later is a new thought: {}", text(&rows[1]));
    }

    #[test]
    fn wrapped_messages_stay_in_their_column() {
        let mut app = room();
        said(&mut app, 2, 1000, "a message long enough that it has to be folded across lines");

        let rows = transcript(50, &app, &Theme::from_env());
        assert!(rows.len() > 1, "this should have wrapped");
        let (time_column, ceiling) = columns(50);
        let names = name_column(&app.visible_lines(), &app, ceiling);
        let indent = " ".repeat(time_column + names + AFTER_NAME);
        assert!(text(&rows[1]).starts_with(&indent), "continuation broke the column: {:?}", text(&rows[1]));
    }

    #[test]
    fn folding_never_loses_or_overflows_a_word() {
        let folded = fold("the quick brown fox", 9);
        assert!(folded.iter().all(|line| line.chars().count() <= 9), "{folded:?}");
        assert_eq!(folded.concat().replace(' ', ""), "thequickbrownfox");

        let long = fold("supercalifragilistic", 6);
        assert!(long.iter().all(|line| line.chars().count() <= 6), "{long:?}");
        assert_eq!(long.concat(), "supercalifragilistic");

        assert_eq!(fold("", 10), vec![""], "an empty message still takes a row");
    }

    #[test]
    fn a_narrow_terminal_gives_the_words_more_of_the_line() {
        let mut app = room();
        said(&mut app, 2, 1000, "the gutter must not eat the message");

        let narrow = transcript(52, &app, &Theme::from_env());
        let (time_column, ceiling) = columns(52);
        assert!(time_column + ceiling < COLUMNS_WIDE.0 + COLUMNS_WIDE.1);
        assert!(text(&narrow[0]).contains("bob"), "the name still has to fit: {}", text(&narrow[0]));
        assert!(narrow[0].width() <= 52, "{}", text(&narrow[0]));
    }

    #[test]
    fn a_short_name_sits_next_to_what_it_said() {
        let mut app = room();
        said(&mut app, 2, 1000, "hey");

        let row = text(&transcript(70, &app, &Theme::from_env())[0]);
        let name = row.find("bob").expect("the name must be there");
        let message = row.find("hey").expect("the message must be there");
        assert_eq!(
            message - (name + "bob".len()),
            AFTER_NAME,
            "a five letter name must not be marooned in the gutter: {row:?}"
        );
    }

    #[test]
    fn the_column_grows_for_the_names_that_are_actually_talking() {
        let short = name_column(&[], &room(), COLUMNS_WIDE.1);
        assert_eq!(short, NAME_FLOOR, "an empty room keeps the edge and no more");

        let mut app = room();
        said(&mut app, 2, 1000, "hi");
        assert_eq!(name_column(&app.visible_lines(), &app, COLUMNS_WIDE.1), "bob".len().max(NAME_FLOOR));
    }

    #[test]
    fn the_drawing_is_exactly_as_tall_as_it_claims() {
        assert_eq!(cans(70, &room(), &Theme::from_env()).len(), CANS_ROWS);
    }

    #[test]
    fn a_tall_pane_keeps_the_link_in_view_above_a_short_conversation() {
        let mut app = room();
        app.voice_available = true;
        app.link = crate::net::voice::LinkStatus {
            direct: 1,
            relayed: 0,
            worst_rtt: Some(std::time::Duration::from_millis(18)),
        };
        said(&mut app, 2, 1000, "hi");

        assert!(screen(70, 24, &app).contains("18ms"), "a big empty terminal should still show the link");
        assert!(!screen(70, 10, &app).contains("18ms"), "a full pane belongs to the conversation");
    }

    #[test]
    fn a_short_conversation_hangs_from_the_bottom_of_the_pane() {
        let mut app = room();
        said(&mut app, 2, 1000, "only one line");

        let drawn = screen(46, 12, &app);
        let rows: Vec<&str> = drawn.lines().collect();

        assert!(rows[0].is_empty(), "the top should be air, not a stranded message: {rows:?}");
        assert!(
            rows[10].contains("only one line"),
            "the last line belongs just above the field: {rows:?}"
        );
    }

    #[test]
    fn an_empty_room_shows_the_code_instead_of_a_logo() {
        let mut app = room();
        app.peers.retain(|peer| peer.id == app.me);
        let drawn: String = diagram(Rect::new(0, 0, 70, 20), &app, &Theme::from_env())
            .iter()
            .map(text)
            .collect::<Vec<_>>()
            .join("\n");

        assert!(drawn.contains("n73w-kuqc-uog2"), "the code is the next step:\n{drawn}");
        assert!(drawn.contains("nobody on the line"), "{drawn}");
    }

    #[test]
    fn a_joined_room_draws_the_other_can_and_the_link() {
        let mut app = room();
        app.voice_available = true;
        app.link = crate::net::voice::LinkStatus {
            direct: 1,
            relayed: 0,
            worst_rtt: Some(std::time::Duration::from_millis(18)),
        };
        let drawn: String = diagram(Rect::new(0, 0, 70, 20), &app, &Theme::from_env())
            .iter()
            .map(text)
            .collect::<Vec<_>>()
            .join("\n");

        assert!(drawn.contains("bob"), "the other end has a name:\n{drawn}");
        assert!(drawn.contains("18ms"), "the link is measured, not decorated:\n{drawn}");
        assert!(drawn.contains("direct"), "{drawn}");
    }

    #[test]
    fn a_relayed_link_ties_a_knot_in_the_string() {
        let theme = Theme::from_env();
        let knotted: String = string_of(30, true, false, &theme, Strand::Slack)
            .iter()
            .map(|span| span.content.as_ref())
            .collect();
        assert!(knotted.contains(theme.glyphs.can.knot), "{knotted}");
        assert_eq!(knotted.chars().count(), 30, "a knot must not stretch the string");
    }

    #[test]
    fn a_tiny_terminal_still_reports_the_link() {
        let app = room();
        let drawn: String = compact(30, &app, &Theme::from_env(), &["bob"])
            .iter()
            .map(text)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(drawn.contains("you"), "{drawn}");
    }
}
