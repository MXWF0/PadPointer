use crate::{
    config::Config,
    gamepad::{GamepadManager, Pad, XInputProvider},
    input::{MouseEngine, Tracker, WindowsInjector},
    mapping::MappingEngine,
    mode::{Detection, GameDetector, Mode, ModeManager, Policy},
    system::HotkeyRegistration,
};
use std::{
    sync::{
        Arc, Mutex,
        mpsc::{self, Receiver, SyncSender},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

pub enum Command {
    Configure(Box<Config>),
    Pause(bool),
    Policy(Policy),
    Stop,
}
#[derive(Clone, Debug, Default)]
pub struct Status {
    pub slots: [bool; 4],
    pub active: Option<u32>,
    pub pad: Option<Pad>,
    pub mode: Mode,
    pub policy: Policy,
    pub paused: bool,
    pub detection: Detection,
    pub input_error: Option<String>,
    pub hotkey_error: Option<String>,
}
pub struct Worker {
    pub tx: SyncSender<Command>,
    pub status: Arc<Mutex<Status>>,
    thread: JoinHandle<()>,
}
impl Worker {
    pub fn spawn(config: Config) -> std::io::Result<Self> {
        let (tx, rx) = mpsc::sync_channel(24);
        let status = Arc::new(Mutex::new(Status::default()));
        let output = Arc::clone(&status);
        let thread = thread::Builder::new()
            .name("padpointer-input".into())
            .spawn(move || run(config, &rx, &output))?;
        Ok(Self { tx, status, thread })
    }
    pub fn stop(self) -> anyhow::Result<()> {
        if let Err(error) = self.tx.send(Command::Stop) {
            tracing::error!(%error, "input thread already stopped");
        }
        self.thread
            .join()
            .map_err(|_| anyhow::anyhow!("输入线程异常退出"))
    }
}
fn run(mut config: Config, rx: &Receiver<Command>, output: &Mutex<Status>) {
    let now = Instant::now();
    let mut gamepad = GamepadManager::new(XInputProvider, now);
    gamepad.select(config.gamepad.slot, now);
    let mut tracker = Tracker::new(WindowsInjector);
    let mut mapping = MappingEngine::default();
    let mut mouse = MouseEngine::default();
    let mut detector = GameDetector::default();
    let mut mode = ModeManager {
        policy: if config.general.automatic {
            Policy::Auto
        } else {
            Policy::Desktop
        },
        ..Default::default()
    };
    let mut hotkey = HotkeyRegistration::new();
    let mut hotkey_error = hotkey
        .set(config.general.hotkey)
        .err()
        .map(|e| format!("{e:#}"));
    let mut input_error = None;
    let mut release_pending = false;
    let mut retry_release = now;
    let mut prev_slot = None;
    let mut prev_tick = now;
    let mut publish = now;
    let mut command = None;
    let mut stop = false;
    let mut sticks_blocked = [false; 2];
    loop {
        let now = Instant::now();
        let elapsed = now - prev_tick;
        prev_tick = now;
        let mut boundary = elapsed > Duration::from_millis(250);
        while let Some(action) = command.take().or_else(|| rx.try_recv().ok()) {
            match action {
                Command::Configure(new) => {
                    if new.general.hotkey != config.general.hotkey || hotkey_error.is_some() {
                        hotkey_error = hotkey
                            .set(new.general.hotkey)
                            .err()
                            .map(|e| format!("{e:#}"));
                    }
                    if new.gamepad.slot != config.gamepad.slot {
                        gamepad.select(new.gamepad.slot, now);
                        boundary = true;
                    }
                    if new.general.automatic != config.general.automatic {
                        tracing::info!(automatic = new.general.automatic, "startup policy updated");
                    }
                    boundary |= new.mapping != config.mapping
                        || new.gamepad.long_press_ms != config.gamepad.long_press_ms
                        || new.gamepad.double_click_ms != config.gamepad.double_click_ms;
                    if new.games != config.games {
                        detector = GameDetector::default();
                    }
                    config = *new;
                }
                Command::Pause(value) => mode.paused = value,
                Command::Policy(value) => mode.policy = value,
                Command::Stop => {
                    stop = true;
                    break;
                }
            }
        }
        if stop {
            break;
        }
        if hotkey.toggled() {
            mode.paused = !mode.paused;
        }
        let detection = detector.detect(&config.games, now);
        if mode.update(detection, now) {
            tracing::info!(mode = ?mode.mode, policy = ?mode.policy, ?detection, "mapping mode changed");
            boundary = true;
        }
        let pad = gamepad.poll(now);
        boundary |= gamepad.active != prev_slot;
        prev_slot = gamepad.active;
        if boundary {
            let state = pad.unwrap_or_default();
            mapping.reset(state.buttons);
            sticks_blocked =
                [state.left, state.right].map(|s| s[0].hypot(s[1]) > config.mouse.deadzone);
            mouse = MouseEngine::default();
            release_pending = true;
            retry_release = now;
        }
        if release_pending && now >= retry_release {
            match tracker.release_all() {
                Ok(()) => release_pending = false,
                Err(error) => {
                    let text = error.to_string();
                    if input_error.as_ref() != Some(&text) {
                        tracing::warn!(%error);
                    }
                    input_error = Some(text);
                    retry_release = now + Duration::from_millis(300);
                }
            }
        }
        if mode.mode == Mode::Desktop
            && !release_pending
            && !boundary
            && let Some(mut state) = pad
        {
            for (blocked, stick) in sticks_blocked
                .iter_mut()
                .zip([&mut state.left, &mut state.right])
            {
                if *blocked {
                    *blocked = stick[0].hypot(stick[1]) > config.mouse.deadzone;
                    *stick = [0.; 2];
                }
            }
            let result = mapping
                .tick(
                    state,
                    &config.mapping,
                    now,
                    Duration::from_millis(config.gamepad.long_press_ms),
                    Duration::from_millis(config.gamepad.double_click_ms),
                    &mut tracker,
                )
                .and_then(|pause| {
                    if !pause {
                        mouse.tick(
                            state,
                            &config.mouse,
                            elapsed.as_secs_f64().min(0.032),
                            mapping.speed_scale(&config.mapping, &config.mouse),
                            &mut tracker,
                        )?;
                    }
                    Ok(pause)
                });
            match result {
                Ok(true) => {
                    mode.paused = true;
                    mode.update(detection, now);
                    mapping.reset(state.buttons);
                    mouse = MouseEngine::default();
                    if let Err(error) = tracker.release_all() {
                        tracing::warn!(%error);
                        input_error = Some(error.to_string());
                        release_pending = true;
                        retry_release = now + Duration::from_millis(100);
                    }
                }
                Ok(false) => {}
                Err(error) => {
                    let text = error.to_string();
                    if input_error.as_ref() != Some(&text) {
                        tracing::warn!(%error);
                    }
                    input_error = Some(text);
                    mapping.reset(state.buttons);
                    mouse = MouseEngine::default();
                    release_pending = true;
                    retry_release = now + Duration::from_millis(200);
                }
            }
        }
        if now >= publish || boundary {
            publish = now + Duration::from_millis(50);
            *output.lock().expect("input status lock poisoned") = Status {
                slots: gamepad.slots,
                active: gamepad.active,
                pad,
                mode: mode.mode,
                policy: mode.policy,
                paused: mode.paused,
                detection,
                input_error: input_error.clone(),
                hotkey_error: hotkey_error.clone(),
            };
        }
        let interval = if pad.is_none() {
            80
        } else if mode.mode != Mode::Desktop {
            16
        } else {
            config.gamepad.polling_interval_ms
        };
        let remaining = Duration::from_millis(interval).saturating_sub(now.elapsed());
        match rx.recv_timeout(remaining) {
            Ok(next) => command = Some(next),
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    for _ in 0..3 {
        match tracker.release_all() {
            Ok(()) => return,
            Err(error) => {
                tracing::error!(%error, "exit release failed");
                thread::sleep(Duration::from_millis(30));
            }
        }
    }
}
