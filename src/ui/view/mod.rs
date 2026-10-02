//! Screen layout and drawing.
//!
//! The screen is three bands — a header, the body, a footer — and the body is a rail
//! of panels, the string, and whatever is being said. Nothing here is boxed: the rail
//! is told apart from the talk by its ground and by the string between them, and each
//! section of the rail is named by a filled chip rather than a border title.
//!
//! Colour and glyph decisions all live in `theme`; this module only arranges.

mod chat;
mod rail;
mod settings;
mod strand;

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::text::{Line as TextLine, Span};
use ratatui::widgets::{Block, Paragraph};

use super::state::{App, ViewMode};
use super::theme::Theme;

/// Wide enough for a three-cell meter, a name and the channel someone is in.
const RAIL_WIDTH: u16 = 28;
/// The string, with a column of air on each side.
const STRAND_WIDTH: u16 = 3;
/// Under this the rail is dropped altogether: on a narrow terminal the talk wins.
const RAIL_NEEDS: u16 = 62;

pub fn draw(frame: &mut Frame, app: &App, theme: &Theme) {
    let area = frame.area();
    frame.render_widget(Block::default().style(theme.surface()), area);
    if area.height == 0 || area.width == 0 {
        return;
    }

    let [header, body, footer] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(0),
        Constraint::Length(1),
    ])
    .areas(area);

    draw_header(frame, header, app, theme);
    draw_footer(frame, footer, app, theme);

    let main = if area.width >= RAIL_NEEDS && body.height >= 4 {
        let [rail_area, strand_area, main] = Layout::horizontal([
            Constraint::Length(RAIL_WIDTH),
            Constraint::Length(STRAND_WIDTH),
            Constraint::Min(20),
        ])
        .areas(body);
        rail::draw(frame, rail_area, app, theme);
        strand::draw(frame, strand_area, app, theme);
        main
    } else {
        body
    };

    match app.view_mode {
        ViewMode::Chat => chat::draw(frame, main, app, theme),
        ViewMode::Settings => settings::draw(frame, main, app, theme),
    }
}

/// Who you are with, and how well you are reaching them. The link chip is the same
/// reading as the string, in words.
fn draw_header(frame: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    frame.render_widget(Block::default().style(theme.panel()), area);

    let mut left = vec![
        Span::raw(" "),
        Span::styled(" TINCAN ", theme.chip_on()),
        Span::raw(" "),
    ];
    if !app.room_name.is_empty() {
        left.push(Span::styled(app.room_name.clone(), theme.strong()));
        left.push(Span::raw(" "));
    }
    match app.view_mode {
        ViewMode::Settings => {
            let chip_style = if app.is_transitioning() {
                theme.chip_on()
            } else {
                theme.chip()
            };
            left.push(Span::styled(" SETTINGS ", chip_style));
        }
        ViewMode::Chat => {
            if !app.channels.is_empty() {
                let channel_style = if app.is_transitioning() {
                    theme.strong()
                } else {
                    theme.accent()
                };
                left.push(Span::styled(
                    format!("#{}", app.channel_name(app.viewing)),
                    channel_style,
                ));
            }
        }
    }

    // The chip summarises the room. When more than one person is on the line and one
    // of them is the problem, it says who, so that a terminal too narrow for the
    // roster still answers the question. If that does not fit, the name goes first,
    // then the number.
    let strand = strand::of(app);
    let named = if app.link.peers() >= 2 {
        named_trouble(app)
    } else {
        None
    };
    let who = named.and_then(|(id, _, link)| {
        app.peers
            .iter()
            .find(|p| p.id == id)
            .map(|p| (p.name.clone(), link.rtt))
    });
    let rtt = who.as_ref().map(|(_, rtt)| *rtt).or(app.link.worst_rtt);

    let mut base = Vec::new();
    if app.recently_dropped() {
        base.push(Span::styled("audio dropping ", theme.error()));
    }
    base.push(Span::styled(
        format!(" {} ", strand::label(app)),
        theme.chip_link(strand),
    ));
    let name = who.map(|(name, _)| {
        Span::styled(
            format!("  {}", clip(&name, rail::NAME_ROOM, theme)),
            theme.text(),
        )
    });
    let number = rtt.map(|rtt| Span::styled(format!(" {}", millis(rtt)), theme.dim()));

    let with = |parts: &[&Option<Span<'static>>]| {
        let mut right = base.clone();
        right.extend(parts.iter().filter_map(|part| (*part).clone()));
        right.push(Span::raw(" "));
        right
    };
    let fits = |right: &Vec<Span<'static>>| {
        let used: usize = left.iter().chain(right.iter()).map(Span::width).sum();
        used < area.width as usize
    };
    let right = [with(&[&name, &number]), with(&[&number])]
        .into_iter()
        .find(fits)
        .unwrap_or_else(|| with(&[]));

    frame.render_widget(Paragraph::new(spread(area.width, left, right)), area);
}

/// The shortcuts, always. They are what a newcomer needs most and they cost one row.
fn draw_footer(frame: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    frame.render_widget(Block::default().style(theme.panel()), area);

    let right = vec![
        Span::styled("f1 code ", theme.dim()),
        Span::styled(short_code(&app.invite_code, theme), theme.brass()),
        Span::raw(" "),
    ];
    // One column for the indent, one so the two halves never touch.
    let used: usize = right.iter().map(Span::width).sum();
    let room = (area.width as usize).saturating_sub(used + 2);

    let hints = match &app.status {
        Some(status) => clip(status, room, theme),
        None => fit(app, room, theme),
    };
    let left = vec![Span::raw(" "), Span::styled(hints, theme.dim())];

    frame.render_widget(Paragraph::new(spread(area.width, left, right)), area);
}

/// The shortcut line, shortened a step at a time rather than cut mid-word.
fn fit(app: &App, width: usize, theme: &Theme) -> String {
    let full = if app.view_mode == ViewMode::Settings {
        [
            "tab section · ↑↓ move · ←→ adjust · a measure · space toggle · m live · esc back",
            "tab section · ↑↓ move · ←→ adjust · enter apply · esc back",
            "tab section · ↑↓ move · ←→ adjust · esc back",
            "↑↓ move · ←→ adjust · esc back",
            "esc back",
        ]
    } else if app.selected_peer.is_some() {
        // While a name is picked out, the row tells you what the keys now do to that
        // one person. Nothing else on this line changes meaning, and these three keys
        // have nowhere else to announce themselves.
        [
            "↑↓ person · ←→ volume · ctrl+k silence · esc done · f2 talk · f6 audio · ctrl+c",
            "↑↓ person · ←→ volume · ctrl+k silence · esc done · f2 talk · ctrl+c",
            "↑↓ person · ←→ volume · ctrl+k silence · esc done · f2 talk",
            "←→ volume · ctrl+k silence · esc done · f2 talk",
            "esc done · f2 talk",
        ]
    } else {
        [
            "tab channel · f2 talk · f3 mute · f5 deafen · f6 audio · ctrl+c quit",
            "tab channel · f2 talk · f3 mute · f6 audio · ctrl+c quit",
            "tab channel · f2 talk · f3 mute · f6 audio · ctrl+c",
            "tab · f2 talk · f6 audio · ctrl+c",
            "ctrl+c quit",
        ]
    };
    for step in full {
        let step = theme.plainly(step);
        if step.chars().count() <= width {
            return step;
        }
    }
    clip(&theme.plainly(full[4]), width, theme)
}

/// Puts `right` against the right edge, `left` against the left.
fn spread(width: u16, left: Vec<Span<'static>>, right: Vec<Span<'static>>) -> TextLine<'static> {
    let used: usize = left.iter().chain(right.iter()).map(Span::width).sum();
    let gap = (width as usize).saturating_sub(used);
    let mut spans = left;
    if gap > 0 {
        spans.push(Span::raw(" ".repeat(gap)));
        spans.extend(right);
    }
    TextLine::from(spans)
}

/// The link worth naming on screen, if any. While audio is breaking up nobody is
/// named: a dropout is measured in our own playback and does not say whose audio was
/// late, and a name beside `CHOPPY` would read as blame.
fn named_trouble(
    app: &App,
) -> Option<(
    crate::proto::PeerId,
    crate::ui::state::Trouble,
    crate::net::voice::PeerLink,
)> {
    if matches!(strand::of(app), crate::ui::theme::Strand::Frayed) {
        return None;
    }
    app.worst_trouble()
}

/// A round trip as the interface writes it everywhere. Past a second the exact figure
/// stops mattering and would not fit the roster, so it is capped rather than cut.
/// Under a millisecond (one machine, or a quiet LAN) the truncated figure would read
/// `0ms`, which looks like no reading at all, so it says `<1ms` instead.
fn millis(rtt: std::time::Duration) -> String {
    match rtt.as_millis() {
        0 => "<1ms".to_string(),
        ms @ 1..=999 => format!("{ms}ms"),
        _ => ">999ms".to_string(),
    }
}

/// Cuts to width, with an ellipsis when something was lost.
fn clip(text: &str, width: usize, theme: &Theme) -> String {
    if text.chars().count() <= width {
        return text.to_string();
    }
    if width <= 1 {
        return text.chars().take(width).collect();
    }
    let kept: String = text.chars().take(width - 1).collect();
    format!("{kept}{}", theme.glyphs.cut)
}

fn short_code(code: &str, theme: &Theme) -> String {
    let head: String = code.chars().take(9).collect();
    format!("{head}{}", theme.glyphs.cut)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proto::PeerId;
    use crate::ui::state::App;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn room() -> App {
        let mut app = App::new(PeerId([1; 32]), "n73w-kuqc-uog2".into());
        app.room_name = "lobby".into();
        app.channels = vec!["general".into(), "gaming".into(), "music".into()];
        app
    }

    fn rendered(width: u16, height: u16, app: &App) -> String {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        let theme = Theme::from_env();
        terminal.draw(|frame| draw(frame, app, &theme)).unwrap();
        terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<Vec<_>>()
            .chunks(width as usize)
            .map(|row| row.concat())
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn draws_without_panicking_at_awkward_sizes() {
        for (width, height) in [(80, 24), (20, 8), (200, 60), (10, 5), (1, 1), (62, 4)] {
            let mut app = room();
            rendered(width, height, &app);
            app.view_mode = ViewMode::Settings;
            rendered(width, height, &app);
        }
    }

    #[test]
    fn the_room_and_the_channel_are_named_in_the_header() {
        let screen = rendered(80, 24, &room());
        let header = screen.lines().next().unwrap().to_string();
        assert!(header.contains("TINCAN"), "{header}");
        assert!(header.contains("lobby"), "{header}");
        assert!(header.contains("#general"), "{header}");
    }

    #[test]
    fn settings_mode_names_settings_in_the_header_and_separator_in_body() {
        let mut app = room();
        app.view_mode = ViewMode::Settings;
        let screen = rendered(80, 24, &app);
        let header = screen.lines().next().unwrap().to_string();
        assert!(header.contains("TINCAN"), "{header}");
        assert!(header.contains("lobby"), "{header}");
        assert!(header.contains("SETTINGS"), "{header}");
        assert!(!header.contains("#general"), "{header}");
        assert!(screen.contains("AUDIO SETTINGS"), "{screen}");
    }

    #[test]
    fn the_shortcuts_are_always_on_screen() {
        let screen = rendered(80, 24, &room());
        assert!(screen.contains("ctrl+c"), "{screen}");
    }

    #[test]
    fn the_settings_hints_name_the_keys_that_screen_actually_has() {
        let mut app = room();
        app.view_mode = ViewMode::Settings;
        let theme = Theme::from_env();

        let full = fit(&app, 80, &theme);
        assert!(
            full.contains("←→"),
            "the dials are only discoverable from here: {full}"
        );
        for width in [70, 55, 40, 20, 4] {
            assert!(fit(&app, width, &theme).chars().count() <= width);
        }
    }

    #[test]
    fn picking_someone_out_of_the_roster_puts_their_keys_in_the_footer() {
        let mut app = room();
        app.selected_peer = Some(crate::proto::PeerId([2; 32]));
        let theme = Theme::from_env();

        let full = fit(&app, 80, &theme);
        assert!(
            full.contains("←→"),
            "one person's volume is only discoverable from here: {full}"
        );
        assert!(full.contains("ctrl+k"), "and so is silencing them: {full}");

        for width in [70, 55, 40, 20, 4] {
            assert!(fit(&app, width, &theme).chars().count() <= width);
        }
    }

    #[test]
    fn the_key_that_joins_a_channel_survives_the_roster_taking_the_footer() {
        let mut app = room();
        app.selected_peer = Some(PeerId([2; 32]));
        let theme = Theme::from_env();

        // The drawing that says "f2 talks in #music" is only up while the room has
        // said nothing at all, so once anyone speaks the footer is the only thing
        // left naming the app's primary action. Picking someone out of the roster
        // must not cost it.
        for width in [80, 60, 50, 40] {
            let line = fit(&app, width, &theme);
            assert!(line.contains("f2"), "at {width} columns: {line}");
            assert!(line.chars().count() <= width);
        }
    }

    #[test]
    fn the_invite_code_survives_a_crowded_footer() {
        // The shortcuts shorten so the code keeps its corner; the code is the one
        // thing on that row nobody can retype from memory.
        for width in [72, 76, 80, 100] {
            let screen = rendered(width, 24, &room());
            let footer = screen.lines().last().unwrap();
            assert!(
                footer.contains("f1 code"),
                "lost the code at {width}: {footer}"
            );
        }
    }

    #[test]
    fn a_narrow_terminal_drops_the_rail_and_keeps_the_talk() {
        let wide = rendered(80, 24, &room());
        assert!(
            wide.contains("CHANNELS"),
            "the rail belongs on a normal terminal"
        );

        let narrow = rendered(50, 24, &room());
        assert!(
            !narrow.contains("CHANNELS"),
            "the rail must give way:\n{narrow}"
        );
        assert!(
            narrow.contains("say something"),
            "the message field must survive:\n{narrow}"
        );
    }

    #[test]
    fn a_plain_terminal_gets_a_screen_it_can_actually_print() {
        let mut app = room();
        app.peers = vec![crate::proto::PeerInfo {
            id: PeerId([2; 32]),
            name: "a-name-too-long-for-the-rail".into(),
            channel: Some(crate::proto::ChannelId(0)),
            muted: false,
            deafened: false,
            afk: false,
        }];
        app.voice_available = true;

        let mut terminal = Terminal::new(TestBackend::new(84, 22)).unwrap();
        let plain = Theme::austere();
        terminal.draw(|frame| draw(frame, &app, &plain)).unwrap();

        let screen: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(screen.is_ascii(), "a glyph escaped the fallback:\n{screen}");
    }

    #[test]
    fn clipping_marks_what_it_cut() {
        let theme = Theme::from_env();
        assert_eq!(clip("general", 20, &theme), "general");
        assert_eq!(
            clip("a very long device name", 8, &theme).chars().count(),
            8
        );
        assert!(clip("a very long device name", 8, &theme).starts_with("a very"));
        assert_eq!(clip("abc", 0, &theme), "");
    }

    #[test]
    fn the_shortcut_line_shortens_instead_of_breaking() {
        let app = room();
        let theme = Theme::from_env();
        assert!(fit(&app, 80, &theme).contains("f3 mute"));
        assert!(
            fit(&app, 55, &theme).contains("f3 mute"),
            "the ladder must not skip a rung"
        );
        assert!(fit(&app, 40, &theme).chars().count() <= 40);
        assert!(fit(&app, 12, &theme).chars().count() <= 12);
        assert!(fit(&app, 3, &theme).chars().count() <= 3);
    }
    #[test]
    fn millis_caps_at_999() {
        use std::time::Duration;
        assert_eq!(millis(Duration::from_millis(340)), "340ms");
        assert_eq!(millis(Duration::from_millis(999)), "999ms");
        assert_eq!(millis(Duration::from_millis(1000)), ">999ms");
        assert_eq!(millis(Duration::from_secs(120)), ">999ms");
    }
    #[test]
    fn millis_says_under_one_rather_than_zero() {
        use std::time::Duration;
        assert_eq!(millis(Duration::ZERO), "<1ms");
        assert_eq!(millis(Duration::from_micros(999)), "<1ms");
        assert_eq!(millis(Duration::from_millis(1)), "1ms");
    }
    use crate::net::voice::{LinkStatus, PeerLink};
    use crate::proto::{ChannelId, PeerInfo};

    /// Us plus `names`, all in general, with voice up and the given links (seed = index + 2).
    fn call(names: &[&str], readings: &[(u8, bool, u64)]) -> App {
        let mut app = room();
        let person = |seed: u8, name: &str| PeerInfo {
            id: PeerId([seed; 32]),
            name: name.into(),
            channel: Some(ChannelId(0)),
            muted: false,
            deafened: false,
            afk: false,
        };
        app.peers = std::iter::once(person(1, "alice"))
            .chain(
                names
                    .iter()
                    .enumerate()
                    .map(|(i, name)| person(i as u8 + 2, name)),
            )
            .collect();
        app.voice = Some(ChannelId(0));
        app.voice_available = true;
        let per_peer: std::collections::BTreeMap<_, _> = readings
            .iter()
            .map(|&(seed, relayed, ms)| {
                (
                    PeerId([seed; 32]),
                    PeerLink {
                        relayed,
                        rtt: std::time::Duration::from_millis(ms),
                    },
                )
            })
            .collect();
        app.take_link(LinkStatus {
            direct: per_peer.values().filter(|l| !l.relayed).count(),
            relayed: per_peer.values().filter(|l| l.relayed).count(),
            worst_rtt: per_peer.values().map(|l| l.rtt).max(),
            per_peer,
        });
        app
    }

    fn header(width: u16, app: &App) -> String {
        rendered(width, 12, app).lines().next().unwrap().to_string()
    }

    #[test]
    fn a_two_person_call_keeps_the_header_it_has_today() {
        let app = call(&["bob"], &[(2, true, 340)]);
        let top = header(80, &app);
        assert!(top.contains("RELAY") && top.contains("340ms"), "{top}");
        assert!(
            !top.contains("bob"),
            "with one other person the name says nothing new: {top}"
        );
    }

    #[test]
    fn a_crowded_call_names_the_relayed_person() {
        let app = call(
            &["bob", "cem", "deniz", "emre"],
            &[
                (2, false, 18),
                (3, false, 22),
                (4, false, 31),
                (5, true, 340),
            ],
        );
        let top = header(80, &app);
        assert!(
            top.contains("RELAY") && top.contains("emre") && top.contains("340ms"),
            "{top}"
        );
    }

    #[test]
    fn a_crowded_healthy_call_names_nobody() {
        let app = call(&["bob", "cem"], &[(2, false, 18), (3, false, 31)]);
        let top = header(80, &app);
        assert!(top.contains("DIRECT") && top.contains("31ms"), "{top}");
        assert!(!top.contains("bob") && !top.contains("cem"), "{top}");
    }

    #[test]
    fn a_narrow_header_lets_the_name_go_before_the_number() {
        let app = call(
            &["bob", "cem", "deniz", "emre"],
            &[
                (2, false, 18),
                (3, false, 22),
                (4, false, 31),
                (5, true, 340),
            ],
        );
        let top = header(40, &app);
        assert!(top.contains("RELAY") && top.contains("340ms"), "{top}");
        assert!(!top.contains("emre"), "{top}");
    }

    #[test]
    fn a_choppy_call_blames_nobody_in_the_header() {
        let mut app = call(
            &["bob", "cem", "deniz", "emre"],
            &[
                (2, false, 18),
                (3, false, 22),
                (4, false, 31),
                (5, true, 340),
            ],
        );
        app.dropped_at = Some(std::time::Instant::now());
        let top = header(80, &app);
        assert!(
            top.contains("audio dropping") && top.contains("CHOPPY"),
            "{top}"
        );
        assert!(
            !top.contains("emre"),
            "dropouts are not anyone's fault we can name: {top}"
        );
    }

    #[test]
    fn at_48_columns_the_whole_reading_fits() {
        let app = call(
            &["bob", "cem", "deniz", "emre"],
            &[
                (2, false, 18),
                (3, false, 22),
                (4, false, 31),
                (5, true, 340),
            ],
        );
        let top = header(48, &app);
        assert!(
            top.contains("RELAY") && top.contains("emre") && top.contains("340ms"),
            "{top}"
        );
    }

    #[test]
    fn when_only_the_chip_fits_the_number_goes_too() {
        let app = call(
            &["bob", "cem", "deniz", "emre"],
            &[
                (2, false, 18),
                (3, false, 22),
                (4, false, 31),
                (5, true, 340),
            ],
        );
        let top = header(34, &app);
        assert!(top.contains("RELAY"), "{top}");
        assert!(!top.contains("340ms") && !top.contains("emre"), "{top}");
    }

    #[test]
    fn a_long_name_is_cut_where_the_rail_cuts_it() {
        let app = call(
            &["bob", "bartholomew-the-great"],
            &[(2, false, 18), (3, true, 340)],
        );
        let top = header(80, &app);
        assert!(
            top.contains("bartholomew"),
            "cut at 12 like the rail: {top}"
        );
        assert!(!top.contains("bartholomew-the-great"), "{top}");
        assert!(top.contains("340ms"), "{top}");
    }
}

/// Regenerates the README's pictures from the real renderer.
///
/// Not a test — it asserts nothing. It lives here because `view` is private, it is
/// `#[ignore]`d so CI never runs it, and it is next to the code it draws so a screen
/// that changes shape cannot leave the README describing an interface that is gone.
///
///     cargo test --lib -- --ignored --nocapture readme_pictures
#[cfg(test)]
mod pictures {
    use super::*;
    use crate::net::Event;
    use crate::proto::{ChannelId, ChatLine, PeerId, PeerInfo, RoomSnapshot};
    use crate::ui::state::{App, SettingsSection};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Buffer;
    use ratatui::style::{Color, Modifier};

    /// One character cell, in pixels. The ratio is the one monospace faces settle on.
    const CELL_W: f32 = 8.6;
    const CELL_H: f32 = 18.0;
    const FONT: f32 = 14.5;
    /// Air around the drawing, so it does not sit against the edge of the picture.
    const PAD: f32 = 14.0;

    fn hex(colour: Color, fallback: &str) -> String {
        match colour {
            Color::Rgb(r, g, b) => format!("#{r:02x}{g:02x}{b:02x}"),
            _ => fallback.to_string(),
        }
    }

    fn escape(text: &str) -> String {
        text.replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
    }

    /// Draws the interface and writes it out as SVG.
    ///
    /// Every run of same-looking cells is one `<text>` pinned to an exact width, so the
    /// columns hold whatever monospace face the reader's browser happens to pick — the
    /// thing a pasted block of terminal text cannot promise.
    fn svg(app: &App, cols: u16, rows: u16) -> String {
        flat_svg(app, cols, rows, &Theme::from_env(), 8.0)
    }

    /// The same drawing with the theme and corner radius chosen by the caller. The
    /// showcase wants square corners: a rounded picture leaves transparent corners that
    /// a GIF can only fill with black.
    fn flat_svg(app: &App, cols: u16, rows: u16, theme: &Theme, radius: f32) -> String {
        let mut terminal = Terminal::new(TestBackend::new(cols, rows)).unwrap();
        terminal.draw(|frame| draw(frame, app, theme)).unwrap();
        let buffer = terminal.backend().buffer().clone();

        let ground = hex(theme.surface().bg.unwrap_or(Color::Reset), "#1a1512");
        let ink = hex(theme.surface().fg.unwrap_or(Color::Reset), "#dcd5cb");
        let (w, h) = (
            cols as f32 * CELL_W + PAD * 2.0,
            rows as f32 * CELL_H + PAD * 2.0,
        );

        let mut out = format!(
            "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{w:.0}\" height=\"{h:.0}\" \
             viewBox=\"0 0 {w:.0} {h:.0}\" font-family=\"ui-monospace,SFMono-Regular,\
             Menlo,Consolas,'Liberation Mono',monospace\" font-size=\"{FONT}\">\n\
             <rect width=\"{w:.0}\" height=\"{h:.0}\" rx=\"{radius}\" fill=\"{ground}\"/>\n"
        );

        for y in 0..rows {
            // Backgrounds first, so no glyph is painted over.
            let mut x = 0;
            while x < cols {
                let bg = hex(buffer[(x, y)].bg, &ground);
                let start = x;
                while x < cols && hex(buffer[(x, y)].bg, &ground) == bg {
                    x += 1;
                }
                if bg != ground {
                    out.push_str(&format!(
                        "<rect x=\"{:.1}\" y=\"{:.1}\" width=\"{:.1}\" height=\"{CELL_H}\" fill=\"{bg}\"/>\n",
                        PAD + start as f32 * CELL_W,
                        PAD + y as f32 * CELL_H,
                        (x - start) as f32 * CELL_W,
                    ));
                }
            }

            out.push_str(&glyphs(&buffer, cols, y, PAD, PAD, &ink));
        }
        out.push_str("</svg>\n");
        out
    }

    /// One row of the buffer's text, as `<text>` elements pinned to the cell grid.
    ///
    /// A run of plain ASCII shares one element stretched to its exact width. Anything
    /// else, box drawing above all, gets an element of its own: the face that renders
    /// it is often a fallback whose advance is not the ASCII one, and stretching a run
    /// that mixes the two spreads the difference over every gap, so the far end of a
    /// long row lands a column or so off.
    fn glyphs(buffer: &Buffer, cols: u16, y: u16, left: f32, top: f32, ink: &str) -> String {
        let mut out = String::new();
        let look = |at: u16| {
            let cell = &buffer[(at, y)];
            (hex(cell.fg, ink), cell.modifier.contains(Modifier::BOLD))
        };
        let plain = |at: u16| {
            let symbol = buffer[(at, y)].symbol();
            symbol.is_ascii() && symbol != " "
        };

        let mut x = 0;
        while x < cols {
            let symbol = buffer[(x, y)].symbol();
            if symbol.trim().is_empty() {
                x += 1;
                continue;
            }
            let (fg, bold) = look(x);
            let start = x;
            let mut run = symbol.to_string();
            x += 1;
            if plain(start) {
                while x < cols && plain(x) && look(x) == (fg.clone(), bold) {
                    run.push_str(buffer[(x, y)].symbol());
                    x += 1;
                }
            } else {
                // A wide glyph owns the blank cells that follow it.
                while x < cols && buffer[(x, y)].symbol().is_empty() {
                    x += 1;
                }
            }
            let weight = if bold { " font-weight=\"600\"" } else { "" };
            out.push_str(&format!(
                "<text x=\"{:.1}\" y=\"{:.1}\" fill=\"{fg}\"{weight} \
                 textLength=\"{:.1}\" lengthAdjust=\"spacing\" xml:space=\"preserve\">{}</text>\n",
                left + start as f32 * CELL_W,
                top + y as f32 * CELL_H + CELL_H * 0.74,
                (x - start) as f32 * CELL_W,
                escape(&run),
            ));
        }
        out
    }

    fn peer(seed: u8, name: &str, channel: Option<ChannelId>) -> PeerInfo {
        PeerInfo {
            id: PeerId([seed; 32]),
            name: name.into(),
            channel,
            muted: false,
            deafened: false,
            afk: false,
        }
    }

    /// The room mid-conversation: three people, one of them turned down.
    fn hero() -> App {
        let me = PeerId([1; 32]);
        let mut app = App::new(
            me,
            "n73w-kuqc-uog2-4mfx-a7bp-9dlt-2ksv-wq3e-hj5n-x8cr-vy6a-2ptm-4z".into(),
        );
        app.apply(Event::Welcome {
            me,
            room: RoomSnapshot {
                room_name: "lobby".into(),
                channels: vec!["general".into(), "gaming".into(), "music".into()],
                peers: vec![
                    peer(1, "alice", Some(ChannelId(0))),
                    peer(2, "bob", Some(ChannelId(0))),
                    peer(3, "cem", Some(ChannelId(1))),
                ],
                recent_chat: vec![],
            },
        });
        app.voice = Some(ChannelId(0));
        app.voice_available = true;
        // Held still: a picture cannot show a pulse travelling, and a frozen one
        // reads as a stray character rather than as motion.
        app.motion = false;
        app.link = crate::net::voice::LinkStatus {
            direct: 2,
            relayed: 0,
            worst_rtt: Some(std::time::Duration::from_millis(18)),
            ..Default::default()
        };
        app.active_input_name = Some("MacBook Pro Microphone".into());
        app.active_output_name = Some("AirPods Pro".into());
        app.peer_levels.insert(PeerId([2; 32]), 3);
        app.speaking.insert(PeerId([2; 32]));

        let said = [
            (2u8, "hey, bob here"),
            (2, "can you hear me alright?"),
            (1, "loud and clear"),
            (2, "oh that is the round trip time on the string?"),
            (1, "yes, and the pulse speed is the latency"),
            (3, "cem here, joining from gaming"),
            (1, "it frays when audio drops out too"),
            (2, "and the meters move with each voice"),
        ];
        for (index, (from, text)) in said.iter().enumerate() {
            app.apply(Event::Chat(ChatLine {
                channel: ChannelId(0),
                from: PeerId([*from; 32]),
                text: (*text).into(),
                at: 1_757_000_000 + index as u64 * 47,
            }));
        }
        // The point of the picture: one voice turned down without deafening the room.
        app.peer_gains.insert(PeerId([3; 32]), 0.0);
        app
    }

    fn device(name: &str, rate: u32, default: bool) -> crate::audio::device::AudioDeviceInfo {
        crate::audio::device::AudioDeviceInfo {
            name: name.into(),
            sample_rate: rate,
            channels: 2,
            is_default: default,
            is_supported: rate == 48_000,
        }
    }

    /// The audio screen: what is open, where the noise floor sits, how loud the keys are.
    fn settings() -> App {
        let mut app = hero();
        app.view_mode = ViewMode::Settings;
        app.settings_section = SettingsSection::InputDevice;
        app.input_devices = vec![
            device("MacBook Pro Microphone", 48_000, true),
            device("AirPods Pro", 48_000, false),
            device("BlackHole 2ch", 48_000, false),
        ];
        app.output_devices = vec![
            device("MacBook Pro Speakers", 48_000, true),
            device("AirPods Pro", 48_000, false),
            device("Studio Display Speakers", 48_000, false),
        ];
        app.selected_input_idx = 0;
        app.selected_output_idx = 1;
        // Mid-sentence, comfortably over a floor set just above the room.
        app.mic_level = 0.46;
        app.input_gate = 0.29;
        app.typing_clicks = true;
        app.typing_volume = 0.4;
        app
    }

    #[test]
    #[ignore = "writes assets/; run it on purpose when a screen changes shape"]
    fn readme_pictures() {
        for (name, app, rows) in [("room", hero(), 22u16), ("audio", settings(), 22)] {
            let path = format!("assets/{name}.svg");
            let picture = svg(&app, 84, rows);
            std::fs::write(&path, &picture)
                .unwrap_or_else(|e| panic!("could not write {path}: {e}"));
            println!("{path} — {} bytes", picture.len());
        }
    }

    #[derive(Clone, Copy)]
    struct Camera {
        x: f32,
        y: f32,
        zoom: f32,
    }

    impl Camera {
        fn lerp(&self, target: &Camera, t: f32) -> Camera {
            let t = t.clamp(0.0, 1.0);
            let s = t * t * (3.0 - 2.0 * t); // smoothstep
            Camera {
                x: self.x + (target.x - self.x) * s,
                y: self.y + (target.y - self.y) * s,
                zoom: self.zoom + (target.zoom - self.zoom) * s,
            }
        }
    }

    const CANVAS_W: f32 = 1280.0;
    const CANVAS_H: f32 = 720.0;
    const WIN_BAR_H: f32 = 36.0;

    fn studio_svg(app: &App, cols: u16, rows: u16, camera: &Camera) -> String {
        let theme = Theme::dark_true();
        let mut terminal = Terminal::new(TestBackend::new(cols, rows)).unwrap();
        terminal.draw(|frame| draw(frame, app, &theme)).unwrap();
        let buffer = terminal.backend().buffer().clone();

        let ground = hex(theme.surface().bg.unwrap_or(Color::Reset), "#14100c");
        let ink = hex(theme.surface().fg.unwrap_or(Color::Reset), "#dcd5cb");

        let vw = CANVAS_W / camera.zoom;
        let vh = CANVAS_H / camera.zoom;
        let vx = camera.x - vw / 2.0;
        let vy = camera.y - vh / 2.0;

        let win_w = cols as f32 * CELL_W + PAD * 2.0;
        let win_h = rows as f32 * CELL_H + PAD * 2.0 + WIN_BAR_H;
        let win_x = (CANVAS_W - win_w) / 2.0;
        let win_y = (CANVAS_H - win_h) / 2.0;
        let content_x = win_x + PAD;
        let content_y = win_y + WIN_BAR_H + PAD;

        let room_title = if app.room_name.is_empty() {
            "tincan".to_string()
        } else {
            format!(
                "tincan — {} (#{})",
                app.room_name,
                app.channel_name(app.viewing)
            )
        };

        let mut out = format!(
            "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{CANVAS_W:.0}\" height=\"{CANVAS_H:.0}\" \
             viewBox=\"{vx:.2} {vy:.2} {vw:.2} {vh:.2}\" font-family=\"ui-monospace,SFMono-Regular,\
             Menlo,Consolas,'Liberation Mono',monospace\" font-size=\"{FONT}\">\n\
             <defs>\n\
               <filter id=\"shadow\" x=\"-15%\" y=\"-15%\" width=\"130%\" height=\"135%\">\n\
                 <feDropShadow dx=\"0\" dy=\"20\" stdDeviation=\"28\" flood-color=\"#000000\" flood-opacity=\"0.65\"/>\n\
               </filter>\n\
               <clipPath id=\"window-clip\">\n\
                 <rect x=\"{win_x:.1}\" y=\"{win_y:.1}\" width=\"{win_w:.1}\" height=\"{win_h:.1}\" rx=\"12\"/>\n\
               </clipPath>\n\
             </defs>\n\
             <rect width=\"{CANVAS_W:.0}\" height=\"{CANVAS_H:.0}\" fill=\"#0e0b08\"/>\n\
             <g filter=\"url(#shadow)\">\n\
               <rect x=\"{win_x:.1}\" y=\"{win_y:.1}\" width=\"{win_w:.1}\" height=\"{win_h:.1}\" rx=\"12\" fill=\"#211a14\" stroke=\"#382d23\" stroke-width=\"1.5\"/>\n\
             </g>\n\
             <g clip-path=\"url(#window-clip)\">\n\
               <rect x=\"{win_x:.1}\" y=\"{win_y:.1}\" width=\"{win_w:.1}\" height=\"{win_h:.1}\" fill=\"#211a14\"/>\n\
               <circle cx=\"{:.1}\" cy=\"{:.1}\" r=\"5.5\" fill=\"#ff5f56\"/>\n\
               <circle cx=\"{:.1}\" cy=\"{:.1}\" r=\"5.5\" fill=\"#ffbd2e\"/>\n\
               <circle cx=\"{:.1}\" cy=\"{:.1}\" r=\"5.5\" fill=\"#27c93f\"/>\n\
               <text x=\"{:.1}\" y=\"{:.1}\" fill=\"#8a8177\" font-family=\"-apple-system,BlinkMacSystemFont,'SF Pro Text',sans-serif\" font-size=\"12\" font-weight=\"500\" text-anchor=\"middle\">{}</text>\n\
               <rect x=\"{win_x:.1}\" y=\"{:.1}\" width=\"{win_w:.1}\" height=\"{:.1}\" fill=\"{ground}\"/>\n",
            win_x + 20.0,
            win_y + 18.0,
            win_x + 38.0,
            win_y + 18.0,
            win_x + 56.0,
            win_y + 18.0,
            win_x + win_w / 2.0,
            win_y + 22.0,
            escape(&room_title),
            win_y + WIN_BAR_H,
            win_h - WIN_BAR_H
        );

        for y in 0..rows {
            let mut x = 0;
            while x < cols {
                let bg = hex(buffer[(x, y)].bg, &ground);
                let start = x;
                while x < cols && hex(buffer[(x, y)].bg, &ground) == bg {
                    x += 1;
                }
                if bg != ground {
                    out.push_str(&format!(
                        "<rect x=\"{:.1}\" y=\"{:.1}\" width=\"{:.1}\" height=\"{CELL_H}\" fill=\"{bg}\"/>\n",
                        content_x + start as f32 * CELL_W,
                        content_y + y as f32 * CELL_H,
                        (x - start) as f32 * CELL_W,
                    ));
                }
            }

            out.push_str(&glyphs(&buffer, cols, y, content_x, content_y, &ink));
        }
        out.push_str("</g>\n</svg>\n");
        out
    }

    /// One reading of the mesh, as `link_status` would hand it over:
    /// (who, through a relay, round trip in ms).
    fn mesh_reading(readings: &[(u8, bool, u64)]) -> crate::net::voice::LinkStatus {
        let per_peer: std::collections::BTreeMap<_, _> = readings
            .iter()
            .map(|&(seed, relayed, ms)| {
                let rtt = std::time::Duration::from_millis(ms);
                (
                    PeerId([seed; 32]),
                    crate::net::voice::PeerLink { relayed, rtt },
                )
            })
            .collect();
        crate::net::voice::LinkStatus {
            direct: per_peer.values().filter(|l| !l.relayed).count(),
            relayed: per_peer.values().filter(|l| l.relayed).count(),
            worst_rtt: per_peer.values().map(|l| l.rtt).max(),
            per_peer,
        }
    }

    #[test]
    #[ignore = "generates assets/demo.mp4 and assets/demo.gif using rsvg-convert and ffmpeg"]
    fn demo_video() {
        let has_rsvg = std::process::Command::new("which")
            .arg("rsvg-convert")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        let has_ffmpeg = std::process::Command::new("which")
            .arg("ffmpeg")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);

        if !has_rsvg || !has_ffmpeg {
            eprintln!("rsvg-convert or ffmpeg is not available; skipping demo_video");
            return;
        }

        let temp_dir = std::env::temp_dir().join("tincan_demo_render");
        let _ = std::fs::remove_dir_all(&temp_dir);
        std::fs::create_dir_all(&temp_dir).unwrap();

        let me = PeerId([1; 32]);
        let bob = PeerId([2; 32]);

        // Start with clean room and cans diagram clearly visible
        let mut app = App::new(
            me,
            "n73w-kuqc-uog2-4mfx-a7bp-9dlt-2ksv-wq3e-hj5n-x8cr-vy6a-2ptm-4z".into(),
        );
        app.apply(Event::Welcome {
            me,
            room: RoomSnapshot {
                room_name: "lobby".into(),
                channels: vec!["general".into(), "gaming".into(), "music".into()],
                peers: vec![
                    peer(1, "alice", Some(ChannelId(0))),
                    peer(2, "bob", Some(ChannelId(0))),
                    peer(3, "cem", Some(ChannelId(1))),
                    peer(4, "deniz", Some(ChannelId(0))),
                ],
                recent_chat: vec![],
            },
        });
        app.voice = Some(ChannelId(0));
        app.voice_available = true;
        app.motion = true;
        let healthy = [(2, false, 18), (4, false, 26)];
        // After the tour, deniz moves to hotel wifi and drops to a relay: the header
        // names him, his row says so, and the far can becomes him. He is direct again
        // before the camera pulls out, so the loop ends on a healthy line.
        let relayed = [(2, false, 18), (4, true, 140)];
        app.take_link(mesh_reading(&healthy));
        app.active_input_name = Some("MacBook Pro Microphone".into());
        app.active_output_name = Some("AirPods Pro".into());
        app.input_gate = 0.23;
        app.typing_volume = 0.4;
        app.peer_gains.insert(PeerId([3; 32]), 0.0);

        const TOTAL_FRAMES: usize = 360;
        const OVERVIEW: Camera = Camera {
            x: 640.0,
            y: 360.0,
            zoom: 1.0,
        };
        const VOICE_FOCUS: Camera = Camera {
            x: 570.0,
            y: 350.0,
            zoom: 1.35,
        };
        const CHAT_FOCUS: Camera = Camera {
            x: 670.0,
            y: 400.0,
            zoom: 1.35,
        };
        const SETTINGS_FOCUS: Camera = Camera {
            x: 640.0,
            y: 320.0,
            zoom: 1.30,
        };

        let prompt_text = "loud and clear! tincan is fast";

        println!("Rendering {TOTAL_FRAMES} SVG frames...");
        for f in 0..TOTAL_FRAMES {
            let cam = if f < 35 {
                OVERVIEW
            } else if f < 55 {
                OVERVIEW.lerp(&VOICE_FOCUS, (f - 35) as f32 / 20.0)
            } else if f < 85 {
                VOICE_FOCUS
            } else if f < 105 {
                VOICE_FOCUS.lerp(&CHAT_FOCUS, (f - 85) as f32 / 20.0)
            } else if f < 165 {
                CHAT_FOCUS
            } else if f < 185 {
                CHAT_FOCUS.lerp(&SETTINGS_FOCUS, (f - 165) as f32 / 20.0)
            } else if f < 215 {
                SETTINGS_FOCUS
            } else if f < 235 {
                SETTINGS_FOCUS.lerp(&OVERVIEW, (f - 215) as f32 / 20.0)
            } else if f < 245 {
                OVERVIEW
            } else if f < 265 {
                OVERVIEW.lerp(&VOICE_FOCUS, (f - 245) as f32 / 20.0)
            } else if f < 330 {
                VOICE_FOCUS
            } else if f < 350 {
                VOICE_FOCUS.lerp(&OVERVIEW, (f - 330) as f32 / 20.0)
            } else {
                OVERVIEW
            };

            let now = std::time::Instant::now();
            let sim_ms = f as u64 * 50;
            app.started = now - std::time::Duration::from_millis(sim_ms);
            app.take_link(mesh_reading(if (275..310).contains(&f) {
                &relayed
            } else {
                &healthy
            }));

            // Voice & Pulse simulation (Bob speaks)
            if (35..85).contains(&f) {
                app.speaking.insert(bob);
                let level = match (f / 3) % 4 {
                    0 => 2,
                    1 => 4,
                    2 => 3,
                    _ => 5,
                };
                app.peer_levels.insert(bob, level);
            } else {
                app.speaking.clear();
                app.peer_levels.clear();
            }

            if f == 270 {
                app.apply(Event::Chat(ChatLine {
                    channel: ChannelId(0),
                    from: PeerId([4; 32]),
                    text: "on hotel wifi now, still hear me?".into(),
                    at: 1_757_000_160,
                }));
            }

            if f == 65 {
                app.apply(Event::Chat(ChatLine {
                    channel: ChannelId(0),
                    from: bob,
                    text: "hey, can you hear me alright?".into(),
                    at: 1_757_000_100,
                }));
            }

            // Typing simulation (Alice types)
            if (110..155).contains(&f) {
                let count = ((f - 110) * prompt_text.len() / 40).min(prompt_text.len());
                app.input = prompt_text[..count].to_string();
            } else if (155..165).contains(&f) {
                app.input = prompt_text.to_string();
            } else if f == 165 {
                app.apply(Event::Chat(ChatLine {
                    channel: ChannelId(0),
                    from: me,
                    text: prompt_text.into(),
                    at: 1_757_000_105,
                }));
                app.input.clear();
            }

            // F6 Settings simulation
            if (180..220).contains(&f) {
                app.view_mode = ViewMode::Settings;
                app.settings_section = if f < 195 {
                    SettingsSection::InputDevice
                } else {
                    SettingsSection::Typing
                };
                app.input_devices = vec![
                    device("MacBook Pro Microphone", 48_000, true),
                    device("AirPods Pro", 48_000, false),
                ];
                app.output_devices = vec![
                    device("MacBook Pro Speakers", 48_000, true),
                    device("AirPods Pro", 48_000, false),
                ];
                let trans_ms = (f - 180) as u64 * 50;
                app.mode_transition_at = Some(now - std::time::Duration::from_millis(trans_ms));
            } else if f >= 220 {
                app.view_mode = ViewMode::Chat;
                let trans_ms = (f - 220) as u64 * 50;
                app.mode_transition_at = Some(now - std::time::Duration::from_millis(trans_ms));
            }

            let svg_content = studio_svg(&app, 84, 22, &cam);
            let frame_svg = temp_dir.join(format!("frame_{f:04}.svg"));
            std::fs::write(&frame_svg, svg_content).unwrap();
        }

        println!("Rasterizing SVG frames to PNG in parallel...");
        let num_threads = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4);
        let chunk_size = TOTAL_FRAMES.div_ceil(num_threads);

        std::thread::scope(|s| {
            for thread_id in 0..num_threads {
                let start = thread_id * chunk_size;
                let end = (start + chunk_size).min(TOTAL_FRAMES);
                let dir = temp_dir.clone();
                s.spawn(move || {
                    for i in start..end {
                        let svg_path = dir.join(format!("frame_{i:04}.svg"));
                        let png_path = dir.join(format!("frame_{i:04}.png"));
                        let _ = std::process::Command::new("rsvg-convert")
                            .args(["-w", "1280", "-h", "720", "-a", "-f", "png", "-o"])
                            .arg(&png_path)
                            .arg(&svg_path)
                            .status();
                    }
                });
            }
        });

        let _ = std::fs::create_dir_all("assets");

        println!("Encoding assets/demo.mp4 via ffmpeg...");
        let mp4_status = std::process::Command::new("ffmpeg")
            .args([
                "-y",
                "-framerate",
                "20",
                "-i",
                temp_dir.join("frame_%04d.png").to_str().unwrap(),
                "-c:v",
                "libx264",
                "-pix_fmt",
                "yuv420p",
                "-crf",
                "18",
                "-preset",
                "medium",
                "assets/demo.mp4",
            ])
            .status()
            .expect("ffmpeg failed to encode mp4");
        assert!(mp4_status.success(), "ffmpeg mp4 encoding failed");

        println!("Encoding assets/demo.gif via ffmpeg (2-pass palette)...");
        let gif_status = std::process::Command::new("ffmpeg")
            .args([
                "-y",
                "-framerate", "20",
                "-i", temp_dir.join("frame_%04d.png").to_str().unwrap(),
                "-filter_complex",
                "[0:v] fps=16,scale=880:-1:flags=lanczos,split [a][b];[a] palettegen=max_colors=128:stats_mode=diff [p];[b][p] paletteuse=dither=bayer:bayer_scale=3",
                "assets/demo.gif",
            ])
            .status()
            .expect("ffmpeg failed to encode gif");
        assert!(gif_status.success(), "ffmpeg gif encoding failed");

        let _ = std::fs::remove_dir_all(&temp_dir);
        println!("Successfully generated assets/demo.mp4 and assets/demo.gif!");
    }

    /// The Ratatui showcase recording: the terminal alone, as VHS would capture it.
    ///
    /// No window, no camera, nothing zooming — the showcase asks for calm motion and
    /// the subject filling the frame. What moves is the interface: people join, the
    /// string pulls taut, a voice travels down it, one person's link falls back to a
    /// relay — the string sags and the roster, header and far can all name him — the
    /// audio breaks up and it frays, naming nobody, and the audio screen opens over it.
    /// Written under `target/` because the showcase asks for media to live outside
    /// the repository.
    ///
    ///     cargo test --lib -- --ignored --nocapture showcase_gif
    #[test]
    #[ignore = "generates target/showcase/ using rsvg-convert and ffmpeg"]
    fn showcase_gif() {
        const COLS: u16 = 100;
        const ROWS: u16 = 22;
        const WIDTH: &str = "1000";
        const FPS: u64 = 20;
        const FRAME_MS: u64 = 1000 / FPS;

        // Each scene is long enough to read before the next one starts.
        const JOIN: usize = 50;
        const TALK: usize = 80;
        const BOB_SAYS: usize = 125;
        const TYPE: usize = 160;
        const SEND: usize = 225;
        const RELAY: usize = 245;
        const RELAY_SAYS: usize = 265;
        const FRAY: usize = 320;
        const MEND: usize = 370;
        const SETTINGS: usize = 395;
        const BACK: usize = 470;
        const TOTAL: usize = 500;

        let out_dir = std::path::Path::new("target/showcase");
        let frames = out_dir.join("frames");
        let _ = std::fs::remove_dir_all(&frames);
        std::fs::create_dir_all(&frames).unwrap();

        let theme = Theme::dark_true();
        let me = PeerId([1; 32]);
        let bob = PeerId([2; 32]);
        let deniz = PeerId([4; 32]);
        let healthy = [(2, false, 18), (3, false, 24), (4, false, 31)];
        // Deniz drops to a relay; everyone else stays direct, so the header, the far
        // can and his row in the roster all have to agree on who it is.
        let relayed = [(2, false, 18), (3, false, 24), (4, true, 140)];

        let mut app = App::new(
            me,
            "n73w-kuqc-uog2-4mfx-a7bp-9dlt-2ksv-wq3e-hj5n-x8cr-vy6a-2ptm-4z".into(),
        );
        app.apply(Event::Welcome {
            me,
            room: RoomSnapshot {
                room_name: "lobby".into(),
                channels: vec!["general".into(), "gaming".into(), "music".into()],
                peers: vec![peer(1, "alice", Some(ChannelId(0)))],
                recent_chat: vec![],
            },
        });
        app.voice = Some(ChannelId(0));
        app.voice_available = true;
        app.motion = true;
        app.active_input_name = Some("MacBook Pro Microphone".into());
        app.active_output_name = Some("AirPods Pro".into());
        app.input_gate = 0.23;
        app.typing_volume = 0.4;
        app.input_devices = vec![
            device("MacBook Pro Microphone", 48_000, true),
            device("AirPods Pro", 48_000, false),
        ];
        app.output_devices = vec![
            device("MacBook Pro Speakers", 48_000, true),
            device("AirPods Pro", 48_000, false),
        ];

        let reply = "loud and clear, no server in between";
        let say = |app: &mut App, from: PeerId, text: &str, at: u64| {
            app.apply(Event::Chat(ChatLine {
                channel: ChannelId(0),
                from,
                text: text.into(),
                at: 1_757_000_000 + at,
            }));
        };

        println!("Rendering {TOTAL} frames...");
        for f in 0..TOTAL {
            let now = std::time::Instant::now();
            app.started = now - std::time::Duration::from_millis(f as u64 * FRAME_MS);
            let since = |start: usize| {
                now - std::time::Duration::from_millis((f - start) as u64 * FRAME_MS)
            };

            match f {
                JOIN => {
                    app.apply(Event::Roster(vec![
                        peer(1, "alice", Some(ChannelId(0))),
                        peer(2, "bob", Some(ChannelId(0))),
                        peer(3, "cem", Some(ChannelId(0))),
                        peer(4, "deniz", Some(ChannelId(0))),
                        peer(5, "jack", Some(ChannelId(1))),
                    ]));
                    app.take_link(mesh_reading(&healthy));
                }
                BOB_SAYS => say(&mut app, bob, "hey, can you hear me alright?", 0),
                SEND => {
                    say(&mut app, me, reply, 40);
                    app.input.clear();
                }
                RELAY => app.take_link(mesh_reading(&relayed)),
                RELAY_SAYS => say(
                    &mut app,
                    deniz,
                    "on hotel wifi now, coming through a relay",
                    95,
                ),
                MEND => {
                    app.dropped_at = None;
                    app.take_link(mesh_reading(&healthy));
                }
                SETTINGS => {
                    app.view_mode = ViewMode::Settings;
                    app.settings_section = SettingsSection::InputDevice;
                }
                BACK => app.view_mode = ViewMode::Chat,
                _ => {}
            }

            // Bob talks over the taut string, then deniz over the relay and the fray —
            // the pulse slows down with the round trip.
            let talker = if (TALK..TYPE).contains(&f) {
                Some(bob)
            } else if (RELAY_SAYS..MEND).contains(&f) {
                Some(deniz)
            } else {
                None
            };
            if let Some(talker) = talker {
                app.speaking.clear();
                app.peer_levels.clear();
                app.speaking.insert(talker);
                let level = [2, 4, 3, 5, 3, 1, 4, 2][(f / 3) % 8];
                app.peer_levels.insert(talker, level);
            } else {
                app.speaking.clear();
                app.peer_levels.clear();
            }

            if (TYPE..SEND).contains(&f) {
                let typed = ((f - TYPE) * reply.len() / (SEND - TYPE - 12)).min(reply.len());
                app.input = reply[..typed].to_string();
            }

            // Held fresh for the whole scene; the real app notes each new dropout.
            if (FRAY..MEND).contains(&f) {
                app.dropped_at = Some(now);
            }

            if (SETTINGS..BACK).contains(&f) {
                app.mode_transition_at = Some(since(SETTINGS));
                // A voice with some life in it, riding over the noise floor.
                let t = (f - SETTINGS) as f32 * 0.35;
                app.mic_level = (0.38 + 0.2 * t.sin() + 0.08 * (t * 2.7).sin()).clamp(0.0, 1.0);
            } else if f >= BACK {
                app.mode_transition_at = Some(since(BACK));
            }

            let picture = flat_svg(&app, COLS, ROWS, &theme, 0.0);
            std::fs::write(frames.join(format!("frame_{f:04}.svg")), picture).unwrap();
        }

        println!("Rasterizing...");
        let threads = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4);
        let chunk = TOTAL.div_ceil(threads);
        std::thread::scope(|s| {
            for t in 0..threads {
                let dir = frames.clone();
                s.spawn(move || {
                    for i in (t * chunk)..((t + 1) * chunk).min(TOTAL) {
                        let status = std::process::Command::new("rsvg-convert")
                            .args(["-w", WIDTH, "-a", "-f", "png", "-o"])
                            .arg(dir.join(format!("frame_{i:04}.png")))
                            .arg(dir.join(format!("frame_{i:04}.svg")))
                            .status()
                            .expect("rsvg-convert is required");
                        assert!(status.success(), "rsvg-convert failed on frame {i}");
                    }
                });
            }
        });

        let input = frames.join("frame_%04d.png");
        let input = input.to_str().unwrap();
        let fps = FPS.to_string();
        let run = |args: &[&str]| {
            let status = std::process::Command::new("ffmpeg")
                .args(["-y", "-v", "error"])
                .args(args)
                .status()
                .expect("ffmpeg is required");
            assert!(status.success(), "ffmpeg failed: {args:?}");
        };
        run(&[
            "-framerate",
            &fps,
            "-i",
            input,
            "-filter_complex",
            "split[a][b];[a]palettegen=max_colors=64:stats_mode=full[p];[b][p]paletteuse=dither=none",
            "target/showcase/tincan.gif",
        ]);
        // ffmpeg writes every frame whole, so a mostly still interface comes out at
        // ~15 MB — over GitHub's attachment limit. Pillow folds identical frames into
        // one longer one and writes only what changed, with ffmpeg's palette kept —
        // all but its transparency green, which the half-covered bottom row of pixels
        // would otherwise be matched to.
        let squeeze = std::process::Command::new("python3")
            .args([
                "-c",
                "from PIL import Image, ImageSequence\n\
                 src = Image.open('target/showcase/tincan.gif')\n\
                 colours = src.getpalette()\n\
                 for i in range(0, len(colours), 3):\n    \
                     if colours[i:i + 3] == [0, 255, 0]: colours[i:i + 3] = [20, 16, 12]\n\
                 pal = Image.new('P', (1, 1)); pal.putpalette(colours)\n\
                 out = []\n\
                 for f in ImageSequence.Iterator(src):\n    \
                     p = f.convert('RGB').quantize(palette=pal, dither=Image.Dither.NONE)\n    \
                     p.info.clear(); out.append(p)\n\
                 out[0].save('target/showcase/tincan.gif', save_all=True, append_images=out[1:], duration=50, loop=0)\n",
            ])
            .status();
        if !squeeze.is_ok_and(|s| s.success()) {
            eprintln!("python3 with Pillow is not available; tincan.gif is left unoptimised");
        }
        run(&[
            "-framerate",
            &fps,
            "-i",
            input,
            // x264 wants even sides; the height is whatever the cell grid came to.
            "-vf",
            "pad=ceil(iw/2)*2:ceil(ih/2)*2",
            "-c:v",
            "libx264",
            "-pix_fmt",
            "yuv420p",
            "-crf",
            "18",
            "target/showcase/tincan.mp4",
        ]);
        std::fs::copy(
            frames.join(format!("frame_{:04}.png", BOB_SAYS + 20)),
            "target/showcase/tincan.png",
        )
        .unwrap();
        println!("Wrote target/showcase/tincan.{{gif,mp4,png}}");
    }
}
