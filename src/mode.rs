use crate::config::{GameProfile, MatchRule};
use std::{
    path::Path,
    time::{Duration, Instant},
};
use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Policy {
    #[default]
    Auto,
    Desktop,
    Game,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Mode {
    #[default]
    Desktop,
    Game,
    Paused,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Detection {
    #[default]
    Desktop,
    Game(usize),
    Unknown,
}

#[derive(Default)]
pub struct ModeManager {
    pub policy: Policy,
    pub paused: bool,
    pub mode: Mode,
    pub(crate) restore_at: Option<Instant>,
}
impl ModeManager {
    pub fn update(&mut self, detection: Detection, now: Instant) -> bool {
        let desired = if self.paused {
            Mode::Paused
        } else {
            match self.policy {
                Policy::Desktop => Mode::Desktop,
                Policy::Game => Mode::Game,
                Policy::Auto => match detection {
                    Detection::Game(_) | Detection::Unknown => Mode::Game,
                    Detection::Desktop => Mode::Desktop,
                },
            }
        };
        if self.policy == Policy::Auto && self.mode == Mode::Game && desired == Mode::Desktop {
            let restore = self
                .restore_at
                .get_or_insert(now + Duration::from_millis(150));
            if now < *restore {
                return false;
            }
        }
        self.restore_at = None;
        let changed = desired != self.mode;
        self.mode = desired;
        changed
    }
}

#[derive(Default)]
pub struct GameDetector {
    window: usize,
    pid: u32,
    cached: Detection,
    retry: Option<Instant>,
}
impl GameDetector {
    pub fn detect(&mut self, games: &[GameProfile], now: Instant) -> Detection {
        if !games.iter().any(|game| game.enabled) {
            return Detection::Desktop;
        }
        let hwnd = unsafe { GetForegroundWindow() };
        let window = hwnd.0 as usize;
        if window == self.window && self.retry.is_some_and(|at| now < at) {
            return self.cached;
        }
        let mut pid = 0;
        if !hwnd.is_invalid() {
            unsafe {
                GetWindowThreadProcessId(hwnd, Some(&mut pid));
            }
        }
        if window == self.window && pid == self.pid && self.retry.is_none() && pid != 0 {
            return self.cached;
        }
        let previous = self.cached;
        self.window = window;
        self.pid = pid;
        let result = if pid == 0 {
            Detection::Unknown
        } else if pid == std::process::id() {
            Detection::Desktop
        } else {
            match crate::system::process_path(pid) {
                Ok(path) => Self::matches(games, &path),
                Err(error) => {
                    if previous != Detection::Unknown {
                        tracing::info!(pid, %error, "foreground process path unavailable");
                    }
                    Detection::Unknown
                }
            }
        };
        self.cached = result;
        self.retry = (result == Detection::Unknown).then_some(now + Duration::from_millis(350));
        result
    }
    pub fn matches(games: &[GameProfile], path: &Path) -> Detection {
        let path_text = path.to_string_lossy().replace('/', "\\").to_lowercase();
        let basename = path
            .file_name()
            .map(|name| name.to_string_lossy().to_lowercase());
        let matches = |game: &GameProfile, rule| {
            game.enabled
                && game.rule == rule
                && match rule {
                    MatchRule::Path => {
                        path_text
                            == game
                                .path
                                .to_string_lossy()
                                .replace('/', "\\")
                                .to_lowercase()
                    }
                    MatchRule::ExecutableName => {
                        basename
                            == game
                                .path
                                .file_name()
                                .map(|name| name.to_string_lossy().to_lowercase())
                    }
                }
        };
        games
            .iter()
            .position(|game| matches(game, MatchRule::Path))
            .or_else(|| {
                games
                    .iter()
                    .position(|game| matches(game, MatchRule::ExecutableName))
            })
            .map_or(Detection::Desktop, Detection::Game)
    }
}
