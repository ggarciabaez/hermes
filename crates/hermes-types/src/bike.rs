use libm::atanf;
use std::f32::consts::PI;
use std::time;
use serde::{Serialize, Deserialize};
pub fn normalize_rot(v: f32) -> f32 {
    (v + PI) % (PI * 2.0) - PI
}

#[derive(Clone, Copy, Debug)]
pub struct BikeConfig {
    pub dt: time::Duration,
    pub max_v: f32,
    pub max_a: f32,
    pub max_steer: f32,
    pub l: f32,
    pub understeer: f32,
}
impl BikeConfig {
    pub fn default(vmax: f32) -> Self {
        Self {
            max_v: vmax,
            ..Default::default()
        }
    }
}
impl Default for BikeConfig {
    fn default() -> Self {
        Self {
            dt: time::Duration::try_from_secs_f32(0.01667).unwrap(),
            max_v: 10.0,
            max_a: 5.0,
            max_steer: 22.5_f32 * PI / 180.0,
            l: 0.324,
            understeer: 0.01,
        }
    }
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Clone, Copy, Default)]
pub struct Twist {
    pub v: f32,  // Linear velocity
    pub w: f32  // Angular velocity
}

impl Twist {
    pub fn new(v: f32, w: f32) -> Self {
        Self { v, w }
    }

    pub fn zero() -> Self {
        Self { v: 0.0, w: 0.0 }
    }

    pub fn is_moving(&self, e: f32) -> bool {
        self.v.abs() > e
    }

    /// Converts angular velocity from a twist message to a steering angle for the bike model.
    /// The math:
    /// w = dtheta = v*tan(y)/L
    /// y = atan(wL/v)
    /// Where:
    /// - w: angular velocity in rad/s
    /// - v: linear velocity in m/s
    /// - L: Wheelbase in meters
    /// - y: Steering angle in rad (?)
    pub fn to_steering(&self, l: f32) -> f32 {
        atanf(self.w * l / self.v)
    }
}

/*
pub struct Twist3 {
    pub linear: Vector3<f32>,
    pub angular: Vector3<f32>
}
*/

