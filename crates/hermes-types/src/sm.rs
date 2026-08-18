use std::fmt;
use serde::{Deserialize, Serialize};
use crate::NodeName;

#[repr(u8)]
#[derive(Serialize, Deserialize, Debug, PartialEq, Eq, Clone, Copy)]
pub enum Mode {
    Standby,
    Manual,
    Auto,
    EStop,
    SWStop,
    Halt
}

impl fmt::Display for Mode {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "Mode::{:?}", self)
    }
}

// TODO: should we be allowed to halt from auto?
/// Checks if a transition is allowed
pub fn transition(curstate: Mode, newstate: Mode) -> bool {
    match (curstate, newstate) {
        (Mode::Standby, _) => true,
        (Mode::Manual, _) => true,
        (Mode::Auto, _) => true,
        (Mode::EStop, Mode::Halt) => true,
        (Mode::SWStop, Mode::Standby | Mode::Halt) => true,
        (Mode::Halt, _) => false,
        _ => false
    }
}

#[derive(Serialize, Deserialize, Debug)]
pub struct ModeRequest{
    pub mode: Mode,
    pub source: NodeName
}