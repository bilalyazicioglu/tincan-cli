//! Screen layout and drawing.

use chrono::{Local, TimeZone};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line as TextLine, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, Paragraph, Wrap};

use super::state::{App, Line, SettingsSection, ViewMode};
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

    draw_logo(frame, logo_area);
    draw_channels(frame, channels, app);
    draw_people(frame, people, app);

    match app.view_mode {
        ViewMode::Chat => {
            let [chat, input] = Layout::vertical([Constraint::Min(3), Constraint::Length(3)])
                .areas(main);
            draw_chat(frame, chat, app);
            draw_input(frame, input, app);
        }
        ViewMode::Settings => {
            draw_settings(frame, main, app);
        }
    }

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
            let viewing = id == app.viewing && app.view_mode == ViewMode::Chat;
            let in_voice = app.voice == Some(id);
            let count = app.peers_in(id).len();

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
            let symbol = match (peer.deafened, peer.muted, peer.channel.is_some(), speaking) {
                (true, _, _, _) => "🎧",
                (_, true, _, _) => "🔇",
                (_, _, true, true) => "●",
                (_, _, true, false) => "○",
                (_, _, false, _) => "·",
            };
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
            format!("  Welcome to #{}! Press F2 to join voice chat. Press F6 for Settings.", app.channel_name(app.viewing)),
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

    let height = area.height.saturating_sub(2) as usize;
    let start = lines.len().saturating_sub(height.max(1));

    let title = format!(" #{} · {} (F6 Settings) ", app.channel_name(app.viewing), app.room_name);
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

fn draw_settings(frame: &mut Frame, area: Rect, app: &App) {
    let has_error = app.settings_error.is_some();
    let constraints = if has_error {
        vec![
            Constraint::Length(3), // Error banner
            Constraint::Min(4),    // Input devices
            Constraint::Min(4),    // Output devices
            Constraint::Length(7), // Mic test
        ]
    } else {
        vec![
            Constraint::Min(4),    // Input devices
            Constraint::Min(4),    // Output devices
            Constraint::Length(7), // Mic test
        ]
    };

    let chunks = Layout::vertical(constraints).split(area);
    let mut offset = 0;

    if let Some(err) = &app.settings_error {
        let err_block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Red))
            .title(" ⚠ Error ");
        let p = Paragraph::new(Span::styled(
            format!("  {err}"),
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
        ))
        .block(err_block);
        frame.render_widget(p, chunks[0]);
        offset = 1;
    }

    let input_area = chunks[offset];
    let output_area = chunks[offset + 1];
    let mic_test_area = chunks[offset + 2];

    // ── Input Devices ───────────────────────────────────────────────────────
    let input_focused = app.settings_section == SettingsSection::InputDevice;
    let input_border_style = if input_focused {
        Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(DIM)
    };

    let input_items: Vec<ListItem> = if app.input_devices.is_empty() {
        vec![ListItem::new("  (No microphones detected. Press 'r' to rescan.)")]
    } else {
        app.input_devices
            .iter()
            .enumerate()
            .map(|(idx, dev)| {
                let is_selected = input_focused && idx == app.selected_input_idx;
                let is_active = match &app.active_input_name {
                    Some(name) => name == &dev.name,
                    None => dev.is_default,
                };
                let pointer = if is_selected { "> " } else { "  " };
                let active_mark = if is_active { "● " } else { "○ " };
                let default_tag = if dev.is_default { " [default]" } else { "" };
                let rate_tag = if dev.is_supported {
                    format!(" [{} Hz, {} ch]", dev.sample_rate, dev.channels)
                } else {
                    format!(" [⚠ {} Hz unsupported]", dev.sample_rate)
                };

                let text_style = if is_selected {
                    Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)
                } else if is_active {
                    Style::default().fg(Color::Green)
                } else if !dev.is_supported {
                    Style::default().fg(Color::DarkGray)
                } else {
                    Style::default()
                };

                ListItem::new(TextLine::from(vec![
                    Span::styled(pointer, Style::default().fg(ACCENT)),
                    Span::styled(active_mark, if is_active { Style::default().fg(Color::Green) } else { Style::default().fg(DIM) }),
                    Span::styled(dev.name.clone(), text_style),
                    Span::styled(rate_tag, if dev.is_supported { Style::default().fg(DIM) } else { Style::default().fg(Color::Red) }),
                    Span::styled(default_tag, Style::default().fg(Color::Yellow)),
                ]))
            })
            .collect()
    };

    let input_block = Block::default()
        .borders(Borders::ALL)
        .border_style(input_border_style)
        .title(" 🎙 Input Device (Microphone) - Press Enter to Select ");
    frame.render_widget(List::new(input_items).block(input_block), input_area);

    // ── Output Devices ──────────────────────────────────────────────────────
    let output_focused = app.settings_section == SettingsSection::OutputDevice;
    let output_border_style = if output_focused {
        Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(DIM)
    };

    let output_items: Vec<ListItem> = if app.output_devices.is_empty() {
        vec![ListItem::new("  (No speakers detected. Press 'r' to rescan.)")]
    } else {
        app.output_devices
            .iter()
            .enumerate()
            .map(|(idx, dev)| {
                let is_selected = output_focused && idx == app.selected_output_idx;
                let is_active = match &app.active_output_name {
                    Some(name) => name == &dev.name,
                    None => dev.is_default,
                };
                let pointer = if is_selected { "> " } else { "  " };
                let active_mark = if is_active { "● " } else { "○ " };
                let default_tag = if dev.is_default { " [default]" } else { "" };
                let rate_tag = if dev.is_supported {
                    format!(" [{} Hz, {} ch]", dev.sample_rate, dev.channels)
                } else {
                    format!(" [⚠ {} Hz unsupported]", dev.sample_rate)
                };

                let text_style = if is_selected {
                    Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)
                } else if is_active {
                    Style::default().fg(Color::Green)
                } else if !dev.is_supported {
                    Style::default().fg(Color::DarkGray)
                } else {
                    Style::default()
                };

                ListItem::new(TextLine::from(vec![
                    Span::styled(pointer, Style::default().fg(ACCENT)),
                    Span::styled(active_mark, if is_active { Style::default().fg(Color::Green) } else { Style::default().fg(DIM) }),
                    Span::styled(dev.name.clone(), text_style),
                    Span::styled(rate_tag, if dev.is_supported { Style::default().fg(DIM) } else { Style::default().fg(Color::Red) }),
                    Span::styled(default_tag, Style::default().fg(Color::Yellow)),
                ]))
            })
            .collect()
    };

    let output_block = Block::default()
        .borders(Borders::ALL)
        .border_style(output_border_style)
        .title(" 🔊 Output Device (Speaker) - Press Enter to Select ");
    frame.render_widget(List::new(output_items).block(output_block), output_area);

    // ── Microphone Test & Level ─────────────────────────────────────────────
    let mic_test_focused = app.settings_section == SettingsSection::MicTest;
    let mic_test_border_style = if mic_test_focused {
        Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(DIM)
    };

    let loopback_span = if app.mic_test_active {
        Span::styled(
            " [Space] Hear Yourself: ▶ ACTIVE (Streaming to output) ",
            Style::default().fg(Color::Black).bg(Color::Green).add_modifier(Modifier::BOLD),
        )
    } else {
        Span::styled(
            " [Space] Hear Yourself: ⏸ OFF (Press Space to test loopback) ",
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        )
    };

    // Render VU Level Meter (width ~ 30 blocks)
    let meter_width = 30;
    let filled = ((app.mic_level.clamp(0.0, 1.0) * meter_width as f32).round() as usize).min(meter_width);
    let mut meter_spans = Vec::new();
    meter_spans.push(Span::styled("  Input Level: [", Style::default().fg(DIM)));

    for i in 0..meter_width {
        if i < filled {
            let color = if i < (meter_width * 6) / 10 {
                Color::Green
            } else if i < (meter_width * 85) / 100 {
                Color::Yellow
            } else {
                Color::Red
            };
            meter_spans.push(Span::styled("█", Style::default().fg(color)));
        } else if i == (meter_width * 2) / 10 {
            // VAD threshold marker
            meter_spans.push(Span::styled("|", Style::default().fg(Color::White).add_modifier(Modifier::BOLD)));
        } else {
            meter_spans.push(Span::styled("░", Style::default().fg(DIM)));
        }
    }
    meter_spans.push(Span::styled("] ", Style::default().fg(DIM)));

    if app.mic_level > 0.15 {
        meter_spans.push(Span::styled("🎙 Speaking ", Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)));
    } else {
        meter_spans.push(Span::styled("· Quiet ", Style::default().fg(DIM)));
    }

    let mic_test_lines = vec![
        TextLine::from(""),
        TextLine::from(vec![Span::raw("  "), loopback_span]),
        TextLine::from(""),
        TextLine::from(meter_spans),
    ];

    let mic_test_block = Block::default()
        .borders(Borders::ALL)
        .border_style(mic_test_border_style)
        .title(" 🎚 Live Microphone Test & VU Meter ");
    frame.render_widget(Paragraph::new(mic_test_lines).block(mic_test_block), mic_test_area);
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

fn fit(text: &str, width: usize) -> String {
    if text.chars().count() <= width {
        return text.to_string();
    }
    for shorter in [
        "Tab · F2 voice · F3 mute · F6 settings · Ctrl+C quit",
        "Tab · F2 voice · F6 settings · Ctrl+C",
        "F6 settings · Ctrl+C quit",
        "Ctrl+C quit",
    ] {
        if shorter.chars().count() <= width {
            return shorter.to_string();
        }
    }
    text.chars().take(width).collect()
}

fn link_summary(app: &App) -> String {
    if app.view_mode == ViewMode::Settings {
        return "Tab section · ↑/↓ navigate · Enter apply · Space test · r rescan · Esc back".to_string();
    }

    let hints = "Tab channel · F2 voice · F3 mute · F6 settings · Ctrl+C quit";
    if !app.voice_available || app.link.peers() == 0 {
        return hints.to_string();
    }

    let mut parts = Vec::new();
    if app.link.relayed > 0 {
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

fn clock(at: u64) -> String {
    match Local.timestamp_opt(at as i64, 0) {
        chrono::LocalResult::Single(time) => time.format("%H:%M").to_string(),
        _ => "--:--".to_string(),
    }
}

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

    #[test]
    fn link_summary_changes_in_settings_mode() {
        use crate::proto::PeerId;
        let mut app = App::new(PeerId([1; 32]), "code".into());
        app.view_mode = ViewMode::Settings;
        assert!(link_summary(&app).contains("Esc back"));
    }

    #[test]
    fn draws_settings_without_panicking() {
        use crate::proto::PeerId;
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
        let mut app = App::new(PeerId([1; 32]), "abcd-efgh".into());
        app.view_mode = ViewMode::Settings;
        app.mic_level = 0.5;
        app.mic_test_active = true;
        terminal.draw(|frame| draw(frame, &app)).unwrap();
    }

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
