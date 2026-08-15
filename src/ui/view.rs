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
            let speaking = app.speaking.contains(&peer.id);
            // Sağırlaştırma susturmayı da kapsar, o yüzden önce o kontrol edilir:
            // kulaklığı kapalı birine konuşmanın anlamı yok, kullanıcı bunu görmeli.
            let symbol = match (peer.deafened, peer.muted, peer.channel.is_some(), speaking) {
                (true, _, _, _) => "🎧",
                (_, true, _, _) => "🔇",
                (_, _, true, true) => "●",
                (_, _, true, false) => "○",
                (_, _, false, _) => "·",
            };
            // Konuşan kişi listede hemen ayırt edilebilmeli.
            let name_style = if speaking {
                Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };
            let mut spans = vec![Span::styled(format!("{symbol} {}", peer.name), name_style)];
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
    let voice = if !app.voice_available {
        "ses kapalı (yalnızca yazışma)".to_string()
    } else if app.deafened {
        "🎧 sağırlaştırıldı".to_string()
    } else {
        match app.voice {
            Some(channel) if app.muted => format!("🔇 {}", app.channel_name(channel)),
            Some(channel) if app.ptt_mode => format!(
                "{} {} (F4 ile konuş)",
                if app.ptt_active { "🎙" } else { "⏸" },
                app.channel_name(channel)
            ),
            Some(channel) => format!("🔊 {}", app.channel_name(channel)),
            None => "sessiz — F2 ile katıl".to_string(),
        }
    };

    let right = TextLine::from(vec![
        Span::styled(voice, Style::default().fg(ACCENT)),
        Span::styled(
            format!("  kod: {}", short_code(&app.invite_code)),
            Style::default().fg(DIM),
        ),
    ]);

    // Sağ taraf kendi genişliğini belirler: emoji'ler iki hücre kapladığı için
    // sabit bir bölme sol taraftaki ipuçlarını ortadan kesiyordu.
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

/// Metni verilen genişliğe sığdırır; sığmıyorsa ipuçlarını kademeli olarak kısaltır.
///
/// Kesip yarım kelime bırakmak yerine daha kısa bir sürüm göstermek, dar terminalde
/// bilginin okunabilir kalmasını sağlar.
fn fit(text: &str, width: usize) -> String {
    if text.chars().count() <= width {
        return text.to_string();
    }
    for shorter in ["Tab · F2 ses · F3 sustur · Ctrl+C çık", "F2 ses · Ctrl+C çık", "Ctrl+C çık"] {
        if shorter.chars().count() <= width {
            return shorter.to_string();
        }
    }
    text.chars().take(width).collect()
}

/// Alt bilgideki sol taraf: bağlantı sağlıklıysa kısayol ipuçları, sorun varsa
/// doğrudan sorunun kendisi. Kullanıcı ancak bir şey ters gittiğinde teknik bilgi ister.
fn link_summary(app: &App) -> String {
    let hints = "Tab kanal · F2 ses · F3 sustur · F5 kulaklık · Ctrl+C çık";
    if !app.voice_available || app.link.peers() == 0 {
        return hints.to_string();
    }

    let mut parts = Vec::new();
    if app.link.relayed > 0 {
        // Relay çalışır ama gecikmelidir; kullanıcının bunu bilmesi gerekir.
        parts.push(format!("{} relay", app.link.relayed));
    }
    if app.link.direct > 0 {
        parts.push(format!("{} doğrudan", app.link.direct));
    }
    if let Some(rtt) = app.link.worst_rtt {
        parts.push(format!("{}ms", rtt.as_millis()));
    }
    if app.audio_dropouts > 0 {
        parts.push(format!("⚠ {} kesinti", app.audio_dropouts));
    }
    parts.join(" · ")
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

    /// Bağlantı sorunsuzken kullanıcıya teknik bilgi değil, kısayollar gösterilmeli.
    #[test]
    fn link_summary_shows_hints_when_there_is_nothing_to_report() {
        use crate::proto::PeerId;
        let mut app = App::new(PeerId([1; 32]), "kod".into());
        assert!(link_summary(&app).contains("F2"));

        app.voice_available = true;
        assert!(
            link_summary(&app).contains("F2"),
            "hiç peer yokken de kısayollar görünmeli"
        );
    }

    /// Relay üzerinden akan bağlantı ve ses kesintileri açıkça bildirilmeli.
    #[test]
    fn link_summary_surfaces_problems() {
        use crate::proto::PeerId;
        use std::time::Duration;

        let mut app = App::new(PeerId([1; 32]), "kod".into());
        app.voice_available = true;
        app.link = crate::net::voice::LinkStatus {
            direct: 1,
            relayed: 2,
            worst_rtt: Some(Duration::from_millis(140)),
        };
        app.audio_dropouts = 3;

        let summary = link_summary(&app);
        assert!(summary.contains("2 relay"), "{summary}");
        assert!(summary.contains("1 doğrudan"), "{summary}");
        assert!(summary.contains("140ms"), "{summary}");
        assert!(summary.contains("kesinti"), "{summary}");
    }

    #[test]
    fn short_code_is_truncated() {
        let code = "abcd-efgh-ijkl-mnop";
        assert_eq!(short_code(code), "abcd-efgh…");
    }

    /// Alt bilgideki iki metin birbirinin üstüne binmemeli.
    ///
    /// Sabit genişlikli bir bölme, emoji'lerin iki hücre kaplaması yüzünden
    /// "Ctrl+C çık" ipucunu ortadan kesiyordu.
    #[test]
    fn footer_does_not_overlap_at_common_widths() {
        use crate::proto::PeerId;
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        for width in [80, 100, 120] {
            let mut terminal = Terminal::new(TestBackend::new(width, 24)).unwrap();
            let mut app = App::new(PeerId([1; 32]), "abcd-efgh-ijkl".into());
            app.voice_available = true;
            app.channels = vec!["genel".into()];
            terminal.draw(|frame| draw(frame, &app)).unwrap();

            let rendered: String = terminal.backend().buffer().content().iter()
                .map(|cell| cell.symbol())
                .collect();
            assert!(
                rendered.contains("Ctrl+C çık"),
                "{width} sütunda çıkış ipucu kesilmemeli"
            );
        }
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
