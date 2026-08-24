use botpad::{AsyncInputController, Button, Gilrs, InputController};
use tokio::sync::mpsc;
use hermes_types::params::{Bounds, Param, ParamError, ParamInfo, ParamValue, Tunable};

pub enum Command {
    Standby,
    Manual,
    Auto,
    Halt,
    EStop,
    Other,
}

#[macro_export]
macro_rules! set_triggers {
    (
        $ctrl:expr,
        $($btn:expr => $val:expr),+
        $(,)?
    ) => {
        $(
            $ctrl.on_press($btn, |_g| {Some($val)})?;
        )+
    };
}


#[derive(Debug)]
pub struct GamepadParams {
    pub(crate) deadzone: Param<f64>,
    pub v_min: Param<f64>,
    pub v_max: Param<f64>,
}

impl GamepadParams {
    pub fn defaults() -> Self {
        Self {
            deadzone: Param {
                name: "deadzone".to_owned(),
                value: 0.1,
                bounds: Some(Bounds { low: 0.0, high: 0.5 }),
            },
            v_min: Param {
                name: "v_min".to_owned(),
                value: -1.0,
                bounds: Some(Bounds { low: -1.0, high: 0.0 }),
            },
            v_max: Param {
                name: "v_max".to_owned(),
                value: 1.0,
                bounds: Some(Bounds { low: 0.0, high: 1.0 }),
            },
        }
    }
}

impl Tunable for GamepadParams {
    fn get(&self, key: &str) -> Option<ParamValue> {
        match key {
            "deadzone" => Some((*self.deadzone.get()).into()),
            "v_min" => Some((*self.v_min.get()).into()),
            "v_max" => Some((*self.v_max.get()).into()),
            _ => None,
        }
    }

    fn set(&mut self, key: &str, value: ParamValue) -> Result<(), ParamError> {
        match key {
            "deadzone" => self.deadzone.set(value.try_into()?),
            "v_min" => self.v_min.set(value.try_into()?),
            "v_max" => self.v_max.set(value.try_into()?),
            _ => Err(ParamError::Unknown(key.to_owned())),
        }
    }

    fn list(&self) -> Vec<ParamInfo> {
        vec![(&self.deadzone).into(), (&self.v_min).into(), (&self.v_max).into()]
    }
}

pub fn apply_deadzone(value: f32, deadzone: f64) -> f32 {
    if value.abs() < deadzone as f32 {
        0.0
    } else {
        value
    }
}


pub fn setup_controller(tx: mpsc::UnboundedSender<Command>, deadzone: f64) -> Result<AsyncInputController<Command>, String> {
    let gilrs = Gilrs::new().expect("Failed to initialize gilrs");
    let gpid = InputController::first_connected(&gilrs);

    let mut sync_controller = InputController::new(tx, gpid, Some(gilrs))?;

    // Deadzone is applied by this node from its own tunable, so botpad's
    // built-in one is disabled here to keep a single source of truth.
    sync_controller.deadzone = 0.0;
    println!("[params] deadzone: {}", deadzone);

    // One will be pressed first. Pressing the second one makes it actually trigger.
    // Thus guaranteeing it will be called once. Rarely it will call twice, but it's safely handled.
    sync_controller.on_press(Button::LeftTrigger, |gamepad| {
        if gamepad.is_pressed(Button::DPadLeft){
            return Some(Command::EStop)
        }
        None
    })?;
    sync_controller.on_press(Button::DPadLeft, |gamepad| {
        if gamepad.is_pressed(Button::LeftTrigger){
            return Some(Command::EStop)
        }
        None
    })?;

    // Other modes
    sync_controller.on_press(Button::Start, |_gamepad| {
        Some(Command::Manual)
    })?;
    sync_controller.on_press(Button::Select, |_gamepad| {
        Some(Command::Auto)
    })?;
    sync_controller.on_press(Button::Mode, |_gamepad| {
        Some(Command::Halt)
    })?;
    sync_controller.on_press(Button::RightThumb, |_gamepad| {
        Some(Command::Standby)
    })?;

    // to please clippy and also cause rumble
    sync_controller.on_press(Button::South, |_gamepad| {
        Some(Command::Other)
    })?;

    // 3. Wrap it in the Async controller (spawns a 16ms background polling task)
    Ok(AsyncInputController::new(sync_controller, 16))
}