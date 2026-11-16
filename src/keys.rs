use crate::config::Hotkey;
use anyhow::{Result, bail, ensure};

const NAMES: &[(&str, u16)] = &[
    ("Ctrl", 17),
    ("Alt", 18),
    ("Shift", 16),
    ("Win", 91),
    ("Left", 37),
    ("Up", 38),
    ("Right", 39),
    ("Down", 40),
    ("Tab", 9),
    ("Enter", 13),
    ("Esc", 27),
    ("Space", 32),
    ("Backspace", 8),
    ("Delete", 46),
    ("Insert", 45),
    ("Home", 36),
    ("End", 35),
    ("PageUp", 33),
    ("PageDown", 34),
    ("PrintScreen", 44),
    ("CapsLock", 20),
    ("LShift", 160),
    ("RShift", 161),
    ("LCtrl", 162),
    ("RCtrl", 163),
    ("LAlt", 164),
    ("RAlt", 165),
    ("RWin", 92),
];
pub fn parse(text: &str) -> Result<Vec<u16>> {
    text.split('+')
        .map(|name| {
            let name = name.trim();
            if let Some((_, key)) = NAMES
                .iter()
                .find(|(alias, _)| alias.eq_ignore_ascii_case(name))
            {
                return Ok(*key);
            }
            if name.len() == 1 && name.as_bytes()[0].is_ascii_alphanumeric() {
                return Ok(u16::from(name.as_bytes()[0].to_ascii_uppercase()));
            }
            if let Some(n) = name
                .to_uppercase()
                .strip_prefix('F')
                .and_then(|v| v.parse::<u16>().ok())
                .filter(|n| (1..=24).contains(n))
            {
                return Ok(111 + n);
            }
            if let Some(key) = name
                .to_uppercase()
                .strip_prefix("VK_")
                .and_then(|v| u16::from_str_radix(v, 16).ok())
                .filter(|n| (8..=254).contains(n))
            {
                return Ok(key);
            }
            bail!("无法识别键名 {name}。示例为 Ctrl + C，其他虚拟键可用 VK_XX。")
        })
        .collect()
}
pub fn label(key: u16) -> String {
    match key {
        48..=57 | 65..=90 => char::from(key as u8).to_string(),
        112..=135 => format!("F{}", key - 111),
        _ => NAMES
            .iter()
            .find(|(_, vk)| *vk == key)
            .map_or_else(|| format!("VK_{key:02X}"), |(name, _)| (*name).into()),
    }
}
pub fn format(keys: &[u16]) -> String {
    keys.iter()
        .map(|&key| label(key))
        .collect::<Vec<_>>()
        .join(" + ")
}
pub fn hotkey(keys: &[u16]) -> Result<Hotkey> {
    let mut modifiers = 0;
    let mut main = None;
    for &key in keys {
        let flag = match key {
            18 | 164 | 165 => 1,
            17 | 162 | 163 => 2,
            16 | 160 | 161 => 4,
            91 | 92 => 8,
            _ => {
                ensure!(main.replace(key).is_none(), "全局热键只能有一个主键");
                continue;
            }
        };
        ensure!(modifiers & flag == 0, "热键包含重复修饰键");
        modifiers |= flag;
    }
    ensure!(modifiers != 0, "全局热键需要 Ctrl、Alt、Shift 或 Win");
    let key = main.ok_or_else(|| anyhow::anyhow!("缺少热键主键"))?;
    Ok(Hotkey { modifiers, key })
}
pub fn hotkey_label(h: Hotkey) -> String {
    let mut keys = Vec::new();
    for (flag, key) in [(2, 17), (1, 18), (4, 16), (8, 91)] {
        if h.modifiers & flag != 0 {
            keys.push(key);
        }
    }
    keys.push(h.key);
    format(&keys)
}
