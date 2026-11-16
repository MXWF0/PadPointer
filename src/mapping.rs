use crate::{
    gamepad::{BUTTONS, Pad},
    input::*,
};
use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant};

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Action {
    #[default]
    None,
    Mouse {
        button: MouseButton,
    },
    DoubleClick {
        button: MouseButton,
    },
    Wheel {
        steps: i32,
        horizontal: bool,
    },
    Keys {
        keys: Vec<u16>,
    },
    Precision,
    Fast,
    Pause,
}
impl Action {
    pub fn valid(&self) -> bool {
        match self {
            Self::Keys { keys } => {
                (1..=5).contains(&keys.len())
                    && keys
                        .iter()
                        .enumerate()
                        .all(|(i, key)| (8..=254).contains(key) && !keys[..i].contains(key))
            }
            Self::Wheel { steps, .. } => (-20..=20).contains(steps) && *steps != 0,
            _ => true,
        }
    }
    pub fn can_hold(&self) -> bool {
        matches!(
            self,
            Self::None | Self::Mouse { .. } | Self::Precision | Self::Fast
        ) || matches!(self, Self::Keys { keys } if keys.len() == 1)
    }
    fn control(&self) -> Option<Control> {
        match self {
            Self::Mouse { button } => Some(Control::Mouse(*button)),
            Self::Keys { keys } if keys.len() == 1 => Some(Control::Key(keys[0])),
            _ => None,
        }
    }
    fn tap<I: InputInjector>(&self, tracker: &mut Tracker<I>) -> Result<bool, InjectionError> {
        match self {
            Self::Mouse { button } => tracker.chord(&[Control::Mouse(*button)])?,
            Self::DoubleClick { button } => tracker.double_click(*button)?,
            Self::Wheel { steps, horizontal } => {
                tracker.event(Event::Wheel(steps * 120, *horizontal))?
            }
            Self::Keys { keys } => {
                let mut chord = [Control::Key(0); 5];
                for (control, key) in chord.iter_mut().zip(keys) {
                    *control = Control::Key(*key);
                }
                tracker.chord(&chord[..keys.len()])?;
            }
            Self::Pause => return Ok(true),
            _ => {}
        }
        Ok(false)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Binding {
    pub mode: Behavior,
    pub press: Action,
    pub click: Action,
    pub double: Action,
    pub long: Action,
    pub hold: Action,
}
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Behavior {
    #[default]
    Native,
    Press,
    Gestures,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MappingProfile {
    pub name: String,
    pub buttons: [Binding; BUTTONS.len()],
}
impl Default for MappingProfile {
    fn default() -> Self {
        use Action::*;
        let keys = |keys: &[u16]| Keys {
            keys: keys.to_vec(),
        };
        let actions = [
            Mouse {
                button: MouseButton::Left,
            },
            Mouse {
                button: MouseButton::Right,
            },
            keys(&[0x12, 0x25]),
            keys(&[0x5b, 9]),
            keys(&[0x5b, 0x44]),
            keys(&[0x5b]),
            keys(&[0x11, 0x10, 9]),
            keys(&[0x11, 9]),
            keys(&[0x26]),
            keys(&[0x28]),
            keys(&[0x25]),
            keys(&[0x27]),
            Mouse {
                button: MouseButton::Middle,
            },
            None,
            None,
            None,
        ];
        Self {
            name: "Xbox 桌面".into(),
            buttons: actions.map(|press| Binding {
                mode: if press.can_hold() {
                    Behavior::Native
                } else {
                    Behavior::Press
                },
                press,
                ..Binding::default()
            }),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Gesture {
    Pressed,
    Released,
    Click,
    DoubleClick,
    LongPress,
    Hold(bool),
}

#[derive(Default)]
pub struct ButtonGestureEngine {
    down_since: Option<Instant>,
    click_until: Option<Instant>,
    second: bool,
    long_fired: bool,
}
impl ButtonGestureEngine {
    pub fn update(
        &mut self,
        down: bool,
        now: Instant,
        long: Option<Duration>,
        double: Option<Duration>,
        mut emit: impl FnMut(Gesture),
    ) {
        if self.click_until.is_some_and(|until| now >= until) && !self.second {
            self.click_until = None;
            emit(Gesture::Click);
        }
        if down && self.down_since.is_none() {
            self.second = self.click_until.is_some();
            self.down_since = Some(now);
            self.long_fired = false;
            emit(Gesture::Pressed);
        }
        if let Some(since) = self.down_since {
            if !self.long_fired && long.is_some_and(|duration| now - since >= duration) {
                if self.click_until.take().is_some() {
                    emit(Gesture::Click);
                }
                self.second = false;
                self.long_fired = true;
                emit(Gesture::LongPress);
                if down {
                    emit(Gesture::Hold(true));
                }
            }
            if !down {
                self.down_since = None;
                emit(Gesture::Released);
                if self.long_fired {
                    emit(Gesture::Hold(false));
                } else if self.second {
                    self.click_until = None;
                    self.second = false;
                    emit(Gesture::DoubleClick);
                } else if let Some(duration) = double {
                    self.click_until = Some(now + duration);
                } else {
                    emit(Gesture::Click);
                }
            }
        }
    }
}

#[derive(Default)]
pub struct MappingEngine {
    gestures: [ButtonGestureEngine; BUTTONS.len()],
    held: u16,
    blocked: u16,
}
impl MappingEngine {
    pub fn reset(&mut self, buttons: u16) {
        *self = Self {
            blocked: buttons,
            ..Self::default()
        };
    }
    pub fn tick<I: InputInjector>(
        &mut self,
        pad: Pad,
        profile: &MappingProfile,
        now: Instant,
        long: Duration,
        double: Duration,
        tracker: &mut Tracker<I>,
    ) -> Result<bool, InjectionError> {
        self.blocked &= pad.buttons;
        let mut pause = false;
        for (i, binding) in profile.buttons.iter().enumerate() {
            let mask = 1 << i;
            let down = pad.button(i) && self.blocked & mask == 0;
            let advanced = binding.mode == Behavior::Gestures;
            let long = (advanced && (binding.long != Action::None || binding.hold != Action::None))
                .then_some(long);
            let double = (advanced && binding.double != Action::None).then_some(double);
            let mut result = Ok(());
            self.gestures[i].update(down, now, long, double, |event| {
                if result.is_err() || pause {
                    return;
                }
                let action = match (binding.mode, event) {
                    (Behavior::Native, Gesture::Pressed) => Some((&binding.press, Some(true))),
                    (Behavior::Native, Gesture::Released) => Some((&binding.press, Some(false))),
                    (Behavior::Press, Gesture::Pressed) => Some((&binding.press, None)),
                    (Behavior::Gestures, Gesture::Click) => Some((&binding.click, None)),
                    (Behavior::Gestures, Gesture::DoubleClick) => Some((&binding.double, None)),
                    (Behavior::Gestures, Gesture::LongPress) => Some((&binding.long, None)),
                    (Behavior::Gestures, Gesture::Hold(down)) => Some((&binding.hold, Some(down))),
                    _ => None,
                };
                if let Some((action, hold)) = action {
                    if let Some(down) = hold {
                        if down {
                            self.held |= mask;
                        } else {
                            self.held &= !mask;
                        }
                        if let Some(control) = action.control() {
                            result = tracker.hold(control, down);
                        }
                    } else {
                        result = action.tap(tracker).map(|requested| pause = requested);
                    }
                }
            });
            result?;
            if pause {
                break;
            }
        }
        Ok(pause)
    }
    pub fn speed_scale(&self, profile: &MappingProfile, settings: &MouseSettings) -> f64 {
        let mut fast = false;
        let mut remaining = self.held;
        while remaining != 0 {
            let i = remaining.trailing_zeros() as usize;
            remaining &= !(1 << i);
            let binding = &profile.buttons[i];
            let action = if binding.mode == Behavior::Gestures {
                &binding.hold
            } else {
                &binding.press
            };
            match action {
                Action::Precision => return settings.precision_multiplier,
                Action::Fast => fast = true,
                _ => {}
            }
        }
        if fast { settings.fast_multiplier } else { 1. }
    }
}
