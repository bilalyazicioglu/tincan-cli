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
use crate::audio::MicTest;
use crate::ui::state::{App, METER_CELLS, SettingsSection};
use crate::ui::theme::Theme;

/// The key and the label in front of a control, so the state after it can be clipped
/// to what is left rather than run off the edge.
const HEAD_ROOM: usize = 30;
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
    let [banner, input, output, test, typing, _rest] = Layout::vertical([
        Constraint::Length(problem),
        Constraint::Max(rows_for(app.input_devices.len())),
        Constraint::Max(rows_for(app.output_devices.len())),
        Constraint::Length(6),
        Constraint::Length(4),
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
        "INPUT",
        app.settings_section == SettingsSection::MicTest,
        test_rows(test.width, app, theme),
    );

    section(
        frame,
        typing,
        theme,
        "TYPING",
        app.settings_section == SettingsSection::Typing,
        typing_rows(typing.width, app, theme),
    );
}

/// The keyboard's own sound. It belongs on this screen and not with the microphone:
/// nobody else ever hears it.
fn typing_rows(width: u16, app: &App, theme: &Theme) -> Vec<TextLine<'static>> {
    let (state, state_style) = if app.typing_clicks {
        ("on", theme.ok())
    } else {
        ("off", theme.dim())
    };
    let loudness = if app.typing_clicks && app.typing_volume > 0.0 {
        format!("{}%", (app.typing_volume * 100.0).round() as u32)
    } else if app.typing_clicks {
        "silent".to_string()
    } else {
        "—".to_string()
    };
    let room = (width as usize).saturating_sub(HEAD_ROOM);

    vec![
        TextLine::from(vec![
            Span::raw("  "),
            Span::styled(format!("{:<5}", "space"), theme.accent()),
            Span::raw("  "),
            Span::styled(format!("{:<21}", "key clicks"), theme.text()),
            Span::styled(clip(state, room, theme), state_style),
        ]),
        TextLine::from(vec![
            Span::raw("  "),
            Span::styled(format!("{:<5}", "←→"), theme.accent()),
            Span::raw("  "),
            Span::styled(format!("{:<21}", "how loud"), theme.text()),
            Span::styled(
                clip(&loudness, room, theme),
                if app.typing_clicks { theme.text() } else { theme.dim() },
            ),
        ]),
    ]
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

/// The noise floor, the loopback test, and what the microphone is picking up.
fn test_rows(width: u16, app: &App, theme: &Theme) -> Vec<TextLine<'static>> {
    // Each label says what its control does, so the state after it only has to say
    // where the control is set. The meter under both shows the rest.
    // The row belongs to whichever control is running, and each stage names itself:
    // "on" would say nothing about which half of the test you are in, and the halves
    // being separate is the whole point.
    let (key, label, state, state_style) = match app.mic_test {
        MicTest::Recording => (
            "space",
            "play yourself back",
            match app.mic_test_left() {
                Some(left) => format!("recording… {}", left.as_secs() + 1),
                None => "recording…".to_string(),
            },
            theme.brass(),
        ),
        MicTest::Playing => ("space", "play yourself back", "playing it back".to_string(), theme.ok()),
        MicTest::Monitoring => ("m", "listen live", "on".to_string(), theme.ok()),
        MicTest::Off if app.fed_back => (
            "space",
            "play yourself back",
            "fed back — use headphones".to_string(),
            theme.error(),
        ),
        MicTest::Off => ("space", "play yourself back", "ready".to_string(), theme.dim()),
    };
    let room = (width as usize).saturating_sub(HEAD_ROOM);

    let (gate, gate_style) = if app.calibrating.is_some() {
        ("listening to the room…".to_string(), theme.brass())
    } else if app.input_gate <= 0.0 {
        ("nothing is ignored".to_string(), theme.dim())
    } else {
        (format!("{}%", (app.input_gate * 100.0).round() as u32), theme.text())
    };

    let floor = vec![
        Span::raw("  "),
        Span::styled("←→", theme.accent()),
        Span::raw("     "),
        Span::styled("ignore quieter than  ", theme.text()),
        Span::styled(clip(&gate, room, theme), gate_style),
    ];
    // The measurement is the faster way to set the floor, so it is offered right
    // beside it — but only when the pane is wide enough to hold both. `spread` drops
    // it rather than letting it run over the edge.
    let offer = match app.calibrating {
        Some(_) => Vec::new(),
        None => vec![
            Span::styled("a", theme.accent()),
            Span::styled("  measure the room ", theme.dim()),
        ],
    };

    vec![
        spread(width, floor, offer),
        spread(
            width,
            vec![
                Span::raw("  "),
                Span::styled(format!("{key:<5}"), theme.accent()),
                Span::raw("  "),
                Span::styled(format!("{label:<21}"), theme.text()),
                Span::styled(clip(&state, room, theme), state_style),
            ],
            match app.mic_test {
                MicTest::Off => vec![
                    Span::styled("m", theme.accent()),
                    Span::styled("  listen live ", theme.dim()),
                ],
                _ => Vec::new(),
            },
        ),
        TextLine::from(""),
        TextLine::from(meter(width, app, theme)),
    ]
}

/// The live level with the noise floor marked on it.
///
/// The mark is drawn from the gate itself rather than from a constant, and it is drawn
/// over the lit cells as well as the dark ones — it is while you are talking that you
/// most want to see how much room you have above the floor.
///
/// The bar shrinks to whatever the pane can hold. The gate keeps stepping by
/// `1 / METER_CELLS` whatever the bar is drawn at, so a narrow terminal loses
/// resolution on screen but never loses the setting.
pub fn meter(width: u16, app: &App, theme: &Theme) -> Vec<Span<'static>> {
    let (verdict, verdict_style) = if app.calibrating.is_some() {
        ("hold still", theme.brass())
    } else if app.gate_open() {
        ("sending", theme.ok())
    } else {
        ("too quiet to send", theme.dim())
    };

    let cells = METER_CELLS.min((width as usize).saturating_sub(verdict.len() + 4));
    if cells == 0 {
        return vec![Span::styled(format!("  {verdict}"), verdict_style)];
    }

    let filled = ((app.mic_level.clamp(0.0, 1.0) * cells as f32).round() as usize).min(cells);
    let [lit, unlit] = theme.glyphs.bar;
    let mark = gate_cell(app.input_gate, cells);

    let mut spans = vec![Span::raw("  ")];
    for cell in 0..cells {
        if Some(cell) == mark {
            spans.push(Span::styled("|", theme.accent()));
        } else if cell < filled {
            let style = if cell * METER_CELLS >= HOT * cells {
                theme.error()
            } else if cell * METER_CELLS >= WARM * cells {
                theme.brass()
            } else {
                theme.ok()
            };
            spans.push(Span::styled(lit.to_string(), style));
        } else {
            spans.push(Span::styled(unlit.to_string(), theme.dim()));
        }
    }
    spans.push(Span::raw("  "));
    spans.push(Span::styled(verdict, verdict_style));
    spans
}

/// Which cell of a `cells`-wide bar the gate sits on, or `None` when it is off the
/// bottom and nothing is being ignored.
pub fn gate_cell(gate: f32, cells: usize) -> Option<usize> {
    if gate <= 0.0 || cells == 0 {
        return None;
    }
    Some(((gate * cells as f32).round() as usize).min(cells - 1))
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

    fn drawn(app: &App) -> String {
        meter(60, app, &Theme::from_env())
            .iter()
            .map(|span| span.content.as_ref())
            .collect()
    }

    #[test]
    fn the_meter_says_whether_the_voice_is_getting_through() {
        let mut app = app();
        app.input_gate = crate::config::DEFAULT_GATE;

        app.mic_level = 0.02;
        assert!(drawn(&app).contains("too quiet"), "{}", drawn(&app));

        app.mic_level = 0.7;
        assert!(drawn(&app).contains("sending"), "{}", drawn(&app));
    }

    #[test]
    fn the_mark_and_the_verdict_cannot_drift_apart() {
        // They used to: the mark was a constant and the verdict compared against that
        // same constant, and neither was the threshold the detector actually used.
        let mut app = app();
        for step in 1..=8 {
            app.input_gate = step as f32 / 10.0;
            let mark = gate_cell(app.input_gate, METER_CELLS).unwrap();

            app.mic_level = app.input_gate - 0.02;
            assert!(!app.gate_open(), "just under the mark at cell {mark} must not send");
            app.mic_level = app.input_gate + 0.02;
            assert!(app.gate_open(), "just over the mark at cell {mark} must send");
        }
    }

    #[test]
    fn moving_the_gate_moves_the_mark() {
        let mut app = app();
        app.input_gate = 0.2;
        let low = drawn(&app).find('|').expect("the mark must be on the meter");

        app.input_gate = 0.6;
        let high = drawn(&app).find('|').expect("the mark must still be there");
        assert!(high > low, "the mark has to follow the setting: {low} then {high}");
    }

    #[test]
    fn the_mark_survives_a_loud_voice() {
        let mut app = app();
        app.input_gate = 0.3;
        app.mic_level = 1.0;
        assert!(
            drawn(&app).contains('|'),
            "the floor matters most when the meter is full: {}",
            drawn(&app)
        );
    }

    #[test]
    fn a_gate_at_the_bottom_has_no_mark_to_draw() {
        let mut app = app();
        app.input_gate = 0.0;
        assert_eq!(gate_cell(0.0, METER_CELLS), None);
        assert!(!drawn(&app).contains('|'));
    }

    #[test]
    fn each_stage_of_the_test_names_itself() {
        let theme = Theme::from_env();
        let mut app = app();

        app.toggle_recorded_test();
        assert!(text(&test_rows(70, &app, &theme)[1]).contains("recording"));

        app.mic_test_until = Some(std::time::Instant::now());
        app.advance_mic_test();
        assert!(text(&test_rows(70, &app, &theme)[1]).contains("playing it back"));
    }

    #[test]
    fn a_cut_off_monitor_says_what_happened_and_what_to_do() {
        let theme = Theme::from_env();
        // Whether the cutoff trips is state's business and tested there; this is
        // about the interface being able to explain it afterwards.
        let mut app = app();
        app.fed_back = true;

        let row = text(&test_rows(90, &app, &theme)[1]);
        assert!(row.contains("fed back"), "{row}");
        assert!(row.contains("headphones"), "it has to say what to do about it: {row}");
        assert!(
            !row.contains('…'),
            "and it must fit rather than be cut off mid-advice: {row}"
        );
    }

    #[test]
    fn live_monitoring_is_offered_only_while_nothing_is_running() {
        let theme = Theme::from_env();
        let mut app = app();
        assert!(text(&test_rows(90, &app, &theme)[1]).contains("listen live"));

        app.toggle_recorded_test();
        assert!(
            !text(&test_rows(90, &app, &theme)[1]).contains("listen live"),
            "one thing at a time"
        );
    }

    #[test]
    fn the_keyboard_section_says_whether_it_is_on_and_how_loud() {
        let theme = Theme::from_env();
        let mut app = app();

        let off = typing_rows(70, &app, &theme);
        assert!(text(&off[0]).contains("off"), "{}", text(&off[0]));
        assert!(
            !text(&off[1]).contains('%'),
            "a volume for a thing that is off is noise: {}",
            text(&off[1])
        );

        app.toggle_typing_clicks();
        app.typing_volume = 0.4;
        let on = typing_rows(70, &app, &theme);
        assert!(text(&on[0]).contains("on"), "{}", text(&on[0]));
        assert!(text(&on[1]).contains("40%"), "{}", text(&on[1]));
    }

    #[test]
    fn the_keyboard_rows_fit_a_narrow_pane() {
        let theme = Theme::from_env();
        let mut app = app();
        app.toggle_typing_clicks();
        for width in [30, 45, 70] {
            for row in typing_rows(width, &app, &theme) {
                assert!(row.width() <= width as usize, "{}", text(&row));
            }
        }
    }

    #[test]
    fn the_floor_row_says_where_it_is_set() {
        let mut app = app();
        app.input_gate = 0.25;
        let row = text(&test_rows(70, &app, &Theme::from_env())[0]);
        assert!(row.contains("25%"), "{row}");
        assert!(row.contains("measure the room"), "{row}");

        app.input_gate = 0.0;
        let off = text(&test_rows(70, &app, &Theme::from_env())[0]);
        assert!(off.contains("nothing is ignored"), "{off}");
    }

    #[test]
    fn a_measurement_in_progress_says_what_it_is_doing() {
        let mut app = app();
        app.start_calibration();
        let row = text(&test_rows(70, &app, &Theme::from_env())[0]);
        assert!(row.contains("listening to the room"), "{row}");
        assert!(drawn(&app).contains("hold still"), "{}", drawn(&app));
    }

    #[test]
    fn the_test_names_its_own_state() {
        let mut app = app();
        let theme = Theme::from_env();
        assert!(text(&test_rows(60, &app, &theme)[1]).contains("ready"));

        app.toggle_monitor();
        let row = text(&test_rows(60, &app, &theme)[1]);
        assert!(row.contains("listen live"), "the row names the control that is running: {row}");
        assert!(row.trim_start().starts_with('m'), "and the key that stops it: {row}");
        for width in [30, 40, 53, 80] {
            for row in test_rows(width, &app, &theme) {
                assert!(
                    row.width() <= width as usize,
                    "a control must fit its section at {width}: {:?}",
                    text(&row)
                );
            }
        }
    }
}
