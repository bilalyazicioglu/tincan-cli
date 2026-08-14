//! Ekran düzeni ve çizim.

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
    let [channels, people] = Layout::vertical([Constraint::Length(3 + app.channels.len() as u16), Constraint::Min(3)])
        .areas(sidebar);
    let [chat, input] = Layout::vertical([Constraint::Min(3), Constraint::Length(3)])
        .areas(main);

    draw_channels(frame, channels, app);
    draw_people(frame, people, app);
    draw_chat(frame, chat, app);
    draw_input(frame, input, app);
    draw_footer(frame, footer, app);
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

            // İki ayrı durum var ve karışmamalı: hangi kanala bakıyorum, hangisinde
            // sesle bağlıyım. Bakılan kanal '>' ile, sesli olunan kanal '🔊' ile.
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
        List::new(items).block(Block::default().borders(Borders::ALL).title(" kanallar ")),
        area,
    );
}

fn draw_people(frame: &mut Frame, area: Rect, app: &App) {
    let items: Vec<ListItem> = app
        .peers
        .iter()
        .map(|peer| {
            // Faz 2'de dolu daire gerçek konuşma göstergesi olacak; şimdilik
            // sadece "ses kanalında mı" bilgisini taşıyor.
            let symbol = match (peer.channel.is_some(), peer.muted) {
                (_, true) => "🔇",
                (true, false) => "●",
                (false, false) => "○",
            };
            let mut spans = vec![Span::raw(format!("{symbol} {}", peer.name))];
            if peer.id == app.me {
                spans.push(Span::styled(" (sen)", Style::default().fg(DIM)));
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

    let title = format!(" kişiler ({}) ", app.peers.len());
    frame.render_widget(
        List::new(items).block(Block::default().borders(Borders::ALL).title(title)),
        area,
    );
}

fn draw_chat(frame: &mut Frame, area: Rect, app: &App) {
    let lines: Vec<TextLine> = app
        .visible_lines()
        .iter()
        .map(|line| match line {
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
        })
        .collect();

    // En yeni mesajlar altta dursun: pencereye sığmayan eski satırlar kırpılır.
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
    let voice = match app.voice {
        Some(channel) if app.muted => format!("🔇 {} (susturuldu)", app.channel_name(channel)),
        Some(channel) => format!("🔊 {}", app.channel_name(channel)),
        None => "sessiz — Ctrl+J ile katıl".to_string(),
    };

    let left = app
        .status
        .clone()
        .unwrap_or_else(|| "Tab kanal · Ctrl+J ses · Ctrl+M sustur · Ctrl+C çık".into());

    let [left_area, right_area] =
        Layout::horizontal([Constraint::Min(20), Constraint::Length(48)]).areas(area);

    frame.render_widget(
        Paragraph::new(TextLine::from(Span::styled(left, Style::default().fg(DIM)))),
        left_area,
    );
    frame.render_widget(
        Paragraph::new(TextLine::from(vec![
            Span::styled(voice, Style::default().fg(ACCENT)),
            Span::styled(
                format!("  kod: {}", short_code(&app.invite_code)),
                Style::default().fg(DIM),
            ),
        ])),
        right_area,
    );
}

/// Unix saniyesini yerel saate çevirir.
fn clock(at: u64) -> String {
    match Local.timestamp_opt(at as i64, 0) {
        chrono::LocalResult::Single(time) => time.format("%H:%M").to_string(),
        _ => "--:--".to_string(),
    }
}

/// Davet kodu tam haliyle 63 karakter; alt bilgide sadece ilk grupları gösterip
/// kullanıcıya kodun var olduğunu hatırlatıyoruz. Tamamı `--show-code` ile alınır.
fn short_code(code: &str) -> String {
    let head: String = code.chars().take(9).collect();
    format!("{head}…")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clock_renders_a_time_or_a_placeholder() {
        assert_eq!(clock(0).len(), 5, "epoch bile biçimlenmeli");
        assert!(clock(1_700_000_000).contains(':'));
    }

    #[test]
    fn short_code_is_truncated() {
        let code = "abcd-efgh-ijkl-mnop";
        assert_eq!(short_code(code), "abcd-efgh…");
    }

    /// Çizim kodu paniklememeli: dar terminaller ve boş oda dahil.
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
