use crate::{
    Colors, GameRow, MainWindow, PadKey, PadTray, Setting,
    config::{self, Config, GameProfile, MatchRule, Theme},
    gamepad::{self, Pad},
    input::MouseButton,
    keys,
    mapping::{Action, Behavior, Binding, MappingProfile},
    mode::{Detection, Mode, Policy},
    system::{self, Dialog, Instance},
    worker::{Command, Status, Worker},
};
use anyhow::{Context, Result};
use slint::{
    CloseRequestResponse, ComponentHandle, Model, ModelRc, SharedString, Timer, TimerMode, VecModel,
};
use std::{
    cell::{Cell, RefCell},
    fs,
    path::PathBuf,
    rc::Rc,
    sync::mpsc::{SyncSender, TrySendError},
    time::{Duration, Instant},
};
use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};

struct Ui {
    config: RefCell<Config>,
    pending: Cell<bool>,
    save_at: Cell<Option<Instant>>,
    status: RefCell<Status>,
    tx: SyncSender<Command>,
    dir: PathBuf,
    exe: PathBuf,
    processes: RefCell<Vec<PathBuf>>,
    buttons: Rc<VecModel<PadKey>>,
    games: Rc<VecModel<GameRow>>,
    sliders: Rc<VecModel<Setting>>,
    slots: Rc<VecModel<SharedString>>,
    visible: Cell<bool>,
    title_dark: Cell<Option<bool>>,
}

const POSITIONS: [(f32, f32); 16] = [
    (0.755, 0.56),
    (0.838, 0.425),
    (0.672, 0.425),
    (0.755, 0.29),
    (0.432, 0.333),
    (0.568, 0.333),
    (0.172, 0.175),
    (0.828, 0.175),
    (0.35, 0.466),
    (0.35, 0.725),
    (0.283, 0.595),
    (0.417, 0.595),
    (0.227, 0.355),
    (0.625, 0.603),
    (0.253, 0.065),
    (0.747, 0.065),
];
const BUTTON_LABELS: [&str; 3] = ["左键", "右键", "中键"];

fn model<T: Clone + 'static>(value: &Rc<VecModel<T>>) -> ModelRc<T> {
    ModelRc::from(Rc::clone(value))
}
fn report(w: &MainWindow, result: Result<()>) {
    if let Err(error) = result {
        tracing::warn!(%error, "operation failed");
        w.set_notice(format!("{error:#}").into());
    }
}
fn action_fields(action: &Action) -> (i32, String) {
    match action {
        Action::None => (0, String::new()),
        Action::Mouse { button } => (1 + *button as i32, String::new()),
        Action::DoubleClick { button } => (4 + *button as i32, String::new()),
        Action::Wheel { steps, horizontal } => (7 + i32::from(*horizontal), steps.to_string()),
        Action::Keys { keys } => (9, keys::format(keys)),
        Action::Precision => (10, String::new()),
        Action::Fast => (11, String::new()),
        Action::Pause => (12, String::new()),
    }
}
fn action_label(action: &Action) -> String {
    match action {
        Action::None => "无操作".into(),
        Action::Mouse { button } => format!("鼠标{}", BUTTON_LABELS[*button as usize]),
        Action::DoubleClick { button } => format!("鼠标{}双击", BUTTON_LABELS[*button as usize]),
        Action::Wheel { steps, horizontal } => format!(
            "{}滚轮 {steps:+}",
            if *horizontal { "水平" } else { "垂直" }
        ),
        Action::Keys { keys } => keys::format(keys),
        Action::Precision => "精细移动".into(),
        Action::Fast => "快速移动".into(),
        Action::Pause => "暂停映射".into(),
    }
}
fn binding_label(binding: &Binding) -> String {
    if binding.mode != Behavior::Gestures {
        return action_label(&binding.press);
    }
    let labels: Vec<_> = [
        ("单击", &binding.click),
        ("双击", &binding.double),
        ("长按", &binding.long),
        ("持续", &binding.hold),
    ]
    .into_iter()
    .filter(|(_, action)| **action != Action::None)
    .map(|(kind, action)| format!("{kind}  {}", action_label(action)))
    .collect();
    if labels.is_empty() {
        "无操作".into()
    } else {
        labels.join("   /   ")
    }
}
fn binding_action(binding: &Binding, gesture: i32) -> &Action {
    match gesture {
        0 => &binding.click,
        1 => &binding.double,
        2 => &binding.long,
        3 => &binding.hold,
        _ => &binding.press,
    }
}
fn make_action(kind: i32, text: &str) -> Result<Action> {
    let buttons = [MouseButton::Left, MouseButton::Right, MouseButton::Middle];
    Ok(match kind {
        1..=3 => Action::Mouse {
            button: buttons[(kind - 1) as usize],
        },
        4..=6 => Action::DoubleClick {
            button: buttons[(kind - 4) as usize],
        },
        7..=8 => Action::Wheel {
            steps: text
                .trim()
                .parse()
                .context("滚轮动作需要一个非零整数步数")?,
            horizontal: kind == 8,
        },
        9 => Action::Keys {
            keys: keys::parse(text)?,
        },
        10 => Action::Precision,
        11 => Action::Fast,
        12 => Action::Pause,
        _ => Action::None,
    })
}

impl Ui {
    fn change(&self, edit: impl FnOnce(&mut Config)) -> Result<()> {
        let mut config = self.config.borrow_mut();
        let mut next = config.clone();
        edit(&mut next);
        next.validate()?;
        *config = next;
        self.pending.set(true);
        self.save_at
            .set(Some(Instant::now() + Duration::from_millis(450)));
        Ok(())
    }
    fn send(&self, command: Command) -> Result<()> {
        self.tx
            .try_send(command)
            .context("无法向输入线程发送命令，输入线程繁忙或已退出")
    }
    fn refresh_config(&self, w: &MainWindow) -> Result<()> {
        let config = self.config.borrow();
        self.refresh_theme(w)?;
        w.set_profile(config.mapping.name.clone().into());
        w.set_speed_label(format!("{:.0} px/s", config.mouse.speed).into());
        w.set_slot_index(config.gamepad.slot.map_or(0, |i| i as i32 + 1));
        w.set_theme_index(config.appearance.theme as i32);
        w.set_automatic_at_start(config.general.automatic);
        w.set_close_to_tray(config.general.close_to_tray);
        w.set_start_hidden(config.general.start_hidden);
        w.set_hotkey_text(keys::hotkey_label(config.general.hotkey).into());
        self.refresh_sliders();
        self.refresh_keys(self.status.borrow().pad.unwrap_or_default());
        self.refresh_games(self.status.borrow().detection);
        Ok(())
    }
    fn refresh_theme(&self, w: &MainWindow) -> Result<()> {
        let dark = match self.config.borrow().appearance.theme {
            Theme::System => system::system_dark()?,
            Theme::Dark => true,
            Theme::Light => false,
        };
        w.global::<Colors>().invoke_apply_theme(dark);
        if self.title_dark.get() != Some(dark) && system::apply_window_theme(w.window(), dark) {
            self.title_dark.set(Some(dark));
        }
        Ok(())
    }
    fn refresh_sliders(&self) {
        let config = self.config.borrow();
        let m = &config.mouse;
        let g = &config.gamepad;
        let rows = [
            ("指针速度", m.speed, 100., 5000., 25., " px/s"),
            ("滚动速度", m.scroll_speed, 1., 60., 1., " 格/s"),
            ("摇杆死区", m.deadzone, 0.02, 0.6, 0.01, ""),
            ("响应指数", m.response_exponent, 0.5, 4., 0.05, ""),
            ("平滑时间", m.smoothing_ms, 0., 80., 1., " ms"),
            ("精细速度倍率", m.precision_multiplier, 0.1, 1., 0.05, " ×"),
            ("快速速度倍率", m.fast_multiplier, 1., 4., 0.05, " ×"),
            ("长按阈值", g.long_press_ms as f64, 200., 2000., 10., " ms"),
            ("双击间隔", g.double_click_ms as f64, 120., 700., 10., " ms"),
            ("手柄轮询", g.polling_interval_ms as f64, 4., 32., 1., " ms"),
        ];
        for (index, (title, value, low, high, step, unit)) in rows.into_iter().enumerate() {
            let row = Setting {
                title: title.into(),
                unit: unit.into(),
                value: value as f32,
                low: low as f32,
                high: high as f32,
                step: step as f32,
            };
            if self.sliders.row_count() <= index {
                self.sliders.push(row);
            } else if self
                .sliders
                .row_data(index)
                .is_some_and(|prev| prev.value != row.value)
            {
                self.sliders.set_row_data(index, row);
            }
        }
    }
    fn refresh_keys(&self, state: Pad) {
        let config = self.config.borrow();
        for (index, name) in gamepad::BUTTONS.into_iter().enumerate() {
            let (x, y) = POSITIONS[index];
            let row = PadKey {
                name: name.into(),
                x,
                y,
                pressed: state.button(index),
                binding: binding_label(&config.mapping.buttons[index]).into(),
            };
            if self.buttons.row_count() <= index {
                self.buttons.push(row);
            } else if self
                .buttons
                .row_data(index)
                .is_some_and(|prev| prev.pressed != row.pressed || prev.binding != row.binding)
            {
                self.buttons.set_row_data(index, row);
            }
        }
    }
    fn refresh_games(&self, detection: Detection) {
        let config = self.config.borrow();
        self.games.set_vec(
            config
                .games
                .iter()
                .enumerate()
                .map(|(index, game)| GameRow {
                    name: game.name.clone().into(),
                    path: game.path.to_string_lossy().as_ref().into(),
                    enabled: game.enabled,
                    rule: game.rule as i32,
                    match_label: if !game.enabled {
                        "已停用"
                    } else if detection == Detection::Game(index) {
                        "前台已匹配"
                    } else {
                        "等待匹配"
                    }
                    .into(),
                })
                .collect::<Vec<_>>(),
        );
    }
    fn refresh_editor(&self, w: &MainWindow, index: i32) {
        w.set_selected(index);
        let config = self.config.borrow();
        let binding = &config.mapping.buttons[index as usize];
        w.set_edit_mode(binding.mode as i32);
        w.set_mapping_summary(
            format!(
                "{}  /  {}",
                gamepad::BUTTONS[index as usize],
                binding_label(binding)
            )
            .into(),
        );
        drop(config);
        self.select_gesture(w, if w.get_edit_mode() == 2 { 0 } else { 4 });
    }
    fn select_gesture(&self, w: &MainWindow, index: i32) {
        w.set_gesture(index);
        let config = self.config.borrow();
        let binding = &config.mapping.buttons[w.get_selected() as usize];
        let (kind, text) = action_fields(binding_action(binding, index));
        w.set_action_index(kind);
        w.set_action_text(text.into());
    }
    fn add_game(&self, w: &MainWindow, path: PathBuf) -> Result<()> {
        let name = path
            .file_stem()
            .context("缺少 EXE 文件名")?
            .to_string_lossy()
            .into_owned();
        self.change(|config| {
            config.games.push(GameProfile {
                name,
                path,
                enabled: true,
                rule: MatchRule::Path,
            })
        })?;
        self.refresh_games(Detection::Desktop);
        w.set_notice("已添加游戏。".into());
        Ok(())
    }
    fn select_policy(&self, value: i32) -> Result<()> {
        self.send(Command::Policy(match value {
            1 => Policy::Desktop,
            2 => Policy::Game,
            _ => Policy::Auto,
        }))
    }
    fn show(&self, w: &MainWindow) -> Result<()> {
        w.show()?;
        self.visible.set(true);
        // The native window exists only after show(), so apply the frame theme now.
        self.refresh_theme(w)?;
        #[cfg(windows)]
        {
            use slint::winit_030::WinitWindowAccessor;
            w.window().with_winit_window(|window| {
                window.set_minimized(false);
                window.focus_window();
            });
        }
        w.window().request_redraw();
        Ok(())
    }
    fn flush(&self, exit: bool) -> Result<()> {
        if self.pending.get() && !exit {
            match self
                .tx
                .try_send(Command::Configure(Box::new(self.config.borrow().clone())))
            {
                Ok(()) => self.pending.set(false),
                Err(TrySendError::Full(_)) => {}
                Err(TrySendError::Disconnected(_)) => {
                    self.pending.set(false);
                    anyhow::bail!("输入线程已退出");
                }
            }
        }
        if self
            .save_at
            .get()
            .is_some_and(|at| exit || Instant::now() >= at)
        {
            self.save_at.set(None);
            config::save(&self.dir.join("config.json"), &self.config.borrow())?;
        }
        Ok(())
    }
    fn sync(&self, w: &MainWindow, status: Status) {
        let prev = self.status.replace(status);
        let now = self.status.borrow();
        let pad = now.pad.unwrap_or_default();
        w.set_controller_name(now.active.map_or_else(
            || "等待连接 XInput 手柄".into(),
            |slot| format!("XInput 手柄 · 槽位 {slot}").into(),
        ));
        w.set_connected(now.active.is_some());
        w.set_mode_label(
            match now.mode {
                Mode::Desktop => "桌面模式",
                Mode::Game => "游戏模式",
                Mode::Paused => "已暂停",
            }
            .into(),
        );
        w.set_policy(match now.policy {
            Policy::Auto => 0,
            Policy::Desktop => 1,
            Policy::Game => 2,
        });
        w.set_paused(now.paused);
        w.set_mapping_enabled(now.mode == Mode::Desktop);
        if now.slots != prev.slots {
            for (index, connected) in now.slots.into_iter().enumerate() {
                self.slots.set_row_data(
                    index + 1,
                    format!(
                        "槽位 {index}（{}）",
                        if connected { "已连接" } else { "未连接" }
                    )
                    .into(),
                );
            }
        }
        if now.detection != prev.detection {
            self.refresh_games(now.detection);
        }
        w.set_detection(match now.detection {
            Detection::Desktop => {
                if self.config.borrow().games.iter().any(|g| g.enabled) {
                    "前台未匹配游戏".into()
                } else {
                    "尚未启用游戏匹配规则".into()
                }
            }
            Detection::Unknown => "无法确认前台进程，自动模式暂停映射。".into(),
            Detection::Game(i) => self.config.borrow().games.get(i).map_or_else(
                || "已匹配游戏".into(),
                |g| format!("前台游戏  {}", g.name).into(),
            ),
        });
        if now.input_error != prev.input_error || now.hotkey_error != prev.hotkey_error {
            w.set_system_status(
                [now.hotkey_error.as_deref(), now.input_error.as_deref()]
                    .into_iter()
                    .flatten()
                    .collect::<Vec<_>>()
                    .join("\n")
                    .into(),
            );
        }
        let mut changed_buttons = pad.buttons ^ prev.pad.unwrap_or_default().buttons;
        while changed_buttons != 0 {
            let index = changed_buttons.trailing_zeros() as usize;
            changed_buttons &= !(1 << index);
            let mut row = self
                .buttons
                .row_data(index)
                .expect("gamepad key model initialized");
            row.pressed = pad.button(index);
            self.buttons.set_row_data(index, row);
        }
        if w.get_page() <= 1 {
            w.set_lx(pad.left[0] as f32);
            w.set_ly(pad.left[1] as f32);
            w.set_rx(pad.right[0] as f32);
            w.set_ry(pad.right[1] as f32);
            w.set_lt(pad.triggers[0] as f32);
            w.set_rt(pad.triggers[1] as f32);
        }
    }
}

pub fn run() -> Result<()> {
    let Some(instance) = Instance::acquire()? else {
        return Ok(());
    };
    let exe = std::env::current_exe()?;
    let folder = exe.parent().context("找不到应用目录")?;
    let portable = folder.join("portable.flag").try_exists()?;
    let dir = if portable {
        folder.join("data")
    } else {
        PathBuf::from(std::env::var_os("APPDATA").context("无法定位当前用户 AppData")?)
            .join("PadPointer")
    };
    fs::create_dir_all(&dir)
        .with_context(|| format!("无法创建 {}。便携版需要可写目录。", dir.display()))?;
    let log = dir.join("padpointer.log");
    let file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log)
        .with_context(|| format!("无法写入日志 {}", log.display()))?;

    let log = std::sync::Mutex::new(Log {
        bytes: file.metadata()?.len(),
        file,
        path: log,
        previous: dir.join("padpointer.previous.log"),
    });
    tracing_subscriber::fmt()
        .with_ansi(false)
        .with_max_level(tracing::Level::INFO)
        .with_writer(LogFactory(log))
        .init();

    let (config, recovery) = config::load(&dir.join("config.json"))?;
    let hidden = config.general.start_hidden && std::env::args_os().any(|arg| arg == "--startup");
    slint::BackendSelector::new()
        .backend_name("winit".into())
        .renderer_name("software".into())
        .with_winit_window_attributes_hook(|attributes| attributes.with_transparent(false))
        .select()?;
    let window = MainWindow::new()?;
    {
        use slint::winit_030::{EventResult, WinitWindowAccessor, winit::event::WindowEvent};
        window.window().on_winit_window_event(|window, event| {
            if matches!(
                event,
                WindowEvent::Focused(true) | WindowEvent::Occluded(false)
            ) {
                window.request_redraw();
            }
            EventResult::Propagate
        });
    }
    let tray = PadTray::new()?;
    let worker = Worker::spawn(config.clone())?;
    let ui = Rc::new(Ui {
        config: RefCell::new(config),
        pending: Cell::new(false),
        save_at: Cell::new(None),
        status: RefCell::new(Status::default()),
        tx: worker.tx.clone(),
        dir,
        exe,
        processes: RefCell::new(Vec::new()),
        buttons: Rc::new(VecModel::default()),
        games: Rc::new(VecModel::default()),
        sliders: Rc::new(VecModel::default()),
        slots: Rc::new(VecModel::from(
            [
                "自动选择",
                "槽位 0（未连接）",
                "槽位 1（未连接）",
                "槽位 2（未连接）",
                "槽位 3（未连接）",
            ]
            .map(SharedString::from)
            .to_vec(),
        )),
        visible: Cell::new(false),
        title_dark: Cell::new(None),
    });
    let result = run_ui(&window, &tray, &worker, &ui, instance, hidden, recovery);
    let stopped = worker.stop();
    result.and(stopped)
}

struct Log {
    file: fs::File,
    bytes: u64,
    path: PathBuf,
    previous: PathBuf,
}
impl std::io::Write for Log {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if self.bytes + buf.len() as u64 > 1024 * 1024 {
            self.file.flush()?;
            fs::rename(&self.path, &self.previous)?;
            self.file = fs::File::create_new(&self.path)?;
            self.bytes = 0;
        }
        let written = self.file.write(buf)?;
        self.bytes += written as u64;
        Ok(written)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.file.flush()
    }
}
struct LogWriter<'a>(std::sync::MutexGuard<'a, Log>);
impl std::io::Write for LogWriter<'_> {
    fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
        self.0.write(data)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.0.flush()
    }
}
struct LogFactory(std::sync::Mutex<Log>);
impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for LogFactory {
    type Writer = LogWriter<'a>;
    fn make_writer(&'a self) -> Self::Writer {
        LogWriter(self.0.lock().expect("log lock poisoned"))
    }
}

fn owner() -> windows::Win32::Foundation::HWND {
    let hwnd = unsafe { GetForegroundWindow() };
    let mut pid = 0;
    unsafe {
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
    }
    if pid == std::process::id() {
        hwnd
    } else {
        Default::default()
    }
}
fn quit() -> Result<()> {
    Ok(slint::quit_event_loop()?)
}

fn run_ui(
    window: &MainWindow,
    tray: &PadTray,
    worker: &Worker,
    ui: &Rc<Ui>,
    instance: Instance,
    hidden: bool,
    recovery: Option<String>,
) -> Result<()> {
    window.set_keys(model(&ui.buttons));
    window.set_games(model(&ui.games));
    window.set_sliders(model(&ui.sliders));
    window.set_slots(model(&ui.slots));
    window.set_storage(ui.dir.to_string_lossy().as_ref().into());
    window.set_version(env!("CARGO_PKG_VERSION").into());
    // Development builds can open a specific page directly, which keeps UI
    // screenshots and layout review independent of mouse input.
    #[cfg(debug_assertions)]
    if let Ok(page) = std::env::var("PADPOINTER_PAGE")
        && let Ok(page) = page.parse::<i32>()
    {
        window.set_page(page);
    }
    if let Some(recovery) = recovery {
        window.set_notice(recovery.into());
    }
    report(
        window,
        system::startup_enabled().map(|value| window.set_startup_enabled(value)),
    );
    ui.refresh_config(window)?;
    ui.refresh_editor(window, 0);

    macro_rules! wire {
        ($callback:ident, |$u:ident,$w:ident $(,$arg:ident)*| $body:block) => {{
            let $u=Rc::clone(ui);
            let weak=window.as_weak();
            window.$callback(move |$($arg),*| {
                if let Some($w)=weak.upgrade() {
                    let operation = || -> Result<()> { $body };
                    report(&$w, operation());
                }
            });
        }}
    }
    wire!(on_pause, |u, w, paused| {
        let _ = &w;
        u.send(Command::Pause(paused))
    });
    wire!(on_select_policy, |u, w, value| {
        let _ = &w;
        u.select_policy(value)
    });
    wire!(on_select_key, |u, w, index| {
        u.refresh_editor(&w, index);
        Ok(())
    });
    wire!(on_select_gesture, |u, w, index| {
        u.select_gesture(&w, index);
        Ok(())
    });
    wire!(on_save_binding, |u, w, mode, kind, text| {
        let action = make_action(kind, text.as_str())?;
        let mode = match mode {
            1 => Behavior::Press,
            2 => Behavior::Gestures,
            _ => Behavior::Native,
        };
        let index = w.get_selected();
        let gesture = w.get_gesture();
        u.change(|config| {
            let binding = &mut config.mapping.buttons[index as usize];
            binding.mode = mode;
            *match (mode, gesture) {
                (Behavior::Gestures, 0) => &mut binding.click,
                (Behavior::Gestures, 1) => &mut binding.double,
                (Behavior::Gestures, 2) => &mut binding.long,
                (Behavior::Gestures, 3) => &mut binding.hold,
                _ => &mut binding.press,
            } = action;
        })?;
        u.refresh_keys(u.status.borrow().pad.unwrap_or_default());
        u.refresh_editor(&w, index);
        u.select_gesture(&w, gesture);
        w.set_notice("绑定已更新。".into());
        Ok(())
    });
    wire!(on_clear_binding, |u, w| {
        let index = w.get_selected();
        u.change(|config| config.mapping.buttons[index as usize] = Binding::default())?;
        u.refresh_keys(u.status.borrow().pad.unwrap_or_default());
        u.refresh_editor(&w, index);
        w.set_notice("该按键的绑定已清空。".into());
        Ok(())
    });
    wire!(on_reset_bindings, |u, w| {
        u.change(|config| config.mapping = MappingProfile::default())?;
        u.refresh_config(&w)?;
        u.refresh_editor(&w, w.get_selected());
        w.set_notice("已恢复 Xbox 默认键位。".into());
        Ok(())
    });
    wire!(on_recorded, |u,
                        w,
                        text,
                        special,
                        ctrl,
                        alt,
                        shift,
                        meta| {
        let _ = &u;
        if w.get_page() != 1 || w.get_action_index() != 9 {
            return Ok(());
        }
        let vk = if special != 0 {
            special as u16
        } else {
            let Some(c) = text.chars().next() else {
                return Ok(());
            };
            if c.is_ascii_alphanumeric() {
                u16::from(c.to_ascii_uppercase() as u8)
            } else if c == ' ' {
                32
            } else if ('\u{f704}'..='\u{f71b}').contains(&c) {
                112 + (c as u16 - 0xf704)
            } else {
                return Ok(());
            }
        };
        let mut chord = Vec::with_capacity(5);
        for (pressed, key) in [(ctrl, 17), (alt, 18), (shift, 16), (meta, 91)] {
            if pressed {
                chord.push(key);
            }
        }
        chord.push(vk);
        w.set_action_text(keys::format(&chord).into());
        Ok(())
    });
    wire!(on_game_file, |u, w| {
        if let Some(path) = system::file_dialog(Dialog::Game, &u.dir, owner())? {
            u.add_game(&w, path)?;
        }
        Ok(())
    });
    wire!(on_scan_processes, |u, w| {
        let paths = system::processes()?;
        w.set_processes(ModelRc::new(VecModel::from(
            paths
                .iter()
                .map(|p| SharedString::from(p.to_string_lossy().as_ref()))
                .collect::<Vec<_>>(),
        )));
        w.set_process_index(if paths.is_empty() { -1 } else { 0 });
        *u.processes.borrow_mut() = paths;
        Ok(())
    });
    wire!(on_add_process, |u, w, index| {
        let path = u.processes.borrow()[index as usize].clone();
        u.add_game(&w, path)
    });
    wire!(on_change_game, |u, w, index, name, enabled, rule| {
        u.change(|config| {
            let game = &mut config.games[index as usize];
            game.name = name.trim().into();
            game.enabled = enabled;
            game.rule = if rule == 0 {
                MatchRule::Path
            } else {
                MatchRule::ExecutableName
            };
        })?;
        u.refresh_games(Detection::Desktop);
        w.set_notice("游戏规则已更新。".into());
        Ok(())
    });
    wire!(on_delete_game, |u, w, index| {
        u.change(|config| {
            config.games.remove(index as usize);
        })?;
        u.refresh_games(Detection::Desktop);
        w.set_notice("游戏规则已删除。".into());
        Ok(())
    });
    wire!(on_change_setting, |u, w, index, value| {
        let value = (f64::from(value) * 1000.).round() / 1000.;
        u.change(|c| match index {
            0 => c.mouse.speed = value,
            1 => c.mouse.scroll_speed = value,
            2 => c.mouse.deadzone = value,
            3 => c.mouse.response_exponent = value,
            4 => c.mouse.smoothing_ms = value,
            5 => c.mouse.precision_multiplier = value,
            6 => c.mouse.fast_multiplier = value,
            7 => c.gamepad.long_press_ms = value.round() as u64,
            8 => c.gamepad.double_click_ms = value.round() as u64,
            _ => c.gamepad.polling_interval_ms = value.round() as u64,
        })?;
        u.refresh_sliders();
        w.set_speed_label(format!("{:.0} px/s", u.config.borrow().mouse.speed).into());
        Ok(())
    });
    wire!(on_change_option, |u, w, index, value| {
        if index == 4 {
            system::set_startup(value != 0, &u.exe)?;
            w.set_startup_enabled(value != 0);
            return Ok(());
        }
        u.change(|c| match index {
            0 => c.gamepad.slot = (value > 0).then(|| (value - 1) as u32),
            1 => {
                c.appearance.theme = match value {
                    1 => Theme::Light,
                    2 => Theme::Dark,
                    _ => Theme::System,
                }
            }
            2 => c.general.start_hidden = value != 0,
            3 => c.general.close_to_tray = value != 0,
            _ => c.general.automatic = value == 0,
        })?;
        u.refresh_config(&w)
    });
    wire!(on_save_hotkey, |u, w, text| {
        let h = keys::hotkey(&keys::parse(text.as_str())?)?;
        u.change(|c| c.general.hotkey = h)?;
        w.set_hotkey_text(keys::hotkey_label(h).into());
        Ok(())
    });
    wire!(on_exchange, |u, w, export| {
        let action = if export {
            Dialog::Export
        } else {
            Dialog::Import
        };
        if let Some(path) = system::file_dialog(action, &u.dir, owner())? {
            if export {
                config::save(&path, &u.config.borrow())?;
                w.set_notice(format!("已导出到 {}", path.display()).into());
            } else {
                let config = config::read(&path)?;
                u.change(|c| *c = config)?;
                u.refresh_config(&w)?;
                u.refresh_editor(&w, w.get_selected());
                w.set_notice(format!("已导入 {}", path.display()).into());
            }
        }
        Ok(())
    });
    wire!(on_open_folder, |u, w| {
        let _ = &w;
        std::process::Command::new("explorer.exe")
            .arg(&u.dir)
            .spawn()
            .context("无法打开资源管理器")?;
        Ok(())
    });
    wire!(on_exit_app, |u, w| {
        let _ = (&u, &w);
        quit()
    });
    {
        let ui = Rc::clone(ui);
        let weak = window.as_weak();
        window.window().on_close_requested(move || {
            if ui.config.borrow().general.close_to_tray {
                ui.visible.set(false);
                CloseRequestResponse::HideWindow
            } else {
                if let Some(w) = weak.upgrade() {
                    report(&w, quit());
                }
                CloseRequestResponse::KeepWindowShown
            }
        });
    }
    {
        let ui = Rc::clone(ui);
        let weak = window.as_weak();
        tray.on_command(move |action| {
            if let Some(w) = weak.upgrade() {
                let result = match action {
                    0 => ui.show(&w),
                    1 => ui.send(Command::Pause(false)),
                    2 => ui.send(Command::Pause(true)),
                    3..=5 => ui.select_policy(action - 3),
                    6 => {
                        w.set_page(3);
                        ui.show(&w)
                    }
                    _ => quit(),
                };
                report(&w, result);
            }
        });
    }
    let timer = Timer::default();
    {
        let ui = Rc::clone(ui);
        let weak = window.as_weak();
        let status = worker.status.clone();
        let mut system_check = Instant::now();
        timer.start(TimerMode::Repeated, Duration::from_millis(80), move || {
            if let Some(w) = weak.upgrade() {
                if instance.show_requested() {
                    report(&w, ui.show(&w));
                }
                report(&w, ui.flush(false));
                if ui.visible.get() {
                    ui.sync(
                        &w,
                        status.lock().expect("input status lock poisoned").clone(),
                    );
                    if Instant::now() >= system_check {
                        system_check = Instant::now() + Duration::from_secs(2);
                        report(&w, ui.refresh_theme(&w));
                    }
                }
            }
        });
    }
    tray.show()?;
    if !hidden {
        ui.show(window)?;
    }
    tracing::info!("PadPointer started");
    let event_loop = slint::run_event_loop_until_quit();
    timer.stop();
    let save = ui.flush(true);
    let hide = tray.hide();
    event_loop?;
    hide?;
    save
}
