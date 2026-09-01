//! Choosing a microphone and a speaker, and hearing yourself before you trust them.
//!
//! Same language as the rest of the screen: chips name the sections, the section you
//! are working in wears the lit chip, and nothing is boxed.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::text::{Line as TextLine, Span};
use ratatui::widgets::Paragraph;

use super::{clip, spread};
use crate::audio::device::AudioDeviceInfo;
use crate::ui::state::{App, SettingsSection};
use crate::ui::theme::Theme;

/// How wide the microphone test meter is drawn.
const METER_CELLS: usize = 28;
/// Where the voice detector starts letting audio through, as a share of the meter.
const VAD_MARK: usize = METER_CELLS / 5;
/// The label in front of the loopback switch, so the state after it can be clipped
/// to what is left rather than run off the edge.
const HEAD_ROOM: usize = 25;
/// Above this share of the meter the input is close to clipping.
const HOT: usize = METER_CELLS * 17 / 20;
const WARM: usize = METER_CELLS * 3 / 5;

pub fn draw(frame: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    if area.height == 0 || area.width == 0 {
        return;
    }
    // Each list takes the room its own devices need and no more, so the sections sit
    // together instead of drifting apart down an empty screen.
    let problem = if app.settings_error.is_some() { 2 } else { 0 };
    let rows_for = |count: usize| (count.max(1) as u16).saturating_add(2);
    let [banner, input, output, test, _rest] = Layout::vertical([
        Constraint::Length(problem),
        Constraint::Max(rows_for(app.input_devices.len())),
        Constraint::Max(rows_for(app.output_devices.len())),
        Constraint::Length(5),
        Constraint::Min(0),
    ])
    .areas(area);

    if let Some(message) = &app.settings_error {
        frame.render_widget(
            Paragraph::new(vec![
                TextLine::from(vec![
                    Span::raw(" "),
                    Span::styled(" PROBLEM ", theme.chip_link(crate::ui::theme::Strand::Frayed)),
                    Span::raw(" "),
                    Span::styled(clip(message, area.width as usize, theme), theme.error()),
                ]),
                TextLine::from(""),
            ]),
            banner,
        );
    }

    section(
        frame,
        input,
        theme,
        "MICROPHONE",
        app.settings_section == SettingsSection::InputDevice,
        devices(
            input.width,
            theme,
            &app.input_devices,
            app.selected_input_idx,
            &app.active_input_name,
            app.settings_section == SettingsSection::InputDevice,
            "no microphone found. press r to look again.",
        ),
    );

    section(
        frame,
        output,
        theme,
        "SPEAKER",
        app.settings_section == SettingsSection::OutputDevice,
        devices(
            output.width,
            theme,
            &app.output_devices,
            app.selected_output_idx,
            &app.active_output_name,
            app.settings_section == SettingsSection::OutputDevice,
            "no speaker found. press r to look again.",
        ),
    );

    section(
        frame,
        test,
        theme,
        "HEAR YOURSELF",
        app.settings_section == SettingsSection::MicTest,
        test_rows(test.width, app, theme),
    );
}

fn section(
    frame: &mut Frame,
    area: Rect,
    theme: &Theme,
    label: &str,
    focused: bool,
    rows: Vec<TextLine<'static>>,
) {
    if area.height == 0 {
        return;
    }
    let style = if focused { theme.chip_on() } else { theme.chip() };
    let mut lines = vec![TextLine::from(vec![
        Span::raw(" "),
        Span::styled(format!(" {label} "), style),
    ])];
    lines.extend(rows.into_iter().take(area.height.saturating_sub(1) as usize));
    frame.render_widget(Paragraph::new(lines), area);
}

#[allow(clippy::too_many_arguments)]
fn devices(
    width: u16,
    theme: &Theme,
    list: &[AudioDeviceInfo],
    selected: usize,
    active: &Option<String>,
    focused: bool,
    empty: &'static str,
) -> Vec<TextLine<'static>> {
    if list.is_empty() {
        return vec![TextLine::from(vec![
            Span::raw("  "),
            Span::styled(empty, theme.dim()),
        ])];
    }

    list.iter()
        .enumerate()
        .map(|(index, device)| {
            let highlighted = focused && index == selected;
            let in_use = match active {
                Some(name) => name == &device.name,
                None => device.is_default,
            };

            let name_style = if highlighted {
                theme.accent()
            } else if in_use {
                theme.ok()
            } else if !device.is_supported {
                theme.dim()
            } else {
                theme.text()
            };

            let left = vec![
                Span::raw("  "),
                Span::styled(
                    if highlighted { theme.glyphs.cursor.to_string() } else { " ".into() },
                    theme.accent(),
                ),
                Span::raw(" "),
                Span::styled(
                    if in_use { theme.glyphs.on_air.to_string() } else { " ".into() },
                    theme.ok(),
                ),
                Span::raw(" "),
                Span::styled(clip(&device.name, 34, theme), name_style),
            ];

            let (tag, tag_style) = if !device.is_supported {
                ("unavailable".to_string(), theme.error())
            } else if in_use {
                (format!("in use{}{} kHz", theme.glyphs.dot, device.sample_rate / 1000), theme.dim())
            } else if device.is_default {
                (format!("default{}{} kHz", theme.glyphs.dot, device.sample_rate / 1000), theme.dim())
            } else {
                (format!("{} kHz", device.sample_rate / 1000), theme.dim())
            };
            spread(width, left, vec![Span::styled(tag, tag_style), Span::raw(" ")])
        })
        .collect()
}

/// The loopback test, and what the microphone is picking up while it runs.
fn test_rows(width: u16, app: &App, theme: &Theme) -> Vec<TextLine<'static>> {
    // The label says what the switch does, so the state only has to say which way it
    // is thrown. The meter under it shows the rest.
    let (state, state_style) = if app.mic_test_active {
        ("on", theme.ok())
    } else {
        ("off", theme.dim())
    };
    let room = (width as usize).saturating_sub(HEAD_ROOM);

    vec![
        TextLine::from(vec![
            Span::raw("  "),
            Span::styled("space", theme.accent()),
            Span::raw("  "),
            Span::styled("hear yourself   ", theme.text()),
            Span::styled(clip(state, room, theme), state_style),
        ]),
        TextLine::from(""),
        TextLine::from(meter(app, theme)),
    ]
}

/// A meter with the speech threshold marked on it: below the mark nothing is sent,
/// which is the one thing worth knowing while you talk into it.
fn meter(app: &App, theme: &Theme) -> Vec<Span<'static>> {
    let filled = ((app.mic_level.clamp(0.0, 1.0) * METER_CELLS as f32).round() as usize).min(METER_CELLS);
    let [lit, unlit] = theme.glyphs.bar;

    let mut spans = vec![Span::raw("  ")];
    for cell in 0..METER_CELLS {
        if cell < filled {
            let style = if cell >= HOT {
                theme.error()
            } else if cell >= WARM {
                theme.brass()
            } else {
                theme.ok()
            };
            spans.push(Span::styled(lit.to_string(), style));
        } else if cell == VAD_MARK {
            spans.push(Span::styled("|", theme.text()));
        } else {
            spans.push(Span::styled(unlit.to_string(), theme.dim()));
        }
    }
    spans.push(Span::raw("  "));
    spans.push(if filled > VAD_MARK {
        Span::styled("sending", theme.ok())
    } else {
        Span::styled("too quiet to send", theme.dim())
    });
    spans
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proto::PeerId;

    fn text(line: &TextLine<'_>) -> String {
        line.spans.iter().map(|span| span.content.as_ref()).collect()
    }

    fn device(name: &str, rate: u32, default: bool) -> AudioDeviceInfo {
        AudioDeviceInfo {
            name: name.into(),
            sample_rate: rate,
            channels: 1,
            is_default: default,
            is_supported: rate > 0,
        }
    }

    fn app() -> App {
        let mut app = App::new(PeerId([1; 32]), "code".into());
        app.input_devices = vec![
            device("MacBook Pro Microphone", 48000, true),
            device("AirPods", 16000, false),
            device("Broken Thing", 0, false),
        ];
        app
    }

    #[test]
    fn the_device_in_use_is_named_as_such() {
        let mut app = app();
        app.active_input_name = Some("AirPods".into());
        let rows = devices(60, &Theme::from_env(), &app.input_devices, 0, &app.active_input_name, true, "");
        assert!(text(&rows[1]).contains("in use"), "{}", text(&rows[1]));
        assert!(!text(&rows[0]).contains("in use"), "{}", text(&rows[0]));
    }

    #[test]
    fn a_device_that_reports_nothing_is_called_unavailable() {
        let app = app();
        let rows = devices(60, &Theme::from_env(), &app.input_devices, 0, &None, true, "");
        assert!(text(&rows[2]).contains("unavailable"), "{}", text(&rows[2]));
    }

    #[test]
    fn an_empty_list_says_what_to_do_about_it() {
        let rows = devices(60, &Theme::from_env(), &[], 0, &None, true, "no microphone found. press r to look again.");
        assert!(text(&rows[0]).contains("press r"), "{}", text(&rows[0]));
    }

    #[test]
    fn a_bluetooth_rate_is_offered_rather_than_refused() {
        let app = app();
        let rows = devices(60, &Theme::from_env(), &app.input_devices, 0, &None, true, "");
        assert!(text(&rows[1]).contains("16 kHz"), "resampling handles it: {}", text(&rows[1]));
        assert!(!text(&rows[1]).contains("unavailable"), "{}", text(&rows[1]));
    }

    #[test]
    fn the_meter_says_whether_the_voice_is_getting_through() {
        let mut app = app();
        let theme = Theme::from_env();

        app.mic_level = 0.02;
        let quiet: String = meter(&app, &theme).iter().map(|s| s.content.as_ref()).collect();
        assert!(quiet.contains("too quiet"), "{quiet}");

        app.mic_level = 0.7;
        let loud: String = meter(&app, &theme).iter().map(|s| s.content.as_ref()).collect();
        assert!(loud.contains("sending"), "{loud}");
    }

    #[test]
    fn the_test_names_its_own_state() {
        let mut app = app();
        let theme = Theme::from_env();
        assert!(text(&test_rows(60, &app, &theme)[0]).contains("off"));

        app.mic_test_active = true;
        assert!(text(&test_rows(60, &app, &theme)[0]).contains("on"));
        assert!(test_rows(30, &app, &theme)[0].width() <= 30, "the switch must fit its section");
    }
}
