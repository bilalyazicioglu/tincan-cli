//! ASCII art logo representation of the crumpled tin-can telephone.

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line as TextLine, Span};

/// Color constants matching the project palette.
const ACCENT: Color = Color::Cyan;
const GOLD: Color = Color::Yellow;

/// Prints the colorful ASCII logo to stdout for CLI startup banners.
pub fn print_banner() {
    let banner = format!(
        "\x1b[38;5;214m       ( o )\x1b[0m\n\
         \x1b[1;36m      /=====\\\x1b[0m\n\
         \x1b[1;36m     |\x1b[0m \x1b[1;33mtincan\x1b[0m \x1b[1;36m|\x1b[0m\n\
         \x1b[1;36m     |  \x1b[38;5;214m/\\\x1b[0m   \x1b[1;36m|\x1b[0m\n\
         \x1b[1;36m     | \x1b[38;5;214m/  \\\x1b[0m  \x1b[1;36m|\x1b[0m\n\
         \x1b[1;36m     | \x1b[38;5;214m\\  /\x1b[0m  \x1b[1;36m|\x1b[0m\n\
         \x1b[1;36m      \\=====/\x1b[0m\n\
         \x1b[38;5;214m         ~\x1b[0m\n\
         \x1b[38;5;214m        S\x1b[0m\n\
         \x1b[38;5;214m         ~\x1b[0m"
    );
    println!("{banner}");
}

/// Returns Ratatui `TextLine` rows rendering the ASCII tin-can logo for TUI screens.
pub fn tui_logo_lines() -> Vec<TextLine<'static>> {
    vec![
        TextLine::from(Span::styled("       ( o )       ", Style::default().fg(GOLD))),
        TextLine::from(Span::styled("      /=====\\      ", Style::default().fg(ACCENT).add_modifier(Modifier::BOLD))),
        TextLine::from(vec![
            Span::styled("     | ", Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)),
            Span::styled("tincan", Style::default().fg(GOLD).add_modifier(Modifier::BOLD)),
            Span::styled(" |", Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)),
        ]),
        TextLine::from(vec![
            Span::styled("     |  ", Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)),
            Span::styled("/\\", Style::default().fg(GOLD)),
            Span::styled("   |", Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)),
        ]),
        TextLine::from(vec![
            Span::styled("     | ", Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)),
            Span::styled("/  \\", Style::default().fg(GOLD)),
            Span::styled("  |", Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)),
        ]),
        TextLine::from(vec![
            Span::styled("     | ", Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)),
            Span::styled("\\  /", Style::default().fg(GOLD)),
            Span::styled("  |", Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)),
        ]),
        TextLine::from(Span::styled("      \\=====/", Style::default().fg(ACCENT).add_modifier(Modifier::BOLD))),
        TextLine::from(Span::styled("         ~         ", Style::default().fg(GOLD))),
        TextLine::from(Span::styled("         S         ", Style::default().fg(GOLD))),
        TextLine::from(Span::styled("         ~         ", Style::default().fg(GOLD))),
    ]
}
