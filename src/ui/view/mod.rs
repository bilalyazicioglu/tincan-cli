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
