//! The string between the rail and the talk.
//!
//! It is the only divider on screen, and it is not a border: it is drawn from the
//! live link. Taut when everyone is reached directly, sagging when someone is coming
//! through a relay, fraying when audio is dropping, sparse when there is nobody on
//! the other end. While anyone is talking a pulse travels down it, and the pulse
//! moves at the speed of the round trip — a slow pulse is a slow connection.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line as TextLine, Span};
use ratatui::widgets::Paragraph;

use crate::ui::state::App;
use crate::ui::theme::{Strand, Theme};

/// How long the pulse rests on one row over a perfect link.
const PULSE_FLOOR_MS: u128 = 55;
/// Round trip time is slowed down by this much before it is added to the rest.
const RTT_DAMPING: u128 = 3;
/// Beyond this the pulse would crawl; a link this slow is already reported as words.
const RTT_CEILING: u128 = 300;

/// What the string is doing right now.
pub fn of(app: &App) -> Strand {
    if !app.voice_available || app.link.peers() == 0 {
        Strand::Idle
    } else if app.recently_dropped() {
        Strand::Frayed
    } else if app.link.relayed > 0 {
        Strand::Slack
    } else {
        Strand::Taut
    }
}

/// The same reading in words, for the header chip.
pub fn label(app: &App) -> &'static str {
    match of(app) {
        Strand::Idle if !app.voice_available => "TEXT ONLY",
        Strand::Idle => "ALONE",
        Strand::Taut => "DIRECT",
        Strand::Slack => "RELAY",
        Strand::Frayed => "CHOPPY",
    }
}

pub fn draw(frame: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let strand = of(app);
    let (glyph, style) = theme.strand(strand);
    let pulse = pulse_row(app, area.height);

    let lines: Vec<TextLine> = (0..area.height)
        .map(|row| {
            let (symbol, style) = if pulse == Some(row) {
                (theme.glyphs.pulse, theme.accent())
            } else if unbroken(strand, row) {
                (glyph, style)
            } else {
                (' ', style)
            };
            TextLine::from(vec![Span::raw(" "), Span::styled(symbol.to_string(), style)])
        })
        .collect();

    frame.render_widget(Paragraph::new(lines), area);
}

/// A waiting string is drawn sparsely — nobody is pulling on it — and a fraying one
/// has gaps torn in it. Taut and slack strings run whole.
fn unbroken(strand: Strand, row: u16) -> bool {
    match strand {
        Strand::Idle => row.is_multiple_of(3),
        Strand::Frayed => row % 4 != 2,
        Strand::Taut | Strand::Slack => true,
    }
}

/// Which row the pulse is on, or `None` when no voice is travelling.
fn pulse_row(app: &App, height: u16) -> Option<u16> {
    if app.speaking.is_empty() || height == 0 {
        return None;
    }
    if !app.motion {
        // Reduced motion still reports that voice is flowing; it just holds still.
        return Some(height / 2);
    }
    let rtt = app
        .link
        .worst_rtt
        .map(|rtt| rtt.as_millis())
        .unwrap_or(0)
        .min(RTT_CEILING);
    let per_row = PULSE_FLOOR_MS + rtt / RTT_DAMPING;
    let elapsed = app.started.elapsed().as_millis();
    Some(((elapsed / per_row) % u128::from(height)) as u16)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::net::voice::LinkStatus;
    use crate::proto::PeerId;
    use std::time::Duration;

    fn connected(direct: usize, relayed: usize) -> App {
        let mut app = App::new(PeerId([1; 32]), "code".into());
        app.voice_available = true;
        app.link = LinkStatus {
            direct,
            relayed,
            worst_rtt: Some(Duration::from_millis(18)),
        };
        app
    }

    #[test]
    fn a_room_with_no_voice_leaves_the_string_slack_and_says_so() {
        let app = App::new(PeerId([1; 32]), "code".into());
        assert_eq!(of(&app), Strand::Idle);
        assert_eq!(label(&app), "TEXT ONLY");
    }

    #[test]
    fn alone_in_the_room_is_not_the_same_as_having_no_audio() {
        let mut app = App::new(PeerId([1; 32]), "code".into());
        app.voice_available = true;
        assert_eq!(of(&app), Strand::Idle);
        assert_eq!(label(&app), "ALONE");
    }

    #[test]
    fn a_direct_link_pulls_the_string_taut() {
        let app = connected(2, 0);
        assert_eq!(of(&app), Strand::Taut);
        assert_eq!(label(&app), "DIRECT");
    }

    #[test]
    fn one_relayed_peer_is_enough_to_slacken_it() {
        let app = connected(3, 1);
        assert_eq!(of(&app), Strand::Slack);
        assert_eq!(label(&app), "RELAY");
    }

    #[test]
    fn dropouts_outrank_everything_else() {
        let mut app = connected(2, 1);
        app.note_dropouts(4);
        assert_eq!(of(&app), Strand::Frayed, "a fraying string is the more urgent news");
        assert_eq!(label(&app), "CHOPPY");
    }

    #[test]
    fn a_fraying_string_actually_has_gaps() {
        let rows: Vec<bool> = (0..8).map(|row| unbroken(Strand::Frayed, row)).collect();
        assert!(rows.contains(&false), "fraying must be visible, not just coloured");
        assert!(rows.contains(&true), "the string must not vanish either");
        assert!((0..8).all(|row| unbroken(Strand::Taut, row)), "a taut string is whole");
    }

    #[test]
    fn nothing_travels_while_nobody_talks() {
        let app = connected(1, 0);
        assert_eq!(pulse_row(&app, 20), None);
    }

    #[test]
    fn a_talking_room_puts_a_pulse_on_the_string() {
        let mut app = connected(1, 0);
        app.speaking.insert(PeerId([2; 32]));
        let row = pulse_row(&app, 20).expect("a talking room must show the voice moving");
        assert!(row < 20, "the pulse must stay on the string");
    }

    #[test]
    fn reduced_motion_still_reports_the_voice() {
        let mut app = connected(1, 0);
        app.speaking.insert(PeerId([2; 32]));
        app.motion = false;
        assert_eq!(pulse_row(&app, 20), Some(10), "it stops moving, it does not disappear");
    }

    #[test]
    fn a_slower_link_moves_the_pulse_more_slowly() {
        let mut fast = connected(1, 0);
        fast.speaking.insert(PeerId([2; 32]));
        let mut slow = fast;
        slow.link.worst_rtt = Some(Duration::from_millis(280));

        let quick_step = PULSE_FLOOR_MS;
        let slow_step = PULSE_FLOOR_MS + 280 / RTT_DAMPING;
        assert!(slow_step > quick_step, "latency has to be visible in the travel");
    }
}
