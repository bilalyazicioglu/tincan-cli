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
    if !app.channels.is_empty() {
        left.push(Span::styled(
            format!("#{}", app.channel_name(app.viewing)),
            theme.accent(),
        ));
    }

    let strand = strand::of(app);
    let mut right = Vec::new();
    if app.recently_dropped() {
        right.push(Span::styled("audio dropping ", theme.error()));
    }
    right.push(Span::styled(
        format!(" {} ", strand::label(app)),
        theme.chip_link(strand),
    ));
    if let Some(rtt) = app.link.worst_rtt {
        right.push(Span::styled(format!(" {}ms", rtt.as_millis()), theme.dim()));
    }
    right.push(Span::raw(" "));

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
        assert!(full.contains("←→"), "the dials are only discoverable from here: {full}");
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
            assert!(footer.contains("f1 code"), "lost the code at {width}: {footer}");
        }
    }

    #[test]
    fn a_narrow_terminal_drops_the_rail_and_keeps_the_talk() {
        let wide = rendered(80, 24, &room());
        assert!(wide.contains("CHANNELS"), "the rail belongs on a normal terminal");

        let narrow = rendered(50, 24, &room());
        assert!(!narrow.contains("CHANNELS"), "the rail must give way:\n{narrow}");
        assert!(narrow.contains("say something"), "the message field must survive:\n{narrow}");
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
        assert_eq!(clip("a very long device name", 8, &theme).chars().count(), 8);
        assert!(clip("a very long device name", 8, &theme).starts_with("a very"));
        assert_eq!(clip("abc", 0, &theme), "");
    }

    #[test]
    fn the_shortcut_line_shortens_instead_of_breaking() {
        let app = room();
        let theme = Theme::from_env();
        assert!(fit(&app, 80, &theme).contains("f3 mute"));
        assert!(fit(&app, 55, &theme).contains("f3 mute"), "the ladder must not skip a rung");
        assert!(fit(&app, 40, &theme).chars().count() <= 40);
        assert!(fit(&app, 12, &theme).chars().count() <= 12);
        assert!(fit(&app, 3, &theme).chars().count() <= 3);
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
        text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
    }

    /// Draws the interface and writes it out as SVG.
    ///
    /// Every run of same-looking cells is one `<text>` pinned to an exact width, so the
    /// columns hold whatever monospace face the reader's browser happens to pick — the
    /// thing a pasted block of terminal text cannot promise.
    fn svg(app: &App, cols: u16, rows: u16) -> String {
        let theme = Theme::from_env();
        let mut terminal = Terminal::new(TestBackend::new(cols, rows)).unwrap();
        terminal.draw(|frame| draw(frame, app, &theme)).unwrap();
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
             <rect width=\"{w:.0}\" height=\"{h:.0}\" rx=\"8\" fill=\"{ground}\"/>\n"
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

            let mut x = 0;
            while x < cols {
                let look = |at: u16| {
                    let cell = &buffer[(at, y)];
                    (hex(cell.fg, &ink), cell.modifier.contains(Modifier::BOLD))
                };
                let (fg, bold) = look(x);
                let start = x;
                let mut run = String::new();
                while x < cols && look(x) == (fg.clone(), bold) {
                    run.push_str(buffer[(x, y)].symbol());
                    x += 1;
                }
                if run.trim().is_empty() {
                    continue;
                }
                let weight = if bold { " font-weight=\"600\"" } else { "" };
                out.push_str(&format!(
                    "<text x=\"{:.1}\" y=\"{:.1}\" fill=\"{fg}\"{weight} \
                     textLength=\"{:.1}\" lengthAdjust=\"spacing\" xml:space=\"preserve\">{}</text>\n",
                    PAD + start as f32 * CELL_W,
                    PAD + y as f32 * CELL_H + CELL_H * 0.74,
                    (x - start) as f32 * CELL_W,
                    escape(&run),
                ));
            }
        }
        out.push_str("</svg>\n");
        out
    }

    fn peer(seed: u8, name: &str, channel: Option<ChannelId>) -> PeerInfo {
        PeerInfo {
            id: PeerId([seed; 32]),
            name: name.into(),
            channel,
            muted: false,
            deafened: false,
        }
    }

    /// The room mid-conversation: three people, one of them turned down.
    fn hero() -> App {
        let me = PeerId([1; 32]);
        let mut app = App::new(me, "n73w-kuqc-uog2-4mfx-a7bp-9dlt-2ksv-wq3e-hj5n-x8cr-vy6a-2ptm-4z".into());
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
            std::fs::write(&path, &picture).unwrap_or_else(|e| panic!("could not write {path}: {e}"));
            println!("{path} — {} bytes", picture.len());
        }
    }
}
