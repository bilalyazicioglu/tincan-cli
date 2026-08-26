//! Screen layout and drawing.

use chrono::{Local, TimeZone};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line as TextLine, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, Paragraph, Wrap};

use super::state::{App, Line};
use crate::proto::ChannelId;

const ACCENT: Color = Color::Cyan;
const DIM: Color = Color::DarkGray;

pub fn draw(frame: &mut Frame, app: &App) {
    let [body, footer] = Layout::vertical([Constraint::Min(3), Constraint::Length(1)])
        .areas(frame.area());
    let [sidebar, main] = Layout::horizontal([Constraint::Length(26), Constraint::Min(20)])
        .areas(body);
    let [logo_area, channels, people] = Layout::vertical([
        Constraint::Length(8),
        Constraint::Length(3 + app.channels.len() as u16),
        Constraint::Min(3),
    ])
    .areas(sidebar);
    let [chat, input] = Layout::vertical([Constraint::Min(3), Constraint::Length(3)])
        .areas(main);

    draw_logo(frame, logo_area);
    draw_channels(frame, channels, app);
    draw_people(frame, people, app);
    draw_chat(frame, chat, app);
    draw_input(frame, input, app);
    draw_footer(frame, footer, app);
}

fn draw_logo(frame: &mut Frame, area: Rect) {
    if area.height < 4 {
        return;
    }
    let logo_text = vec![
        TextLine::from(Span::styled("       ( o )       ", Style::default().fg(Color::Yellow))),
        TextLine::from(Span::styled("      /=====\\      ", Style::default().fg(ACCENT).add_modifier(Modifier::BOLD))),
        TextLine::from(vec![
            Span::styled("     | ", Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)),
            Span::styled("tincan", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
            Span::styled(" |", Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)),
        ]),
        TextLine::from(vec![
            Span::styled("     |  ", Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)),
            Span::styled("/\\", Style::default().fg(Color::Yellow)),
            Span::styled("   |", Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)),
        ]),
        TextLine::from(Span::styled("      \\=====/", Style::default().fg(ACCENT).add_modifier(Modifier::BOLD))),
        TextLine::from(Span::styled("         S         ", Style::default().fg(Color::Yellow))),
    ];

    frame.render_widget(
        Paragraph::new(logo_text)
            .block(Block::default().borders(Borders::ALL).title(" tincan ")),
        area,
    );
}

fn draw_channels(frame: &mut Frame, area: Rect, app: &App) {
    let items: Vec<ListItem> = app
        .channels
        .iter()
        .enumerate()
        .map(|(index, name)| {
            let id = ChannelId(index as u8);
            let viewing = id == app.viewing;
            let in_voice = app.voice == Some(id);
            let count = app.peers_in(id).len();

            // Two separate states that must not be confused: which channel I am
            // looking at, and which one I am connected to by voice. The viewed channel
            // is marked '>', the voice channel '🔊'.
            let marker = if viewing { ">" } else { " " };
            let voice_mark = if in_voice { "🔊" } else { "  " };

            let style = if viewing {
                Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };
            ListItem::new(TextLine::from(vec![
                Span::styled(format!("{marker} {voice_mark} {name}"), style),
                Span::styled(
                    if count > 0 { format!("  {count}") } else { String::new() },
                    Style::default().fg(DIM),
                ),
            ]))
        })
        .collect();

    frame.render_widget(
        List::new(items).block(Block::default().borders(Borders::ALL).title(" channels ")),
        area,
    );
}

fn draw_people(frame: &mut Frame, area: Rect, app: &App) {
    let items: Vec<ListItem> = app
        .peers
        .iter()
        .map(|peer| {
            let speaking = app.speaking.contains(&peer.id);
            // Deafening implies muting, so it is checked first: there is no point
            // talking to someone with their headphones off, and the user should see it.
            let symbol = match (peer.deafened, peer.muted, peer.channel.is_some(), speaking) {
                (true, _, _, _) => "🎧",
                (_, true, _, _) => "🔇",
                (_, _, true, true) => "●",
                (_, _, true, false) => "○",
                (_, _, false, _) => "·",
            };
            // Whoever is speaking must stand out in the list at a glance.
            let name_style = if speaking {
                Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };
            let mut spans = vec![Span::styled(format!("{symbol} {}", peer.name), name_style)];
            if peer.id == app.me {
                spans.push(Span::styled(" (you)", Style::default().fg(DIM)));
            }
            if let Some(channel) = peer.channel {
                spans.push(Span::styled(
                    format!(" · {}", app.channel_name(channel)),
                    Style::default().fg(DIM),
                ));
            }
            ListItem::new(TextLine::from(spans))
        })
        .collect();

    let title = format!(" people ({}) ", app.peers.len());
    frame.render_widget(
        List::new(items).block(Block::default().borders(Borders::ALL).title(title)),
        area,
    );
}

fn draw_chat(frame: &mut Frame, area: Rect, app: &App) {
    let visible = app.visible_lines();
    let mut lines: Vec<TextLine> = if visible.is_empty() {
        let mut logo = crate::logo::tui_logo_lines();
        logo.push(TextLine::from(""));
        logo.push(TextLine::from(Span::styled(
            format!("  Welcome to #{}! Press F2 to join voice chat.", app.channel_name(app.viewing)),
            Style::default().fg(DIM).add_modifier(Modifier::ITALIC),
        )));
        logo
    } else {
        Vec::new()
    };

    lines.extend(visible.iter().map(|line| match line {
        Line::Chat(chat) => {
            let mine = chat.from == app.me;
            let name_style = if mine {
                Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)
            } else {
                Style::default().add_modifier(Modifier::BOLD)
            };
            TextLine::from(vec![
                Span::styled(format!("{} ", clock(chat.at)), Style::default().fg(DIM)),
                Span::styled(format!("{}: ", app.name_of(chat.from)), name_style),
                Span::raw(chat.text.clone()),
            ])
        }
        Line::Notice { text, at } => TextLine::from(vec![
            Span::styled(format!("{} ", clock(*at)), Style::default().fg(DIM)),
            Span::styled(format!("— {text}"), Style::default().fg(DIM).add_modifier(Modifier::ITALIC)),
        ]),
    }));

    // Newest messages sit at the bottom: older lines that do not fit are clipped.
    let height = area.height.saturating_sub(2) as usize;
    let start = lines.len().saturating_sub(height.max(1));

    let title = format!(" #{} · {} ", app.channel_name(app.viewing), app.room_name);
    frame.render_widget(
        Paragraph::new(lines[start..].to_vec())
            .wrap(Wrap { trim: false })
            .block(Block::default().borders(Borders::ALL).title(title)),
        area,
    );
}

fn draw_input(frame: &mut Frame, area: Rect, app: &App) {
    let prompt = format!("#{} ", app.channel_name(app.viewing));
    let content = TextLine::from(vec![
        Span::styled(prompt.clone(), Style::default().fg(DIM)),
        Span::raw(app.input.clone()),
        Span::styled("▏", Style::default().fg(ACCENT)),
    ]);
    frame.render_widget(
        Paragraph::new(content).block(Block::default().borders(Borders::ALL)),
        area,
    );
}

fn draw_footer(frame: &mut Frame, area: Rect, app: &App) {
    let voice = if !app.voice_available {
        "audio off (text only)".to_string()
    } else if app.deafened {
        "🎧 deafened".to_string()
    } else {
        match app.voice {
            Some(channel) if app.muted => format!("🔇 {}", app.channel_name(channel)),
            Some(channel) if app.ptt_mode => format!(
                "{} {} (F4 to talk)",
                if app.ptt_active { "🎙" } else { "⏸" },
                app.channel_name(channel)
            ),
            Some(channel) => format!("🔊 {}", app.channel_name(channel)),
            None => "silent — F2 to join".to_string(),
        }
    };

    let right = TextLine::from(vec![
        Span::styled(voice, Style::default().fg(ACCENT)),
        Span::styled(
            format!("  F1 code: {}", short_code(&app.invite_code)),
            Style::default().fg(DIM),
        ),
    ]);

    // The right side decides its own width: because emoji occupy two cells, a fixed
    // split used to cut the hints on the left in half.
    let right_width = (right.width() as u16).min(area.width);
    let [left_area, right_area] =
        Layout::horizontal([Constraint::Min(0), Constraint::Length(right_width)]).areas(area);

    let left = app.status.clone().unwrap_or_else(|| link_summary(app));
    let left = fit(&left, left_area.width as usize);

    frame.render_widget(
        Paragraph::new(TextLine::from(Span::styled(left, Style::default().fg(DIM)))),
        left_area,
    );
    frame.render_widget(Paragraph::new(right), right_area);
}

/// Fits text into the given width, shortening the hints step by step if it does not.
///
/// Showing a shorter version beats truncating mid-word: it keeps the information
/// readable in a narrow terminal.
fn fit(text: &str, width: usize) -> String {
    if text.chars().count() <= width {
        return text.to_string();
    }
    for shorter in [
        "Tab · F2 voice · F3 mute · Ctrl+C quit",
        "F2 voice · Ctrl+C quit",
        "Ctrl+C quit",
    ] {
        if shorter.chars().count() <= width {
            return shorter.to_string();
        }
    }
    text.chars().take(width).collect()
}

/// The left half of the footer: shortcut hints while the link is healthy, and the
/// problem itself when there is one. Users only want technical detail when something
/// has gone wrong.
fn link_summary(app: &App) -> String {
    let hints = "Tab channel · F2 voice · F3 mute · F5 deafen · Ctrl+C quit";
    if !app.voice_available || app.link.peers() == 0 {
        return hints.to_string();
    }

    let mut parts = Vec::new();
    if app.link.relayed > 0 {
        // A relay works but adds latency; the user needs to know.
        parts.push(format!("{} relay", app.link.relayed));
    }
    if app.link.direct > 0 {
        parts.push(format!("{} direct", app.link.direct));
    }
    if let Some(rtt) = app.link.worst_rtt {
        parts.push(format!("{}ms", rtt.as_millis()));
    }
    if app.audio_dropouts > 0 {
        let plural = if app.audio_dropouts == 1 { "" } else { "s" };
        parts.push(format!("⚠ {} dropout{plural}", app.audio_dropouts));
    }
    parts.join(" · ")
}

/// Converts Unix seconds to local time.
fn clock(at: u64) -> String {
    match Local.timestamp_opt(at as i64, 0) {
        chrono::LocalResult::Single(time) => time.format("%H:%M").to_string(),
        _ => "--:--".to_string(),
    }
}

/// The invite code is 63 characters in full; the footer shows only its first groups,
/// next to the key that puts the whole thing back on screen.
fn short_code(code: &str) -> String {
    let head: String = code.chars().take(9).collect();
    format!("{head}…")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clock_renders_a_time_or_a_placeholder() {
        assert_eq!(clock(0).len(), 5, "even the epoch must format");
        assert!(clock(1_700_000_000).contains(':'));
    }

    /// While the link is fine, the user should see shortcuts, not technical detail.
    #[test]
    fn link_summary_shows_hints_when_there_is_nothing_to_report() {
        use crate::proto::PeerId;
        let mut app = App::new(PeerId([1; 32]), "code".into());
        assert!(link_summary(&app).contains("F2"));

        app.voice_available = true;
        assert!(
            link_summary(&app).contains("F2"),
            "shortcuts show even with no peers"
        );
    }

    /// Relayed links and audio dropouts must both be reported plainly.
    #[test]
    fn link_summary_surfaces_problems() {
        use crate::proto::PeerId;
        use std::time::Duration;

        let mut app = App::new(PeerId([1; 32]), "code".into());
        app.voice_available = true;
        app.link = crate::net::voice::LinkStatus {
            direct: 1,
            relayed: 2,
            worst_rtt: Some(Duration::from_millis(140)),
        };
        app.audio_dropouts = 3;

        let summary = link_summary(&app);
        assert!(summary.contains("2 relay"), "{summary}");
        assert!(summary.contains("1 direct"), "{summary}");
        assert!(summary.contains("140ms"), "{summary}");
        assert!(summary.contains("dropout"), "{summary}");
    }

    #[test]
    fn short_code_is_truncated() {
        let code = "abcd-efgh-ijkl-mnop";
        assert_eq!(short_code(code), "abcd-efgh…");
    }

    /// The two footer texts must not overlap.
    ///
    /// A fixed-width split used to cut the "Ctrl+C quit" hint in half, because emoji
    /// occupy two cells.
    #[test]
    fn footer_does_not_overlap_at_common_widths() {
        use crate::proto::PeerId;
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        for width in [80, 100, 120] {
            let mut terminal = Terminal::new(TestBackend::new(width, 24)).unwrap();
            let mut app = App::new(PeerId([1; 32]), "abcd-efgh-ijkl".into());
            app.voice_available = true;
            app.channels = vec!["general".into()];
            terminal.draw(|frame| draw(frame, &app)).unwrap();

            let rendered: String = terminal.backend().buffer().content().iter()
                .map(|cell| cell.symbol())
                .collect();
            assert!(
                rendered.contains("Ctrl+C quit"),
                "the quit hint must not be cut off at {width} columns"
            );
        }
    }

    /// The drawing code must not panic: narrow terminals and an empty room included.
    #[test]
    fn draws_without_panicking_at_awkward_sizes() {
        use crate::proto::PeerId;
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        for (width, height) in [(80, 24), (20, 8), (200, 60), (10, 5)] {
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            let app = App::new(PeerId([1; 32]), "abcd-efgh".into());
            terminal.draw(|frame| draw(frame, &app)).unwrap();
        }
    }
}
