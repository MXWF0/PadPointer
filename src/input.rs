use serde::{Deserialize, Serialize};
use windows::Win32::UI::Input::KeyboardAndMouse::*;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MouseButton {
    Left,
    Right,
    Middle,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Control {
    Mouse(MouseButton),
    Key(u16),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Event {
    Control(Control, bool),
    Move(i32, i32),
    Wheel(i32, bool),
}

pub trait InputInjector {
    /// Returns the number of events accepted, in order.
    fn send(&mut self, events: &[Event]) -> usize;
}

pub struct WindowsInjector;
impl InputInjector for WindowsInjector {
    fn send(&mut self, events: &[Event]) -> usize {
        let mut inputs = [INPUT::default(); 32];
        for (dst, event) in inputs.iter_mut().zip(events) {
            let mouse = match *event {
                Event::Control(Control::Key(vk), down) => {
                    let extended =
                        matches!(vk, 0x21..=0x28 | 0x2C..=0x2E | 0x5B..=0x5D | 0x6F | 0xA3 | 0xA5);
                    let mut flags = KEYBD_EVENT_FLAGS(0);
                    if !down {
                        flags |= KEYEVENTF_KEYUP;
                    }
                    if extended {
                        flags |= KEYEVENTF_EXTENDEDKEY;
                    }
                    *dst = INPUT {
                        r#type: INPUT_KEYBOARD,
                        Anonymous: INPUT_0 {
                            ki: KEYBDINPUT {
                                wVk: VIRTUAL_KEY(vk),
                                dwFlags: flags,
                                ..Default::default()
                            },
                        },
                    };
                    continue;
                }
                Event::Control(Control::Mouse(button), down) => {
                    let flags = match (button, down) {
                        (MouseButton::Left, true) => MOUSEEVENTF_LEFTDOWN,
                        (MouseButton::Left, false) => MOUSEEVENTF_LEFTUP,
                        (MouseButton::Right, true) => MOUSEEVENTF_RIGHTDOWN,
                        (MouseButton::Right, false) => MOUSEEVENTF_RIGHTUP,
                        (MouseButton::Middle, true) => MOUSEEVENTF_MIDDLEDOWN,
                        (MouseButton::Middle, false) => MOUSEEVENTF_MIDDLEUP,
                    };
                    MOUSEINPUT {
                        dwFlags: flags,
                        ..Default::default()
                    }
                }
                Event::Move(dx, dy) => MOUSEINPUT {
                    dx,
                    dy,
                    dwFlags: MOUSEEVENTF_MOVE,
                    ..Default::default()
                },
                Event::Wheel(amount, horizontal) => MOUSEINPUT {
                    mouseData: amount as u32,
                    dwFlags: if horizontal {
                        MOUSEEVENTF_HWHEEL
                    } else {
                        MOUSEEVENTF_WHEEL
                    },
                    ..Default::default()
                },
            };
            *dst = INPUT {
                r#type: INPUT_MOUSE,
                Anonymous: INPUT_0 { mi: mouse },
            };
        }
        unsafe { SendInput(&inputs[..events.len()], size_of::<INPUT>() as i32) as usize }
    }
}

#[derive(Debug, thiserror::Error)]
#[error("SendInput 仅接受 {accepted}/{requested} 个事件。前台窗口权限或安全桌面可能阻止输入。")]
pub struct InjectionError {
    accepted: usize,
    requested: usize,
}

pub struct Tracker<I> {
    injector: I,
    held: Vec<(Control, usize)>,
}
impl<I: InputInjector> Tracker<I> {
    pub fn new(injector: I) -> Self {
        Self {
            injector,
            held: Vec::with_capacity(16),
        }
    }
    pub fn event(&mut self, event: Event) -> Result<(), InjectionError> {
        self.send(&[event])
    }
    fn send(&mut self, events: &[Event]) -> Result<(), InjectionError> {
        let accepted = self.injector.send(events);
        for &event in &events[..accepted] {
            if let Event::Control(control, down) = event {
                if down {
                    self.held.push((control, 1));
                } else {
                    self.held.retain(|&(key, _)| key != control);
                }
            }
        }
        if accepted == events.len() {
            Ok(())
        } else {
            Err(InjectionError {
                accepted,
                requested: events.len(),
            })
        }
    }
    pub fn hold(&mut self, control: Control, down: bool) -> Result<(), InjectionError> {
        if let Some((_, count)) = self.held.iter_mut().find(|(key, _)| *key == control) {
            if down {
                *count += 1;
                return Ok(());
            }
            if *count > 1 {
                *count -= 1;
                return Ok(());
            }
        } else if !down {
            return Ok(());
        }
        self.event(Event::Control(control, down))
    }
    pub fn chord(&mut self, controls: &[Control]) -> Result<(), InjectionError> {
        let mut events = [Event::Move(0, 0); 16];
        let mut count = 0;
        for &control in controls {
            if !self.held.iter().any(|&(key, _)| key == control) {
                events[count] = Event::Control(control, true);
                count += 1;
            }
        }
        for index in 0..count {
            let Event::Control(control, _) = events[count - index - 1] else {
                unreachable!()
            };
            events[count + index] = Event::Control(control, false);
        }
        if count == 0 {
            Ok(())
        } else {
            self.send(&events[..count * 2])
        }
    }
    pub fn double_click(&mut self, button: MouseButton) -> Result<(), InjectionError> {
        let control = Control::Mouse(button);
        if self.held.iter().any(|&(key, _)| key == control) {
            return Ok(());
        }
        let down = Event::Control(control, true);
        let up = Event::Control(control, false);
        self.send(&[down, up, down, up])
    }
    pub fn release_all(&mut self) -> Result<(), InjectionError> {
        let mut failure = Ok(());
        for index in (0..self.held.len()).rev() {
            let control = self.held[index].0;
            if let Err(err) = self.event(Event::Control(control, false)) {
                failure = Err(err);
            }
        }
        failure
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct MouseSettings {
    pub deadzone: f64,
    pub response_exponent: f64,
    pub speed: f64,
    pub scroll_speed: f64,
    pub precision_multiplier: f64,
    pub fast_multiplier: f64,
    pub smoothing_ms: f64,
}
impl Default for MouseSettings {
    fn default() -> Self {
        Self {
            deadzone: 0.15,
            response_exponent: 1.6,
            speed: 1350.,
            scroll_speed: 15.,
            precision_multiplier: 0.35,
            fast_multiplier: 2.,
            smoothing_ms: 14.,
        }
    }
}
pub fn radial(stick: [f64; 2], deadzone: f64, exponent: f64) -> [f64; 2] {
    let radius = stick[0].hypot(stick[1]);
    if radius <= deadzone {
        return [0.; 2];
    }
    let gain = ((radius.min(1.) - deadzone) / (1. - deadzone)).powf(exponent) / radius;
    stick.map(|axis| axis * gain)
}

#[derive(Default)]
pub struct MouseEngine {
    smooth: [f64; 2],
    remainder: [f64; 2],
    scroll: [f64; 2],
}
impl MouseEngine {
    pub fn tick<I: InputInjector>(
        &mut self,
        pad: crate::gamepad::Pad,
        settings: &MouseSettings,
        seconds: f64,
        speed_scale: f64,
        injector: &mut Tracker<I>,
    ) -> Result<(), InjectionError> {
        let shaped = radial(pad.left, settings.deadzone, settings.response_exponent);
        let alpha = if settings.smoothing_ms == 0. {
            1.
        } else {
            1. - (-seconds * 1000. / settings.smoothing_ms).exp()
        };
        let mut pixels = [0; 2];
        if shaped == [0.; 2] {
            self.smooth = [0.; 2];
            self.remainder = [0.; 2];
        } else {
            for (smoothed, target) in self.smooth.iter_mut().zip(shaped) {
                *smoothed += (target - *smoothed) * alpha;
            }
            let acceleration = 0.65 + 0.35 * self.smooth[0].hypot(self.smooth[1]);
            for (axis, pixel) in pixels.iter_mut().enumerate() {
                self.remainder[axis] +=
                    self.smooth[axis] * settings.speed * acceleration * speed_scale * seconds;
                *pixel = self.remainder[axis].trunc() as i32;
                self.remainder[axis] -= f64::from(*pixel);
            }
        }
        if pixels != [0; 2] {
            injector.event(Event::Move(pixels[0], -pixels[1]))?;
        }
        let scroll = radial(pad.right, settings.deadzone, 1.35);
        for (axis, value) in scroll.into_iter().enumerate() {
            if value == 0. {
                self.scroll[axis] = 0.;
                continue;
            }
            self.scroll[axis] += value * settings.scroll_speed * seconds;
            let steps = self.scroll[axis].trunc() as i32;
            if steps != 0 {
                self.scroll[axis] -= f64::from(steps);
                injector.event(Event::Wheel(steps * 120, axis == 0))?;
            }
        }
        Ok(())
    }
}
