pub use botpad::ControllerState;
use serde::{Serialize, Deserialize};

#[derive(Debug, Serialize, Deserialize, Clone, Copy)]
pub struct GPadDrive {
    pub v: f32,
    pub y: f32
}