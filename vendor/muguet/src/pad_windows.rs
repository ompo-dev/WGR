// muguet - pad.rs
// Constantin Clerc (c22dev) - 0.1

use crate::input;
use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};
use windows_sys::Win32::UI::Input::XboxController::{XInputGetState, XINPUT_STATE};

pub static ACTIVE: AtomicBool = AtomicBool::new(false);
pub static FOCUSED: AtomicBool = AtomicBool::new(true);

pub static IN_VEHICLE: AtomicBool = AtomicBool::new(false);

pub const MODE_FOOT: u8 = 0;
pub const MODE_CAR: u8 = 1;
pub const MODE_BIKE: u8 = 2;
pub const MODE_HELI: u8 = 3;
pub const MODE_PLANE: u8 = 4;
pub const MODE_BOAT: u8 = 5;
pub const MODE_SUB: u8 = 6;
pub const MODE_BICYCLE: u8 = 7;
pub const MODE_PASSENGER: u8 = 8;
pub static VEH_MODE: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(MODE_FOOT);

pub const CTX_TOWING: u32 = 1;
pub const CTX_TRAILER: u32 = 2;
pub const CTX_CARGOBOB: u32 = 4;
pub const CTX_ROPE: u32 = 8;
pub const CTX_CONVERTIBLE: u32 = 16;
pub const CTX_VTOL: u32 = 32;
pub const CTX_GUN: u32 = 64;
pub const CTX_MELEE: u32 = 128;
pub const CTX_THROWN: u32 = 256;
pub const CTX_SWIM: u32 = 512;
pub const CTX_SNIPER: u32 = 1024;
pub static CONTEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

pub static INJECT: Mutex<Vec<(i32, f32)>> = Mutex::new(Vec::new());
pub static RADIO_WHEEL: Mutex<Option<(f32, f32)>> = Mutex::new(None);
pub const FRONTEND_CONTROL: i32 = 2;
pub const RADIO_WHEEL_UD: i32 = 332;
pub const RADIO_WHEEL_LR: i32 = 333;

#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(not(target_os = "ios"), allow(dead_code))]
pub enum Request {
    DropTow,
    DropTrailer,
    Rope,
}
pub static REQUESTS: Mutex<Vec<Request>> = Mutex::new(Vec::new());
#[cfg_attr(not(target_os = "ios"), allow(dead_code))]
pub fn request(r: Request) {
    REQUESTS.lock().unwrap().push(r);
}

static MENU: AtomicBool = AtomicBool::new(false);
static SCRIPT_TICK: Mutex<Option<Instant>> = Mutex::new(None);
pub fn script_tick(menu: bool) {
    MENU.store(menu, Ordering::Relaxed);
    *SCRIPT_TICK.lock().unwrap() = Some(Instant::now());
}
#[cfg_attr(not(target_os = "ios"), allow(dead_code))]
pub fn in_menu() -> bool {
    MENU.load(Ordering::Relaxed)
        || SCRIPT_TICK
            .lock()
            .unwrap()
            .is_some_and(|t| t.elapsed() > Duration::from_millis(500))
}

pub static HORN_PAD: AtomicBool = AtomicBool::new(false);
pub static HORN_TOUCH: AtomicBool = AtomicBool::new(false);
pub fn horn_held() -> bool {
    HORN_PAD.load(Ordering::Relaxed) || HORN_TOUCH.load(Ordering::Relaxed)
}

struct State {
    held: HashSet<&'static str>,
    buttons: u8,
    last: Instant,
}
static STATE: Mutex<Option<State>> = Mutex::new(None);
pub fn poll() -> bool {
    if !FOCUSED.load(Ordering::Relaxed) {
        release_all();
        return false;
    }
    let mut state: XINPUT_STATE = unsafe { std::mem::zeroed() };
    if unsafe { XInputGetState(0, &mut state) } != 0 {
        ACTIVE.store(false, Ordering::Relaxed);
        release_all();
        return false;
    }
    ACTIVE.store(true, Ordering::Relaxed);
    let gp = state.Gamepad;
    let down = |mask| gp.wButtons & mask != 0;
    let (lx, ly) = (gp.sThumbLX as f32 / 32768.0, gp.sThumbLY as f32 / 32768.0);
    let in_car = IN_VEHICLE.load(Ordering::Relaxed);
    let mut keys = HashSet::new();
    let mut buttons = 0u8;
    if lx < -0.3 {
        keys.insert("KeyA");
    }
    if lx > 0.3 {
        keys.insert("KeyD");
    }
    if in_car {
        if gp.bRightTrigger > 51 {
            keys.insert("KeyW");
        }
        if gp.bLeftTrigger > 51 {
            keys.insert("KeyS");
        }
        if ly > 0.3 {
            keys.insert("Numpad8");
        }
        if ly < -0.3 {
            keys.insert("Numpad2");
        }
        if down(0x0200) {
            keys.insert("Space");
        }
    } else {
        if ly > 0.3 {
            keys.insert("KeyW");
        }
        if ly < -0.3 {
            keys.insert("KeyS");
        }
        if gp.bRightTrigger > 51 {
            buttons |= 1;
        }
        if gp.bLeftTrigger > 51 {
            buttons |= 2;
        }
        if down(0x1000) {
            keys.insert("ShiftLeft");
        }
        if down(0x0200) {
            keys.insert("KeyQ");
        }
        if down(0x0040) {
            keys.insert("ControlLeft");
        }
    }
    for (mask, key) in [
        (0x1000, "Enter"),
        (0x2000, "KeyR"),
        (0x2000, "Backspace"),
        (0x8000, "KeyF"),
        (0x4000, "Space"),
        (0x0100, "Tab"),
        (0x0080, "KeyC"),
        (1, "ArrowUp"),
        (2, "ArrowDown"),
        (4, "ArrowLeft"),
        (8, "ArrowRight"),
        (0x0010, "Escape"),
        (0x0020, "KeyV"),
    ] {
        if down(mask) {
            keys.insert(key);
        }
    }
    HORN_PAD.store(in_car && down(0x0040), Ordering::Relaxed);
    let mut guard = STATE.lock().unwrap();
    let old = guard.get_or_insert_with(|| State {
        held: HashSet::new(),
        buttons: 0,
        last: Instant::now(),
    });
    let dt = old.last.elapsed().as_secs_f64().min(0.05);
    old.last = Instant::now();
    let curve = |v: i16| {
        let v = v as f64 / 32768.0;
        if v.abs() < 0.12 {
            0.0
        } else {
            ((v.abs() - 0.12) / 0.88).powf(1.8).copysign(v)
        }
    };
    input::mouse_delta_f(
        curve(gp.sThumbRX) * 700.0 * input::look_sens() * dt,
        -curve(gp.sThumbRY) * 700.0 * input::look_sens() * dt,
    );
    for key in old.held.difference(&keys) {
        input::key(key, false, false, None, false);
    }
    for key in keys.difference(&old.held) {
        crate::mp::key_pressed(key);
        input::key(key, true, false, None, false);
    }
    for (bit, button) in [(1, 0), (2, 2)] {
        if (buttons ^ old.buttons) & bit != 0 {
            input::button(button, buttons & bit != 0);
        }
    }
    old.held = keys;
    old.buttons = buttons;
    true
}
pub fn release_all() {
    HORN_PAD.store(false, Ordering::Relaxed);
    if let Some(state) = STATE.lock().unwrap().take() {
        for key in state.held {
            input::key(key, false, false, None, false);
        }
        if state.buttons & 1 != 0 {
            input::button(0, false);
        }
        if state.buttons & 2 != 0 {
            input::button(2, false);
        }
    }
}
