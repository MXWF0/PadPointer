use crate::{
    config::{self, Config, GameProfile, MatchRule},
    gamepad::{GamepadManager, GamepadProvider, Pad},
    input::{
        Control, Event, InputInjector, MouseButton, MouseEngine, MouseSettings, Tracker, radial,
    },
    mapping::{Action, Behavior, ButtonGestureEngine, Gesture, MappingEngine, MappingProfile},
    mode::{Detection, GameDetector, Mode, ModeManager, Policy},
};
use std::{
    cell::{Cell, RefCell},
    path::{Path, PathBuf},
    rc::Rc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

#[derive(Clone, Default)]
struct MockInputInjector {
    events: Rc<RefCell<Vec<Event>>>,
    limit: Rc<Cell<Option<usize>>>,
}
impl InputInjector for MockInputInjector {
    fn send(&mut self, events: &[Event]) -> usize {
        let count = self.limit.get().unwrap_or(events.len()).min(events.len());
        self.events.borrow_mut().extend_from_slice(&events[..count]);
        count
    }
}
#[derive(Clone, Default)]
struct MockGamepadProvider(Rc<Cell<[Option<Pad>; 4]>>);
impl GamepadProvider for MockGamepadProvider {
    fn state(&mut self, i: u32) -> Option<Pad> {
        self.0.get()[i as usize]
    }
}
fn ms(value: u64) -> Duration {
    Duration::from_millis(value)
}
fn pad(buttons: u16) -> Pad {
    Pad {
        buttons,
        ..Default::default()
    }
}
fn key(vk: u16, down: bool) -> Event {
    Event::Control(Control::Key(vk), down)
}
fn left(down: bool) -> Event {
    Event::Control(Control::Mouse(MouseButton::Left), down)
}

#[test]
fn radial_deadzone_is_circular() {
    assert_eq!(radial([0.1, 0.1], 0.15, 1.6), [0.; 2]);
    assert_ne!(radial([0.12, 0.12], 0.15, 1.6), [0.; 2]);
}
#[test]
fn response_curve_and_diagonal_saturation() {
    let linear = radial([0.5, 0.], 0.15, 1.)[0];
    assert!(radial([0.5, 0.], 0.15, 1.6)[0] < linear);
    let diag = radial([1., 1.], 0.15, 1.6);
    assert!((diag[0].hypot(diag[1]) - 1.).abs() < 1e-12);
}
#[test]
fn small_delta_accumulates_and_neutral_stops() {
    let mock = MockInputInjector::default();
    let mut inject = Tracker::new(mock.clone());
    let mut mouse = MouseEngine::default();
    let settings = MouseSettings {
        speed: 100.,
        smoothing_ms: 0.,
        ..Default::default()
    };
    let low = Pad {
        left: [0.3, 0.],
        ..Default::default()
    };
    mouse.tick(low, &settings, 0.008, 1., &mut inject).unwrap();
    assert!(mock.events.borrow().is_empty());
    for _ in 0..100 {
        mouse.tick(low, &settings, 0.008, 1., &mut inject).unwrap();
    }
    assert!(
        mock.events
            .borrow()
            .iter()
            .any(|event| matches!(event, Event::Move(1, 0)))
    );
    mock.events.borrow_mut().clear();
    mouse
        .tick(Pad::default(), &settings, 0.008, 1., &mut inject)
        .unwrap();
    assert!(mock.events.borrow().is_empty());
}
#[test]
fn time_based_motion_is_independent_of_poll_rate() {
    let displacement = |interval, polls| {
        let mock = MockInputInjector::default();
        let mut inject = Tracker::new(mock.clone());
        let mut mouse = MouseEngine::default();
        let settings = MouseSettings {
            smoothing_ms: 0.,
            ..Default::default()
        };
        let full = Pad {
            left: [1., 0.],
            ..Default::default()
        };
        for _ in 0..polls {
            mouse
                .tick(full, &settings, interval, 1., &mut inject)
                .unwrap();
        }
        mock.events
            .borrow()
            .iter()
            .map(|event| match event {
                Event::Move(x, _) => *x,
                _ => 0,
            })
            .sum::<i32>()
    };
    assert!((displacement(0.004, 100) - displacement(0.008, 50)).abs() <= 1);
}
#[test]
fn default_mouse_press_drag_and_release() {
    let mock = MockInputInjector::default();
    let mut inject = Tracker::new(mock.clone());
    let mut mapping = MappingEngine::default();
    let mut mouse = MouseEngine::default();
    let profile = MappingProfile::default();
    let now = Instant::now();
    mapping
        .tick(pad(1), &profile, now, ms(500), ms(260), &mut inject)
        .unwrap();
    mouse
        .tick(
            Pad {
                left: [1., 0.],
                ..pad(1)
            },
            &MouseSettings::default(),
            0.008,
            1.,
            &mut inject,
        )
        .unwrap();
    mapping
        .tick(pad(0), &profile, now + ms(8), ms(500), ms(260), &mut inject)
        .unwrap();
    let events = mock.events.borrow();
    assert_eq!(events[0], left(true));
    assert!(matches!(events[1],Event::Move(x,0) if x>0));
    assert_eq!(events[2], left(false));
}
#[test]
fn click_waits_if_double_is_bound() {
    let now = Instant::now();
    let mut gesture = ButtonGestureEngine::default();
    let mut events = Vec::new();
    for (down, at) in [
        (true, 0),
        (false, 20),
        (true, 110),
        (false, 140),
        (false, 450),
    ] {
        gesture.update(down, now + ms(at), Some(ms(500)), Some(ms(260)), |e| {
            events.push(e)
        });
    }
    assert_eq!(
        events,
        [
            Gesture::Pressed,
            Gesture::Released,
            Gesture::Pressed,
            Gesture::Released,
            Gesture::DoubleClick
        ]
    );
}
#[test]
fn single_click_fires_at_deadline() {
    let now = Instant::now();
    let mut gesture = ButtonGestureEngine::default();
    let mut events = Vec::new();
    gesture.update(true, now, None, Some(ms(260)), |e| events.push(e));
    gesture.update(false, now + ms(20), None, Some(ms(260)), |e| events.push(e));
    gesture.update(false, now + ms(279), None, Some(ms(260)), |e| {
        events.push(e)
    });
    assert_eq!(events, [Gesture::Pressed, Gesture::Released]);
    gesture.update(false, now + ms(280), None, Some(ms(260)), |e| {
        events.push(e)
    });
    assert_eq!(events.last(), Some(&Gesture::Click));
}
#[test]
fn long_press_holds_and_suppresses_click() {
    let now = Instant::now();
    let mut gesture = ButtonGestureEngine::default();
    let mut events = Vec::new();
    for (down, at) in [
        (true, 0),
        (true, 499),
        (true, 500),
        (true, 700),
        (false, 720),
    ] {
        gesture.update(down, now + ms(at), Some(ms(500)), None, |e| events.push(e));
    }
    assert_eq!(
        events,
        [
            Gesture::Pressed,
            Gesture::LongPress,
            Gesture::Hold(true),
            Gesture::Released,
            Gesture::Hold(false)
        ]
    );
}
#[test]
fn custom_chord_releases_in_reverse_order() {
    let mock = MockInputInjector::default();
    let mut inject = Tracker::new(mock.clone());
    let mut profile = MappingProfile::default();
    profile.buttons[0].mode = Behavior::Press;
    profile.buttons[0].press = Action::Keys {
        keys: crate::keys::parse("Ctrl + Shift + Esc").unwrap(),
    };
    MappingEngine::default()
        .tick(
            pad(1),
            &profile,
            Instant::now(),
            ms(500),
            ms(260),
            &mut inject,
        )
        .unwrap();
    assert_eq!(
        *mock.events.borrow(),
        [
            key(17, true),
            key(16, true),
            key(27, true),
            key(27, false),
            key(16, false),
            key(17, false)
        ]
    );
}
#[test]
fn native_double_click_is_two_complete_clicks() {
    let mock = MockInputInjector::default();
    Tracker::new(mock.clone())
        .double_click(MouseButton::Left)
        .unwrap();
    assert_eq!(
        *mock.events.borrow(),
        [left(true), left(false), left(true), left(false)]
    );
}
#[test]
fn partial_send_only_tracks_accepted_keys_and_retries() {
    let mock = MockInputInjector::default();
    let mut inject = Tracker::new(mock.clone());
    mock.limit.set(Some(2));
    assert!(
        inject
            .chord(&[Control::Key(17), Control::Key(16), Control::Key(27)])
            .is_err()
    );
    mock.limit.set(Some(0));
    assert!(inject.release_all().is_err());
    mock.limit.set(None);
    inject.release_all().unwrap();
    assert_eq!(
        *mock.events.borrow(),
        [key(17, true), key(16, true), key(16, false), key(17, false)]
    );
}
#[test]
fn shared_hold_is_released_once() {
    let mock = MockInputInjector::default();
    let mut inject = Tracker::new(mock.clone());
    let control = Control::Mouse(MouseButton::Left);
    inject.hold(control, true).unwrap();
    inject.hold(control, true).unwrap();
    inject.hold(control, false).unwrap();
    assert_eq!(*mock.events.borrow(), [left(true)]);
    inject.release_all().unwrap();
    assert_eq!(*mock.events.borrow(), [left(true), left(false)]);
}
#[test]
fn switch_releases_drag_and_blocks_held_button_until_lifted() {
    let mock = MockInputInjector::default();
    let mut inject = Tracker::new(mock.clone());
    let mut m = MappingEngine::default();
    let p = MappingProfile::default();
    let now = Instant::now();
    m.tick(pad(1), &p, now, ms(500), ms(260), &mut inject)
        .unwrap();
    inject.release_all().unwrap();
    m.reset(1);
    for (buttons, dt) in [(1, 160), (0, 168), (1, 176)] {
        m.tick(
            pad(buttons),
            &p,
            now + ms(dt),
            ms(500),
            ms(260),
            &mut inject,
        )
        .unwrap();
    }
    assert_eq!(*mock.events.borrow(), [left(true), left(false), left(true)]);
}
#[test]
fn automatic_mode_immediately_enters_game_and_delays_restore() {
    let now = Instant::now();
    let mut m = ModeManager::default();
    assert!(m.update(Detection::Game(0), now));
    assert_eq!(m.mode, Mode::Game);
    assert!(!m.update(Detection::Desktop, now + ms(8)));
    assert!(m.update(Detection::Desktop, now + ms(158)));
    assert_eq!(m.mode, Mode::Desktop);
}
#[test]
fn fast_switch_and_unknown_foreground_stay_safe() {
    let now = Instant::now();
    let mut m = ModeManager::default();
    m.update(Detection::Unknown, now);
    m.update(Detection::Desktop, now + ms(8));
    m.update(Detection::Game(0), now + ms(120));
    m.update(Detection::Desktop, now + ms(148));
    assert!(!m.update(Detection::Desktop, now + ms(158)));
    assert!(m.update(Detection::Desktop, now + ms(298)));
}
#[test]
fn manual_policy_and_pause_override_detection() {
    let now = Instant::now();
    let mut m = ModeManager {
        policy: Policy::Desktop,
        ..Default::default()
    };
    m.update(Detection::Game(0), now);
    assert_eq!(m.mode, Mode::Desktop);
    m.paused = true;
    m.update(Detection::Game(0), now);
    assert_eq!(m.mode, Mode::Paused);
    m.paused = false;
    m.policy = Policy::Game;
    m.update(Detection::Desktop, now);
    assert_eq!(m.mode, Mode::Game);
}
#[test]
fn game_path_selects_exact_match_before_filename() {
    let games = [
        GameProfile {
            name: "Name".into(),
            path: PathBuf::from(r"C:\one\game.exe"),
            enabled: true,
            rule: MatchRule::ExecutableName,
        },
        GameProfile {
            name: "Path".into(),
            path: PathBuf::from(r"C:\two\GAME.exe"),
            enabled: true,
            rule: MatchRule::Path,
        },
    ];
    assert_eq!(
        GameDetector::matches(&games, Path::new(r"c:\two\game.exe")),
        Detection::Game(1)
    );
    assert_eq!(
        GameDetector::matches(&games, Path::new(r"c:\one\game.exe")),
        Detection::Game(0)
    );
    assert_eq!(
        GameDetector::matches(&games, Path::new(r"c:\one\other.exe")),
        Detection::Desktop
    );
}
#[test]
fn chooses_slot_and_detects_disconnect_and_reconnect() {
    let now = Instant::now();
    let mock = MockGamepadProvider::default();
    mock.0.set([None, Some(pad(1)), None, Some(pad(2))]);
    let mut manager = GamepadManager::new(mock.clone(), now);
    assert_eq!(manager.poll(now), Some(pad(1)));
    manager.select(Some(3), now + ms(8));
    assert_eq!(manager.poll(now + ms(8)), Some(pad(2)));
    mock.0.set([None, Some(pad(1)), None, None]);
    assert_eq!(manager.poll(now + ms(16)), None);
    assert_eq!(manager.active, None);
    assert_eq!(manager.poll(now + ms(24)), None);
    mock.0.set([None, Some(pad(1)), None, Some(pad(4))]);
    assert_eq!(manager.poll(now + ms(524)), Some(pad(4)));
}
#[test]
fn config_reads_writes_replaces_and_recovers_corrupt_file() {
    let count = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("PadPointer-test-{}-{count}", std::process::id()));
    std::fs::create_dir(&dir).unwrap();
    let path = dir.join("config.json");
    let (mut conf, _) = config::load(&path).unwrap();
    conf.mouse.speed = 320.;
    config::save(&path, &conf).unwrap();
    assert_eq!(config::read(&path).unwrap().mouse.speed, 320.);
    std::fs::write(&path, b"broken data").unwrap();
    let (reset, notice) = config::load(&path).unwrap();
    assert!(notice.is_some());
    assert_eq!(reset.mouse.speed, Config::default().mouse.speed);
    for entry in std::fs::read_dir(&dir).unwrap() {
        std::fs::remove_file(entry.unwrap().path()).unwrap();
    }
    std::fs::remove_dir(dir).unwrap();
}
#[test]
fn config_migrates_v1_and_rejects_newer_version_and_conflicts() {
    let value = serde_json::json!({"version":1, "bindings":Config::default().mapping.buttons});
    let config = config::decode(&serde_json::to_vec(&value).unwrap()).unwrap();
    assert_eq!(config.version, config::VERSION);
    assert!(config::decode(br#"{"version":999}"#).is_err());
    let mut config = Config::default();
    config.mapping.buttons[0].mode = Behavior::Press;
    config.mapping.buttons[0].press = Action::Keys {
        keys: vec![17, 18, 123],
    };
    assert!(config.validate().is_err());
}
