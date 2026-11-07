use super::{receiver::Sink, win_input, win_monitor};
use crate::{
    config::Edge,
    keys::windows_key,
    wire::{Input, Mode},
};
use anyhow::{Context, Result, ensure};
use std::{collections::BTreeSet, mem::size_of};
use windows_sys::Win32::{
    Foundation::POINT,
    UI::{
        HiDpi::{DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext},
        Input::KeyboardAndMouse::*,
        WindowsAndMessaging::GetCursorPos,
    },
};

pub struct NativeSink {
    active: bool,
    edge: Option<Edge>,
    return_edge: bool,
    keys: BTreeSet<u16>,
    buttons: BTreeSet<u16>,
    inject: fn(Input) -> Result<()>,
    pointer: Option<super::win_cursor::Overlay>,
    pointer_failed: bool,
    cursor_setting: tokio::sync::watch::Receiver<bool>,
}

impl NativeSink {
    pub fn new(mut edge: Option<Edge>) -> Result<Self> {
        let dpi =
            unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
        ensure!(
            dpi != 0 || edge.is_none(),
            "cannot enable per-monitor DPI awareness for the return edge"
        );
        if let Some(edge) = &mut edge
            && edge.screen == "auto"
        {
            edge.screen = win_monitor::screens()?
                .into_iter()
                .next()
                .context("No screen is available")?
                .name;
        }
        Ok(Self {
            active: false,
            edge,
            return_edge: false,
            keys: BTreeSet::new(),
            buttons: BTreeSet::new(),
            inject,
            pointer: None,
            pointer_failed: false,
            cursor_setting: crate::lifecycle::remote_cursor(),
        })
    }

    fn validate_edge(&self, position: Option<u16>, mode: Mode, check_screen: bool) -> Result<()> {
        if position.is_some() {
            let edge = self
                .edge
                .as_ref()
                .context("configure the receiver's return edge")?;
            ensure!(
                edge.mode == mode,
                "edge modes differ between host and receiver"
            );
            if check_screen {
                ensure!(
                    win_monitor::screens()?
                        .iter()
                        .filter(|screen| screen.name.eq_ignore_ascii_case(&edge.screen))
                        .count()
                        == 1,
                    "the selected Windows return screen is unavailable or ambiguous"
                );
            }
        }
        Ok(())
    }

    fn receiving_mouse(&mut self) {
        // Unit tests use an injected sink and must never create a native window.
        if cfg!(test) || !*self.cursor_setting.borrow() {
            return;
        }
        if self.pointer.is_none() && !self.pointer_failed {
            match super::win_cursor::Overlay::start() {
                Ok(pointer) => self.pointer = Some(pointer),
                Err(error) => {
                    self.pointer_failed = true;
                    eprintln!("Windows pointer fallback could not start: {error:#}");
                }
            }
        }
        if let Some(pointer) = &self.pointer {
            pointer.active(true);
            pointer.refresh();
        }
    }
}

fn encode(event: Input) -> Result<INPUT> {
    let mut input = INPUT::default();
    match event {
        Input::Key { code, down } => {
            let (scan, extended, virtual_key) =
                windows_key(code).context("unsupported Windows physical key code")?;
            let mut flags = if virtual_key { 0 } else { KEYEVENTF_SCANCODE };
            if extended {
                flags |= KEYEVENTF_EXTENDEDKEY;
            }
            if !down {
                flags |= KEYEVENTF_KEYUP;
            }
            input.r#type = INPUT_KEYBOARD;
            input.Anonymous.ki = KEYBDINPUT {
                wVk: if virtual_key { scan } else { 0 },
                wScan: if virtual_key { 0 } else { scan },
                dwFlags: flags,
                ..Default::default()
            };
        }
        event => {
            let mut mouse = MOUSEINPUT::default();
            match event {
                Input::Move { dx, dy } => {
                    // SendInput queues motion. Reading GetCursorPos for every delta
                    // can repeatedly observe the same old position and lose distance.
                    // Native relative motion preserves the complete input stream and
                    // uses this PC's normal pointer speed and acceleration settings.
                    mouse.dx = dx;
                    mouse.dy = dy;
                    mouse.dwFlags = MOUSEEVENTF_MOVE;
                }
                Input::Button { code, down } => {
                    mouse.dwFlags = match (code, down) {
                        (272, true) => MOUSEEVENTF_LEFTDOWN,
                        (272, false) => MOUSEEVENTF_LEFTUP,
                        (273, true) => MOUSEEVENTF_RIGHTDOWN,
                        (273, false) => MOUSEEVENTF_RIGHTUP,
                        (274, true) => MOUSEEVENTF_MIDDLEDOWN,
                        (274, false) => MOUSEEVENTF_MIDDLEUP,
                        (275 | 276, true) => MOUSEEVENTF_XDOWN,
                        (275 | 276, false) => MOUSEEVENTF_XUP,
                        _ => anyhow::bail!("unsupported Windows mouse button"),
                    };
                    if code >= 275 {
                        mouse.mouseData = if code == 275 { 1 } else { 2 };
                    }
                }
                Input::Scroll { dx, dy } => {
                    ensure!(dx == 0 || dy == 0, "encode each scroll axis separately");
                    if dy != 0 {
                        mouse.dwFlags = MOUSEEVENTF_WHEEL;
                        mouse.mouseData = (-i32::from(dy)) as u32;
                    } else {
                        mouse.dwFlags = MOUSEEVENTF_HWHEEL;
                        mouse.mouseData = i32::from(dx) as u32;
                    }
                }
                _ => unreachable!(),
            }
            input.r#type = INPUT_MOUSE;
            input.Anonymous.mi = mouse;
        }
    }
    Ok(input)
}

fn inject(event: Input) -> Result<()> {
    if let Input::Scroll { dx, dy } = event
        && dx != 0
        && dy != 0
    {
        inject(Input::Scroll { dx, dy: 0 })?;
        return inject(Input::Scroll { dx: 0, dy });
    }
    let input = encode(event)?;
    ensure!(
        unsafe { SendInput(1, &input, size_of::<INPUT>() as i32) } == 1,
        "Windows rejected SendInput; check the interactive desktop and application privileges"
    );
    Ok(())
}

impl Sink for NativeSink {
    async fn poll(&mut self) -> Result<()> {
        self.cursor_setting
            .changed()
            .await
            .context("cursor setting channel closed")?;
        let enabled = *self.cursor_setting.borrow_and_update();
        if !enabled {
            self.pointer = None;
            self.pointer_failed = false;
        } else if self.active {
            self.receiving_mouse();
        }
        Ok(())
    }

    fn prepare(&self, edge: Option<u16>, mode: Mode) -> Result<()> {
        ensure!(!self.active, "Windows input is already active");
        self.validate_edge(edge, mode, mode == Mode::Input)
    }

    fn activate(&mut self, position: Option<u16>, mode: Mode) -> Result<()> {
        self.validate_edge(position, mode, true)?;
        if let Some(position) = position {
            win_input::enter(self.edge.as_ref().unwrap(), position)?;
        }
        self.return_edge = position.is_some();
        self.active = true;
        self.pointer_failed = false;
        Ok(())
    }

    fn event(&mut self, event: Input) -> Result<Option<u16>> {
        ensure!(
            self.active && event.valid(),
            "invalid or inactive Windows input event"
        );
        if let Input::Move { dx, dy } = event
            && self.return_edge
            && self.keys.is_empty()
            && self.buttons.is_empty()
        {
            let edge = self.edge.as_ref().unwrap();
            let screens = win_monitor::screens()?;
            let screen = screens
                .iter()
                .find(|screen| screen.name.eq_ignore_ascii_case(&edge.screen))
                .context("the Windows return screen disappeared")?;
            let mut cursor = POINT::default();
            ensure!(
                unsafe { GetCursorPos(&mut cursor) } != 0,
                "cannot read the Windows cursor"
            );
            if let Some(position) = edge.side.crossing(
                screen.rect,
                cursor.x.into(),
                cursor.y.into(),
                dx.into(),
                dy.into(),
            ) {
                return Ok(Some(position));
            }
        }
        match event {
            Input::Key { code, down } => {
                if down == self.keys.contains(&code) {
                    return Ok(None);
                }
                (self.inject)(event)?;
                if down {
                    self.keys.insert(code);
                } else {
                    self.keys.remove(&code);
                }
            }
            Input::Button { code, down } => {
                if down == self.buttons.contains(&code) {
                    return Ok(None);
                }
                (self.inject)(event)?;
                if down {
                    self.buttons.insert(code);
                } else {
                    self.buttons.remove(&code);
                }
            }
            _ => (self.inject)(event)?,
        }
        if !matches!(event, Input::Key { .. }) {
            self.receiving_mouse();
        }
        Ok(None)
    }

    fn release(&mut self) -> Result<()> {
        if let Some(pointer) = &self.pointer {
            pointer.active(false);
        }
        let mut result = Ok(());
        for code in std::mem::take(&mut self.keys) {
            result = result.and((self.inject)(Input::Key { code, down: false }));
        }
        for code in std::mem::take(&mut self.buttons) {
            result = result.and((self.inject)(Input::Button { code, down: false }));
        }
        self.active = false;
        self.return_edge = false;
        result
    }
}

impl Drop for NativeSink {
    fn drop(&mut self) {
        let _ = self.release();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[test]
    fn receiver_releases_every_held_input_after_an_injection_failure() {
        static EVENTS: Mutex<Vec<Input>> = Mutex::new(Vec::new());
        fn fake(event: Input) -> Result<()> {
            EVENTS.lock().unwrap().push(event);
            ensure!(
                !matches!(
                    event,
                    Input::Key {
                        code: 31,
                        down: true
                    }
                ),
                "synthetic SendInput failure"
            );
            Ok(())
        }
        let mut sink = NativeSink {
            active: false,
            edge: None,
            return_edge: false,
            keys: BTreeSet::new(),
            buttons: BTreeSet::new(),
            inject: fake,
            pointer: None,
            pointer_failed: false,
            cursor_setting: tokio::sync::watch::channel(true).1,
        };
        sink.prepare(None, Mode::Input).unwrap();
        sink.activate(None, Mode::Input).unwrap();
        sink.event(Input::Key {
            code: 30,
            down: true,
        })
        .unwrap();
        sink.event(Input::Button {
            code: 272,
            down: true,
        })
        .unwrap();
        assert!(
            sink.event(Input::Key {
                code: 31,
                down: true
            })
            .is_err()
        );
        sink.release().unwrap();
        let events = EVENTS.lock().unwrap();
        assert!(events.contains(&Input::Key {
            code: 30,
            down: false
        }));
        assert!(events.contains(&Input::Button {
            code: 272,
            down: false
        }));
        assert!(!sink.active && sink.keys.is_empty() && sink.buttons.is_empty());
    }

    #[test]
    fn queued_motion_preserves_distance_and_button_order_without_cursor_readback() {
        let events = [
            Input::Move { dx: 7, dy: -3 },
            Input::Move { dx: -2, dy: 5 },
            Input::Move { dx: 100, dy: 20 },
            Input::Button {
                code: 272,
                down: true,
            },
            Input::Move { dx: -10, dy: -8 },
        ];
        // Encode the entire burst before the OS applies any event. In the old
        // path every absolute target could be based on the same stale cursor.
        let queued = events
            .into_iter()
            .map(encode)
            .collect::<Result<Vec<_>>>()
            .unwrap();
        let mut position = (-1920, 50);
        let mut button_position = None;
        for input in queued {
            assert_eq!(input.r#type, INPUT_MOUSE);
            let mouse = unsafe { input.Anonymous.mi };
            if mouse.dwFlags & MOUSEEVENTF_MOVE != 0 {
                assert_eq!(mouse.dwFlags & MOUSEEVENTF_ABSOLUTE, 0);
                position.0 += mouse.dx;
                position.1 += mouse.dy;
            } else if mouse.dwFlags & MOUSEEVENTF_LEFTDOWN != 0 {
                button_position = Some(position);
            }
        }
        assert_eq!(button_position, Some((-1815, 72)));
        assert_eq!(position, (-1825, 64));
    }
}
