use std::time::{Duration, Instant};
use windows::Win32::UI::Input::XboxController::{XINPUT_STATE, XInputGetState};

pub const BUTTONS: [&str; 16] = [
    "A", "B", "X", "Y", "View", "Menu", "LB", "RB", "↑", "↓", "←", "→", "LS", "RS", "LT", "RT",
];
const MASKS: [u16; 14] = [
    0x1000, 0x2000, 0x4000, 0x8000, 0x20, 0x10, 0x100, 0x200, 1, 2, 4, 8, 0x40, 0x80,
];

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Pad {
    pub buttons: u16,
    pub left: [f64; 2],
    pub right: [f64; 2],
    pub triggers: [f64; 2],
}
impl Pad {
    pub fn button(&self, index: usize) -> bool {
        self.buttons & (1 << index) != 0
    }
}
pub trait GamepadProvider {
    fn state(&mut self, slot: u32) -> Option<Pad>;
}
pub struct XInputProvider;
impl GamepadProvider for XInputProvider {
    fn state(&mut self, slot: u32) -> Option<Pad> {
        let mut state = XINPUT_STATE::default();
        let result = unsafe { XInputGetState(slot, &mut state) };
        if result != 0 {
            if result != 1167 {
                tracing::debug!(slot, result, "XInputGetState failed");
            }
            return None;
        }
        let g = state.Gamepad;
        let axis = |v: i16| f64::from(v) / if v < 0 { 32768. } else { 32767. };
        let triggers = [g.bLeftTrigger, g.bRightTrigger].map(|v| f64::from(v) / 255.);
        let mut buttons = 0;
        for (index, mask) in MASKS.into_iter().enumerate() {
            if g.wButtons.0 & mask != 0 {
                buttons |= 1 << index;
            }
        }
        for (index, value) in triggers.into_iter().enumerate() {
            if value >= 30. / 255. {
                buttons |= 1 << (14 + index);
            }
        }
        Some(Pad {
            buttons,
            left: [axis(g.sThumbLX), axis(g.sThumbLY)],
            right: [axis(g.sThumbRX), axis(g.sThumbRY)],
            triggers,
        })
    }
}

pub struct GamepadManager<P> {
    provider: P,
    pub slots: [bool; 4],
    pub active: Option<u32>,
    pub preferred: Option<u32>,
    scan_at: Instant,
}
impl<P: GamepadProvider> GamepadManager<P> {
    pub fn new(provider: P, now: Instant) -> Self {
        Self {
            provider,
            slots: [false; 4],
            active: None,
            preferred: None,
            scan_at: now,
        }
    }
    pub fn select(&mut self, slot: Option<u32>, now: Instant) {
        self.preferred = slot;
        self.scan_at = now;
    }
    pub fn poll(&mut self, now: Instant) -> Option<Pad> {
        if now >= self.scan_at {
            self.scan_at = now + Duration::from_millis(500);
            let states = std::array::from_fn::<_, 4, _>(|i| self.provider.state(i as u32));
            self.slots = states.map(|state| state.is_some());
            self.active = self
                .preferred
                .map(|s| s as usize)
                .or_else(|| self.active.map(|s| s as usize).filter(|&s| self.slots[s]))
                .or_else(|| self.slots.iter().position(|&connected| connected))
                .filter(|&s| self.slots[s])
                .map(|s| s as u32);
            return self.active.and_then(|s| states[s as usize]);
        }
        let slot = self.active?;
        let state = self.provider.state(slot);
        if state.is_none() {
            self.active = None;
            self.slots[slot as usize] = false;
            self.scan_at = now;
        }
        state
    }
}
