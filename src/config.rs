use crate::{
    input::MouseSettings,
    mapping::{Action, Behavior, MappingProfile},
};
use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, OpenOptions},
    io::Write,
    os::windows::ffi::OsStrExt,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};
use windows::{
    Win32::Storage::FileSystem::{MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW},
    core::PCWSTR,
};

pub const VERSION: u32 = 2;
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub version: u32,
    pub general: GeneralSettings,
    pub mouse: MouseSettings,
    pub gamepad: GamepadSettings,
    pub mapping: MappingProfile,
    pub games: Vec<GameProfile>,
    pub appearance: AppearanceSettings,
}
impl Default for Config {
    fn default() -> Self {
        Self {
            version: VERSION,
            general: Default::default(),
            mouse: Default::default(),
            gamepad: Default::default(),
            mapping: Default::default(),
            games: Vec::new(),
            appearance: Default::default(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct GeneralSettings {
    pub automatic: bool,
    pub start_hidden: bool,
    pub close_to_tray: bool,
    pub hotkey: Hotkey,
}
impl Default for GeneralSettings {
    fn default() -> Self {
        Self {
            automatic: true,
            start_hidden: false,
            close_to_tray: true,
            hotkey: Hotkey {
                modifiers: 3,
                key: 0x7b,
            },
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hotkey {
    pub modifiers: u32,
    pub key: u16,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct GamepadSettings {
    pub slot: Option<u32>,
    pub polling_interval_ms: u64,
    pub long_press_ms: u64,
    pub double_click_ms: u64,
}
impl Default for GamepadSettings {
    fn default() -> Self {
        Self {
            slot: None,
            polling_interval_ms: 8,
            long_press_ms: 500,
            double_click_ms: 260,
        }
    }
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GameProfile {
    pub name: String,
    pub path: PathBuf,
    pub enabled: bool,
    #[serde(default)]
    pub rule: MatchRule,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MatchRule {
    #[default]
    Path,
    ExecutableName,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct AppearanceSettings {
    pub theme: Theme,
}
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Theme {
    #[default]
    System,
    Light,
    Dark,
}

impl Config {
    pub fn validate(&self) -> Result<()> {
        ensure!(self.version == VERSION, "配置版本不受支持");
        let g = &self.gamepad;
        ensure!(
            g.slot.is_none_or(|slot| slot < 4)
                && (4..=32).contains(&g.polling_interval_ms)
                && (200..=2000).contains(&g.long_press_ms)
                && (120..=700).contains(&g.double_click_ms),
            "手柄参数超出范围"
        );
        let m = &self.mouse;
        ensure!(
            (0.02..=0.6).contains(&m.deadzone)
                && (0.5..=4.).contains(&m.response_exponent)
                && (100.0..=5000.).contains(&m.speed)
                && (1.0..=60.).contains(&m.scroll_speed)
                && (0.1..=1.).contains(&m.precision_multiplier)
                && (1.0..=4.).contains(&m.fast_multiplier)
                && (0.0..=80.).contains(&m.smoothing_ms),
            "鼠标参数超出范围"
        );
        let h = self.general.hotkey;
        ensure!(
            (1..=15).contains(&h.modifiers)
                && (8..=254).contains(&h.key)
                && !matches!(h.key, 0x10..=0x12 | 0x5b..=0x5c | 0xa0..=0xa5),
            "热键须包含修饰键和一个主键"
        );
        ensure!(!self.mapping.name.trim().is_empty(), "映射名称不能为空");
        for (index, binding) in self.mapping.buttons.iter().enumerate() {
            let label = crate::gamepad::BUTTONS[index];
            ensure!(
                [
                    &binding.press,
                    &binding.click,
                    &binding.double,
                    &binding.long,
                    &binding.hold
                ]
                .into_iter()
                .all(Action::valid),
                "{label} 存在无效按键、重复键或滚轮步数"
            );
            ensure!(
                (binding.mode != Behavior::Native || binding.press.can_hold())
                    && binding.hold.can_hold(),
                "{label} 的持续动作需要单键、鼠标按钮或速度模式"
            );
            let taps: &[&Action] = match binding.mode {
                Behavior::Native => &[],
                Behavior::Press => &[&binding.press],
                Behavior::Gestures => &[&binding.click, &binding.double, &binding.long],
            };
            ensure!(
                taps.iter()
                    .all(|a| !matches!(a, Action::Precision | Action::Fast)),
                "{label} 的速度模式需要持续按住"
            );
            ensure!(taps.iter().all(|a| !matches!(a, Action::Keys { keys } if crate::keys::hotkey(keys).ok()==Some(h))), "{label} 与全局热键冲突");
        }
        for (index, game) in self.games.iter().enumerate() {
            ensure!(
                !game.name.trim().is_empty()
                    && game.path.is_absolute()
                    && game
                        .path
                        .extension()
                        .is_some_and(|ext| ext.eq_ignore_ascii_case("exe")),
                "游戏需要名称和绝对 EXE 路径"
            );
            ensure!(
                !self.games[..index]
                    .iter()
                    .any(|g| g.path.to_string_lossy().to_lowercase()
                        == game.path.to_string_lossy().to_lowercase()
                        || (g.rule == MatchRule::ExecutableName
                            && game.rule == MatchRule::ExecutableName
                            && g.path
                                .file_name()
                                .map(|n| n.to_string_lossy().to_lowercase())
                                == game
                                    .path
                                    .file_name()
                                    .map(|n| n.to_string_lossy().to_lowercase()))),
                "游戏匹配规则重复"
            );
        }
        Ok(())
    }
}
pub fn decode(data: &[u8]) -> Result<Config> {
    let mut value: serde_json::Value = serde_json::from_slice(data)?;
    let table = value.as_object_mut().context("配置根节点应为对象")?;
    let version = match table.get("version") {
        None => 1,
        Some(v) => v.as_u64().context("配置版本应为整数")?,
    };
    match version {
        1 => {
            if !table.contains_key("mapping")
                && let Some(buttons) = table.remove("bindings")
            {
                table.insert("mapping".into(), serde_json::json!({"buttons": buttons}));
            }
            table.insert("version".into(), VERSION.into());
        }
        v if v == u64::from(VERSION) => {}
        _ => bail!("不支持配置版本 {version}"),
    }
    let config: Config = serde_json::from_value(value)?;
    config.validate()?;
    Ok(config)
}
pub fn read(path: &Path) -> Result<Config> {
    decode(&fs::read(path).with_context(|| format!("无法读取 {}", path.display()))?)
}
pub fn load(path: &Path) -> Result<(Config, Option<String>)> {
    match fs::read(path) {
        Ok(bytes) => match decode(&bytes) {
            Ok(config) => Ok((config, None)),
            Err(error) => {
                // Preserve newer configurations for opening with the matching build.
                if serde_json::from_slice::<serde_json::Value>(&bytes)
                    .ok()
                    .and_then(|v| v["version"].as_u64())
                    .is_some_and(|v| v > u64::from(VERSION))
                {
                    return Err(error);
                }
                let timestamp = SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis();
                let backup = path.with_extension(format!("invalid-{timestamp}.json"));
                fs::rename(path, &backup).context("无法备份损坏配置")?;
                let config = Config::default();
                save(path, &config)?;
                Ok((
                    config,
                    Some(format!("配置已恢复默认，原文件保存在 {}", backup.display())),
                ))
            }
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let config = Config::default();
            save(path, &config)?;
            Ok((config, None))
        }
        Err(error) => Err(error).with_context(|| format!("无法读取 {}", path.display())),
    }
}
pub fn wide(path: &std::ffi::OsStr) -> Vec<u16> {
    path.encode_wide().chain([0]).collect()
}
pub fn save(path: &Path, config: &Config) -> Result<()> {
    config.validate()?;
    let timestamp = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
    let tmp = path.with_extension(format!("{}.{}.tmp", std::process::id(), timestamp));
    let bytes = serde_json::to_vec_pretty(config)?;
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&tmp)
        .with_context(|| format!("无法写入 {}", path.display()))?;
    let result: Result<()> = (|| {
        file.write_all(&bytes)?;
        file.sync_all()?;
        drop(file);
        let from = wide(tmp.as_os_str());
        let to = wide(path.as_os_str());
        unsafe {
            MoveFileExW(
                PCWSTR(from.as_ptr()),
                PCWSTR(to.as_ptr()),
                MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
            )
        }?;
        Ok(())
    })();
    if result.is_err()
        && let Err(error) = fs::remove_file(&tmp)
    {
        tracing::warn!(%error, "cannot remove temporary configuration");
    }
    result.with_context(|| format!("保存失败 {}", path.display()))
}
