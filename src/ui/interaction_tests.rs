//! Exercise displayed targets and resulting session state together.
use super::*;
use crate::proto::PeerInfo;
use ratatui::{Terminal, backend::TestBackend, layout::Rect};

fn room() -> App {
    let mut app = App::new(PeerId([1; 32]), "test".into());
    app.channels = vec!["general".into(), "gaming".into(), "music".into()];
    app.peers = (1..=4)
        .map(|id| PeerInfo {
            id: PeerId([id; 32]),
            name: format!("person{id}"),
            channel: Some(ChannelId(if id == 4 { 1 } else { 0 })),
            muted: id == 1,
            deafened: id == 1,
            afk: false,
        })
        .collect();
    app.voice = Some(ChannelId(0));
    app.voice_available = true;
    app.muted = true;
    app.deafened = true;
    app.input = "unfinished draft".into();
    app
}

fn screen(app: &App, size: (u16, u16), theme: &Theme) -> (String, view::ClickMap) {
    let mut terminal = Terminal::new(TestBackend::new(size.0, size.1)).unwrap();
    terminal
        .draw(|frame| view::draw(frame, app, theme))
        .unwrap();
    let buffer = terminal.backend().buffer();
    let text = (0..size.1)
        .map(|y| {
            (0..size.0)
                .map(|x| buffer[(x, y)].symbol())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n");
    (
        text,
        view::click_map(Rect::new(0, 0, size.0, size.1), app, theme),
    )
}

/// Pick a position by what the user can read, not a duplicate of layout arithmetic.
fn position(text: &str, label: &str) -> (u16, u16) {
    let preferred = text.lines().enumerate().find_map(|(row, line)| {
        line.find(label).filter(|at| row > 0 && *at < 10).map(|at| {
            (
                ratatui::text::Span::raw(&line[..at]).width() as u16,
                row as u16,
            )
        })
    });
    preferred
        .or_else(|| {
            text.lines().enumerate().find_map(|(row, line)| {
                line.find(label).map(|at| {
                    (
                        ratatui::text::Span::raw(&line[..at]).width() as u16,
                        row as u16,
                    )
                })
            })
        })
        .unwrap_or_else(|| panic!("missing {label}:\n{text}"))
}

#[tokio::test]
async fn clicking_and_keyboard_navigation_have_the_same_channel_and_person_results() {
    let mut mouse = room();
    mouse.unread.extend([ChannelId(1), ChannelId(2)]);
    mouse.mentioned.extend([ChannelId(1), ChannelId(2)]);
    mouse.selected_peer = Some(PeerId([3; 32]));
    let mut keyboard = room();
    keyboard.unread = mouse.unread.clone();
    keyboard.mentioned = mouse.mentioned.clone();
    keyboard.selected_peer = mouse.selected_peer;
    let (commands, _receiver) = mpsc::channel(16);
    let size = (80, 24);
    let (text, map) = screen(&mouse, size, &Theme::austere());
    let (x, y) = position(&text, "gaming");
    assert!(
        handle_click(&mut mouse, x, y, &map, size),
        "{text}\n{x},{y}: {map:?}"
    );
    handle_key(
        &mut keyboard,
        KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE),
        &commands,
        None,
    )
    .await
    .unwrap();
    for app in [&mouse, &keyboard] {
        assert_eq!(app.viewing, ChannelId(1));
        assert_eq!(app.voice, Some(ChannelId(0)));
        assert!(app.muted && app.deafened);
        assert_eq!(app.input, "unfinished draft");
        assert_eq!(app.unread, HashSet::from([ChannelId(2)]));
        assert_eq!(app.mentioned, HashSet::from([ChannelId(2)]));
        assert_eq!(app.selected_peer, None);
    }
    let (text, map) = screen(&mouse, size, &Theme::austere());
    let (x, y) = position(&text, "person2");
    assert!(
        handle_click(&mut mouse, x, y, &map, size),
        "{text}\n{x},{y}: {map:?}"
    );
    handle_key(
        &mut keyboard,
        KeyEvent::new(KeyCode::Down, KeyModifiers::NONE),
        &commands,
        None,
    )
    .await
    .unwrap();
    for app in [&mut mouse, &mut keyboard] {
        handle_key(
            app,
            KeyEvent::new(KeyCode::Left, KeyModifiers::NONE),
            &commands,
            None,
        )
        .await
        .unwrap();
        assert_eq!(app.selected_peer, Some(PeerId([2; 32])));
        assert_eq!(app.gain_of(PeerId([2; 32])), 1.0 - PEER_VOLUME_STEP);
        assert_eq!(app.gain_of(PeerId([3; 32])), 1.0);
        assert_eq!(app.input, "unfinished draft");
    }
    assert_eq!(mouse.peer_gains, keyboard.peer_gains);
}

#[test]
fn stale_clicks_never_transfer_to_another_person_after_resize_or_roster_change() {
    let mut app = room();
    let size = (80, 24);
    let (text, drawn) = screen(&app, size, &Theme::austere());
    let (x, y) = position(&text, "person2");
    assert!(!handle_click(&mut app, x, y, &drawn, (44, 12)));
    assert!(!handle_click(&mut app, x, y, &drawn, (70, 20)));
    app.peers.swap(1, 2);
    assert!(!handle_click(&mut app, x, y, &drawn, size));
    let (text, reordered) = screen(&app, size, &Theme::austere());
    let (x, y) = position(&text, "person2");
    assert!(handle_click(&mut app, x, y, &reordered, size));
    assert_eq!(app.selected_peer, Some(PeerId([2; 32])));
    let remaining = app
        .peers
        .iter()
        .filter(|p| p.id != PeerId([2; 32]))
        .cloned()
        .collect();
    app.apply(Event::Roster(remaining));
    assert_eq!(app.selected_peer, None);
    assert!(!handle_click(&mut app, x, y, &reordered, size));
    app.nudge_peer_volume(-0.5);
    assert!(app.peer_gains.is_empty());
}

#[test]
fn terminal_mouse_events_activate_only_plain_left_presses_and_preserve_the_wheel() {
    let mut app = room();
    app.voice_available = false; // Text-only sessions still have chat and a roster.
    let size = (80, 24);
    let (text, map) = screen(&app, size, &Theme::austere());
    let (column, row) = position(&text, "gaming");
    for kind in [
        MouseEventKind::Down(MouseButton::Right),
        MouseEventKind::Down(MouseButton::Middle),
        MouseEventKind::Up(MouseButton::Left),
        MouseEventKind::Drag(MouseButton::Left),
        MouseEventKind::Moved,
    ] {
        assert!(
            mouse_event(
                MouseEvent {
                    kind,
                    column,
                    row,
                    modifiers: KeyModifiers::NONE
                },
                &map
            )
            .is_none()
        );
    }
    for modifiers in [
        KeyModifiers::SHIFT,
        KeyModifiers::CONTROL,
        KeyModifiers::ALT,
    ] {
        assert!(
            mouse_event(
                MouseEvent {
                    kind: MouseEventKind::Down(MouseButton::Left),
                    column,
                    row,
                    modifiers
                },
                &map
            )
            .is_none()
        );
    }
    assert!(matches!(
        mouse_event(
            MouseEvent {
                kind: MouseEventKind::ScrollUp,
                column,
                row,
                modifiers: KeyModifiers::NONE
            },
            &map
        ),
        Some(UiEvent::ScrollUp)
    ));
    assert!(matches!(
        mouse_event(
            MouseEvent {
                kind: MouseEventKind::ScrollDown,
                column,
                row,
                modifiers: KeyModifiers::NONE
            },
            &map
        ),
        Some(UiEvent::ScrollDown)
    ));
    let Some(UiEvent::Click { column, row, drawn }) = mouse_event(
        MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column,
            row,
            modifiers: KeyModifiers::NONE,
        },
        &map,
    ) else {
        panic!("left press lost")
    };
    assert!(handle_click(&mut app, column, row, &drawn, size));
    assert_eq!(app.viewing, ChannelId(1));
    assert!(!app.voice_available);
}

#[test]
fn long_unicode_names_and_tiny_settings_degrade_without_overflow_or_panics() {
    let mut app = room();
    app.room_name = "長い部屋名".repeat(20);
    app.channels[0] = "長いチャンネル".repeat(20);
    app.peers[1].name = "長い名前e\u{301}".repeat(20);
    for theme in [Theme::austere(), Theme::dark_true()] {
        for size in [
            (0, 0),
            (1, 1),
            (10, 5),
            (44, 12),
            (56, 16),
            (62, 20),
            (70, 20),
            (80, 24),
            (120, 30),
        ] {
            app.view_mode = ViewMode::Chat;
            let (_, map) = screen(&app, size, &theme);
            for (rect, _) in map.targets {
                assert!(rect.right() <= size.0 && rect.bottom() <= size.1);
            }
            app.view_mode = ViewMode::Settings;
            app.settings_error = Some("Device unavailable; choose another device".into());
            for focus in [
                SettingsSection::InputDevice,
                SettingsSection::OutputDevice,
                SettingsSection::MicTest,
                SettingsSection::Typing,
            ] {
                app.settings_section = focus;
                let (_, map) = screen(&app, size, &theme);
                assert!(map.targets.iter().all(|(_, action)| !matches!(
                    action,
                    view::ClickAction::Channel(_) | view::ClickAction::Peer(_)
                )));
            }
        }
    }
}

#[test]
fn only_visible_actionable_rows_have_targets_in_crowded_and_compact_layouts() {
    let mut app = room();
    app.peers.extend((5..=24).map(|id| PeerInfo {
        id: PeerId([id; 32]),
        name: format!("person{id}"),
        channel: None,
        muted: false,
        deafened: false,
        afk: false,
    }));
    for peer in &mut app.peers {
        peer.name = format!("p{}", peer.id.0[0]);
    }
    for size in [
        (44, 12),
        (56, 16),
        (61, 20),
        (62, 20),
        (70, 20),
        (80, 24),
        (120, 30),
        (80, 7),
    ] {
        let (text, map) = screen(&app, size, &Theme::austere());
        for (rect, action) in &map.targets {
            let line = text.lines().nth(rect.y as usize).unwrap();
            let label = match action {
                view::ClickAction::Key(_) => continue,
                view::ClickAction::Channel(id) => app.channel_name(*id),
                view::ClickAction::Peer(id) => {
                    &app.peers.iter().find(|p| p.id == *id).unwrap().name
                }
                _ => panic!("settings action offered in chat"),
            };
            assert!(
                line.contains(label),
                "{size:?}, target {action:?} does not match its displayed row: {line}"
            );
            assert!(
                !line
                    .chars()
                    .take(rect.width as usize)
                    .collect::<String>()
                    .contains("more")
            );
        }
        for label in ["CHANNELS", "ON THE LINE", " you", "+"] {
            if let Some((row, line)) = text
                .lines()
                .enumerate()
                .find(|(row, line)| *row < size.1 as usize - 1 && line.contains(label))
            {
                let x = ratatui::text::Span::raw(&line[..line.find(label).unwrap()]).width() as u16;
                assert!(
                    map.action(x, row as u16).is_none(),
                    "{label} should not select anyone"
                );
            }
        }
        if size.0 < 62 {
            assert!(
                map.targets
                    .iter()
                    .all(|(_, action)| matches!(action, view::ClickAction::Key(_)))
            );
        }
    }
    app.view_mode = ViewMode::Settings;
    assert!(
        screen(&app, (80, 24), &Theme::austere())
            .1
            .targets
            .iter()
            .all(|(_, action)| !matches!(
                action,
                view::ClickAction::Channel(_) | view::ClickAction::Peer(_)
            ))
    );
}

#[tokio::test]
async fn mixed_navigation_resize_and_roster_sequences_preserve_session_state() {
    // All four-event combinations of six user/environment actions. These exercise
    // outcomes across transitions, without prescribing an internal call sequence.
    for sequence in 0..6usize.pow(4) {
        let mut app = room();
        let mut size = (80, 24);
        let (commands, _receiver) = mpsc::channel(16);
        let mut events = sequence;
        let mut expected_gain = 1.0;
        for _ in 0..4 {
            let event = events % 6;
            events /= 6;
            let (text, drawn) = screen(&app, size, &Theme::austere());
            match event {
                0 => {
                    if let Some((row, line)) = text.lines().enumerate().find(|(row, line)| {
                        size.0 >= 62
                            && *row > 0
                            && *row < size.1 as usize - 2
                            && line.find("gaming").is_some_and(|at| at < 10)
                    }) {
                        let x = line.find("gaming").unwrap() as u16;
                        assert!(
                            handle_click(&mut app, x, row as u16, &drawn, size),
                            "{text}\n{x},{row}: {drawn:?}"
                        );
                        assert_eq!(app.viewing, ChannelId(1));
                        assert_eq!(app.selected_peer, None);
                    }
                }
                1 => {
                    if let Some((row, line)) = text.lines().enumerate().find(|(row, line)| {
                        size.0 >= 62
                            && *row > 0
                            && *row < size.1 as usize - 2
                            && line.find("person2").is_some_and(|at| at < 10)
                    }) {
                        let x = line.find("person2").unwrap() as u16;
                        assert!(handle_click(&mut app, x, row as u16, &drawn, size));
                        handle_key(
                            &mut app,
                            KeyEvent::new(KeyCode::Left, KeyModifiers::NONE),
                            &commands,
                            None,
                        )
                        .await
                        .unwrap();
                        expected_gain -= PEER_VOLUME_STEP;
                        assert_eq!(app.selected_peer, Some(PeerId([2; 32])));
                    }
                }
                2 => {
                    let before = app.viewing;
                    handle_key(
                        &mut app,
                        KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE),
                        &commands,
                        None,
                    )
                    .await
                    .unwrap();
                    assert_eq!(app.viewing, ChannelId((before.0 + 1) % 3));
                }
                3 => {
                    size = if size.0 == 80 { (44, 12) } else { (80, 24) };
                    for (rect, _) in &drawn.targets {
                        assert!(!handle_click(&mut app, rect.x, rect.y, &drawn, size));
                    }
                }
                4 => {
                    app.peers.swap(1, 2);
                    for (rect, _) in &drawn.targets {
                        assert!(!handle_click(&mut app, rect.x, rect.y, &drawn, size));
                    }
                }
                _ => {
                    let draft = app.input.clone();
                    app.input = "@per".into();
                    handle_key(
                        &mut app,
                        KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE),
                        &commands,
                        None,
                    )
                    .await
                    .unwrap();
                    assert!(app.input.starts_with("@per"));
                    handle_key(
                        &mut app,
                        KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
                        &commands,
                        None,
                    )
                    .await
                    .unwrap();
                    app.input = draft;
                }
            }
            assert_eq!(app.voice, Some(ChannelId(0)), "sequence {sequence}");
            assert!(app.muted && app.deafened, "sequence {sequence}");
            assert_eq!(app.input, "unfinished draft", "sequence {sequence}");
            assert!((app.gain_of(PeerId([2; 32])) - expected_gain).abs() < 0.00001);
            assert_eq!(app.gain_of(PeerId([3; 32])), 1.0);
            let (after, _) = screen(&app, size, &Theme::austere());
            assert!(!after.is_empty());
        }
    }
}

#[tokio::test]
async fn footer_clicks_use_the_keyboard_actions_and_send_the_expected_commands() {
    for theme in [Theme::austere(), Theme::dark_true()] {
        for (label, expected, quits) in [
            ("tab channel", None, false),
            ("f2 talk", Some(Command::SwitchChannel(None)), false),
            ("f3 mute", Some(Command::SetMuted(false)), false),
            ("f5 deafen", Some(Command::SetDeafened(false)), false),
            ("ctrl+c quit", Some(Command::Quit), true),
        ] {
            let mut app = room();
            let size = (160, 24);
            let (text, map) = screen(&app, size, &theme);
            let footer = text.lines().last().unwrap();
            let at = footer.find(label).unwrap();
            let x = ratatui::text::Span::raw(&footer[..at]).width() as u16;
            let action = click_action(&app, x, 23, &map, size, &theme).unwrap();
            assert_eq!(
                click_action(&app, x + label.len() as u16 - 1, 23, &map, size, &theme),
                Some(action.clone()),
                "whole hint must be clickable"
            );
            let (commands, mut received) = mpsc::channel(16);
            assert_eq!(
                dispatch_click(&mut app, action, &commands, None)
                    .await
                    .unwrap(),
                quits
            );
            assert_eq!(received.try_recv().ok(), expected);
            assert!(received.try_recv().is_err());
            assert_eq!(app.input, "unfinished draft");
            if label == "tab channel" {
                assert_eq!(app.viewing, ChannelId(1));
                assert_eq!(app.voice, Some(ChannelId(0)));
            }
        }
    }
}

#[test]
fn footer_targets_track_visible_hints_status_and_settings_in_both_terminal_modes() {
    for theme in [Theme::austere(), Theme::dark_true()] {
        for size in [(44, 12), (56, 16), (80, 24), (160, 30)] {
            let mut app = room();
            let (text, map) = screen(&app, size, &theme);
            let footer = text.lines().last().unwrap();
            for (rect, action) in map.targets.iter().filter(|(rect, _)| rect.y == size.1 - 1) {
                let view::ClickAction::Key(_) = action else {
                    panic!("footer target must use a keyboard action")
                };
                assert!(rect.right() <= size.0);
                assert!(
                    !footer
                        .chars()
                        .skip(rect.x as usize)
                        .take(rect.width as usize)
                        .all(|c| c == ' ')
                );
            }

            let at = ratatui::text::Span::raw(&footer[..footer.find("f1").unwrap()]).width();
            assert!(
                matches!(map.action(at as u16,size.1-1),Some(view::ClickAction::Key(key)) if key.code==KeyCode::F(1))
            );

            app.status = Some("f3 mute is temporarily covered".into());
            let (_, status) = screen(&app, size, &theme);
            assert!(
                !status
                    .targets
                    .iter()
                    .any(|(rect, action)| rect.y == size.1 - 1 && !matches!(action, view::ClickAction::Key(key) if key.code == KeyCode::F(1))),
                "status text is not a button"
            );
            assert!(
                click_action(&app, 1, size.1 - 1, &map, size, &theme).is_none(),
                "queued footer click must not act on changed hints"
            );
            app.status = None;
            app.view_mode = ViewMode::Settings;
            let (text, settings) = screen(&app, size, &theme);
            let footer = text.lines().last().unwrap();
            let at = footer.find("esc back").unwrap();
            let x = ratatui::text::Span::raw(&footer[..at]).width() as u16;
            assert!(
                matches!(settings.action(x,size.1-1),Some(view::ClickAction::Key(key)) if key.code==KeyCode::Esc)
            );
            assert!(click_action(&app, 1, size.1 - 1, &map, size, &theme).is_none());
        }
        let mut app = room();
        app.selected_peer = Some(PeerId([2; 32]));
        let (text, map) = screen(&app, (160, 24), &theme);
        let footer = text.lines().last().unwrap();
        let pair = theme.plainly("←→");
        let at = footer.find(&pair).unwrap();
        let x = ratatui::text::Span::raw(&footer[..at]).width() as u16;
        let offset = if theme.ascii { 5 } else { 1 };
        assert!(
            matches!(map.action(x,23),Some(view::ClickAction::Key(key)) if key.code==KeyCode::Left)
        );
        assert!(
            matches!(map.action(x+offset,23),Some(view::ClickAction::Key(key)) if key.code==KeyCode::Right)
        );
    }
}

fn settings_room() -> App {
    use crate::audio::device::AudioDeviceInfo;
    let mut app = room();
    app.view_mode = ViewMode::Settings;
    app.input_devices = (0..12)
        .map(|i| AudioDeviceInfo {
            name: format!("mic{i:02}"),
            sample_rate: 48000,
            channels: 1,
            is_default: i == 0,
            is_supported: true,
        })
        .collect();
    app.output_devices = (0..12)
        .map(|i| AudioDeviceInfo {
            name: format!("out{i:02}"),
            sample_rate: 48000,
            channels: 2,
            is_default: i == 0,
            is_supported: true,
        })
        .collect();
    app.active_input_name = Some("mic00".into());
    app.active_output_name = Some("out00".into());
    app
}

#[test]
fn device_clicks_resolve_visible_names_across_focus_resize_and_list_windows() {
    for theme in [Theme::austere(), Theme::dark_true()] {
        for size in [(44, 12), (56, 16), (70, 20), (80, 24), (120, 30)] {
            for section in [SettingsSection::InputDevice, SettingsSection::OutputDevice] {
                let mut app = settings_room();
                app.settings_section = section;
                for selected in [0, 1, 7, 12] {
                    app.selected_input_idx = selected;
                    app.selected_output_idx = selected;
                    let (text, map) = screen(&app, size, &theme);
                    for (rect, action) in map
                        .targets
                        .iter()
                        .filter(|(_, action)| matches!(action, view::ClickAction::Device { .. }))
                    {
                        let view::ClickAction::Device { section, name } = action else {
                            unreachable!()
                        };
                        let row = text.lines().nth(rect.y as usize).unwrap();
                        assert!(
                            row.contains(name.as_deref().unwrap_or("System default")),
                            "{text}"
                        );
                        let resolved =
                            click_action(&app, rect.x, rect.y, &map, size, &theme).unwrap();
                        let mut candidate = settings_room();
                        candidate.settings_section = app.settings_section;
                        candidate.selected_input_idx = app.selected_input_idx;
                        candidate.selected_output_idx = app.selected_output_idx;
                        assert_eq!(
                            prepare_settings_click(&mut candidate, &resolved),
                            Some(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
                        );
                        assert_eq!(candidate.settings_section, *section);
                        let chosen = match section {
                            SettingsSection::InputDevice => candidate.selected_input_device(),
                            SettingsSection::OutputDevice => candidate.selected_output_device(),
                            _ => unreachable!(),
                        };
                        assert_eq!(chosen.map(|device| device.name.as_str()), name.as_deref());
                        assert_eq!(candidate.voice, Some(ChannelId(0)));
                        assert_eq!(candidate.input, "unfinished draft");
                    }
                }
            }
        }
    }
}

#[test]
fn settings_clicks_never_retarget_after_hotplug_or_focus_changes() {
    let theme = Theme::austere();
    let size = (80, 24);
    let mut app = settings_room();
    let (_, drawn) = screen(&app, size, &theme);
    let (rect,action)=drawn.targets.iter().find(|(_,action)|matches!(action,view::ClickAction::Device {name:Some(name),..} if name=="mic00")).unwrap();
    app.input_devices.swap(0, 1);
    assert!(click_action(&app, rect.x, rect.y, &drawn, size, &theme).is_none());
    app.input_devices.remove(1);
    assert!(prepare_settings_click(&mut app, action).is_none());
    assert_eq!(app.active_input_name.as_deref(), Some("mic00"));
    let (_, drawn) = screen(&app, size, &theme);
    app.settings_section = SettingsSection::MicTest;
    for (rect, _) in &drawn.targets {
        assert!(click_action(&app, rect.x, rect.y, &drawn, size, &theme).is_none());
    }
}

#[tokio::test]
async fn settings_clicks_drive_recording_monitoring_calibration_and_gate_results() {
    let mut app = settings_room();
    let theme = Theme::austere();
    let size = (56, 16);
    let (commands, _received) = mpsc::channel(16);
    let (text, map) = screen(&app, size, &theme);
    let (x, y) = position(&text, "INPUT");
    let focus = click_action(&app, x, y, &map, size, &theme).unwrap();
    dispatch_click(&mut app, focus, &commands, None)
        .await
        .unwrap();
    assert_eq!(app.settings_section, SettingsSection::MicTest);
    let (text, map) = screen(&app, size, &theme);
    let (x, y) = position(&text, "play yourself back");
    let recorded = click_action(&app, x, y, &map, size, &theme).unwrap();
    let (mx, my) = position(&text, "m  listen live");
    dispatch_click(&mut app, recorded, &commands, None)
        .await
        .unwrap();
    assert_eq!(app.mic_test, MicTest::Recording);
    assert!(
        click_action(&app, mx, my, &map, size, &theme).is_none(),
        "the old monitor button must disappear during recording"
    );
    let (text, map) = screen(&app, size, &theme);
    let (x, y) = position(&text, "play yourself back");
    let action = click_action(&app, x, y, &map, size, &theme).unwrap();
    dispatch_click(&mut app, action, &commands, None)
        .await
        .unwrap();
    assert_eq!(app.mic_test, MicTest::Off);
    let (text, map) = screen(&app, size, &theme);
    let (x, y) = position(&text, "m  listen live");
    let action = click_action(&app, x, y, &map, size, &theme).unwrap();
    dispatch_click(&mut app, action, &commands, None)
        .await
        .unwrap();
    assert_eq!(app.mic_test, MicTest::Monitoring);
    let (text, map) = screen(&app, size, &theme);
    let (x, y) = position(&text, "listen live");
    let action = click_action(&app, x, y, &map, size, &theme).unwrap();
    dispatch_click(&mut app, action, &commands, None)
        .await
        .unwrap();
    assert_eq!(app.mic_test, MicTest::Off);
    let (text, map) = screen(&app, size, &theme);
    let (x, y) = position(&text, "a  measure the room");
    let action = click_action(&app, x, y, &map, size, &theme).unwrap();
    dispatch_click(&mut app, action, &commands, None)
        .await
        .unwrap();
    assert!(app.calibrating.is_some());
    let (_, map) = screen(&app, size, &theme);
    let (_, gate) = map
        .targets
        .iter()
        .find(|(_, action)| matches!(action, view::ClickAction::Gate { cell: 8, .. }))
        .unwrap();
    let view::ClickAction::Gate { cell, cells } = gate else {
        unreachable!()
    };
    let expected = *cell as f32 / *cells as f32;
    dispatch_click(&mut app, gate.clone(), &commands, None)
        .await
        .unwrap();
    assert!(app.calibrating.is_none());
    assert!((app.input_gate - expected).abs() < 0.00001);
    let (text, map) = screen(&app, size, &theme);
    let (x, y) = position(&text, "clean up the room");
    let action = click_action(&app, x, y, &map, size, &theme).unwrap();
    assert_eq!(
        prepare_settings_click(&mut app, &action),
        Some(KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE))
    );
    assert_eq!(app.settings_section, SettingsSection::MicTest);
}

#[tokio::test]
async fn clicking_unavailable_devices_reports_failure_without_changing_actual_routing() {
    let mut app = settings_room();
    app.voice_available = false;
    app.input_devices[0].is_supported = false;
    let theme = Theme::austere();
    let size = (80, 24);
    let (commands, _received) = mpsc::channel(16);
    let (_, map) = screen(&app, size, &theme);
    let (_,action)=map.targets.iter().find(|(_,action)|matches!(action,view::ClickAction::Device {name:Some(name),..} if name=="mic00")).unwrap();
    dispatch_click(&mut app, action.clone(), &commands, None)
        .await
        .unwrap();
    assert!(
        app.settings_error
            .as_ref()
            .unwrap()
            .contains("cannot be opened")
    );
    let (_, map) = screen(&app, size, &theme);
    let (_, action) = map
        .targets
        .iter()
        .find(|(_, action)| {
            matches!(
                action,
                view::ClickAction::Device {
                    section: SettingsSection::InputDevice,
                    name: None
                }
            )
        })
        .unwrap();
    dispatch_click(&mut app, action.clone(), &commands, None)
        .await
        .unwrap();
    assert!(
        app.settings_error
            .as_ref()
            .unwrap()
            .contains("audio is unavailable")
    );
    assert_eq!(app.active_input_name.as_deref(), Some("mic00"));
    assert_eq!(app.active_output_name.as_deref(), Some("out00"));
    assert!(app.follow_input && app.follow_output);
}
