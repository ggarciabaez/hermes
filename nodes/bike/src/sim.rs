use hermes_types::bike::{BikeConfig, Twist};
use libm::{cosf, sinf, tanf};
use nalgebra::{Matrix3, Vector3, vector};
use std::time;

pub struct Bicycle {
    pub x: Vector3<f32>,
    pub cfg: BikeConfig,
    vprev: f32,
    pub t: time::Duration,
    pub x_hist: Vec<Vector3<f32>>,
}

impl std::ops::Deref for Bicycle {
    type Target = BikeConfig;
    fn deref(&self) -> &Self::Target {
        &self.cfg
    }
}

impl Bicycle {
    /// TODO: add covariance-based noise using statrs
    pub fn from_cfg(x0: Vector3<f32>, cfg: BikeConfig) -> Self {
        Self {
            x: x0,
            cfg,
            vprev: 0.0_f32,
            t: time::Duration::from_secs(0),
            x_hist: vec![x0]
        }
    }

    pub fn new(
        x0: Vector3<f32>,
        vmax: f32,
        amax: f32,
        smax: f32,
        l: f32,
        dt: time::Duration,
    ) -> Self {
        Self {
            x: x0,
            cfg: BikeConfig {
                dt,
                max_v: vmax,
                max_a: amax,
                max_steer: smax,
                l,
                understeer: 0.05,
            },
            vprev: 0.0_f32,
            t: time::Duration::from_secs(0),
            x_hist: vec![x0],
        }
    }

    pub fn new_default(vmax: f32) -> Self {
        Self {
            x: Vector3::zeros(),
            cfg: BikeConfig::default(vmax),
            vprev: 0.0_f32,
            t: time::Duration::from_secs(0),
            x_hist: vec![Vector3::zeros()],
        }
    }

    /// Step bike simulation by 1 timestep.
    /// Returns: (d_dist, d_angle) Odometry measurements
    pub fn step(&mut self, u: Twist) -> (f32, f32) {
        // TODO: add noise option
        let u = self.limit_input(&u);
        self.vprev = u.v;
        let dx = self.dt.as_secs_f32() * self.deriv(&self.x, u);
        self.x += dx;
        self.x_hist.push(self.x);
        self.t += self.dt;
        (libm::hypotf(dx.x, dx.y), dx.z)
    }

    /// To use with odometry measurements.
    /// u = (dd: f32, dth: f32) where
    /// dd is traveled distance, and dth is change in rotation.
    /// ENSURE THIS IS THE ACTUAL FUNCTION YOU WANT. THIS DOES NOT USE A TWIST FOR A REASON (right?)
    pub fn f_odom(x: &Vector3<f32>, u: (f32, f32)) -> Vector3<f32> {
        x + vector![u.0 * cosf(x.z), u.0 * sinf(x.z), u.1]
    }

    pub fn f_input(&self, x: &Vector3<f32>, u: Twist) -> Vector3<f32> {
        self.dt.as_secs_f32() * self.deriv(x, u)
    }

    /// deriv WILL NOT CLEAN YOUR INPUTS.
    /// Make sure your control inputs are safe to use with the limit_input method.
    pub fn deriv(&self, x: &Vector3<f32>, u: Twist) -> Vector3<f32> {
        let theta = x.z;
        let eff_l = self.l + (self.cfg.understeer * u.v.powi(2));

        // Convert twist to steering angle
        let steer = u.to_steering(self.cfg.l);

        let mot_norm: Vector3<f32> = vector![cosf(theta), sinf(theta), tanf(steer) / eff_l];
        mot_norm * u.v
    }

    pub fn limit_input(&self, u: &Twist) -> Twist {
        let mut t = Twist::new(self.limits_va(u.v), u.w);
        let y_clamped = u.to_steering(self.cfg.l).clamp(-self.max_steer, self.max_steer);

        t.w = t.v * y_clamped.tan() / self.cfg.l;
        t
    }

    fn limits_va(&self, v: f32) -> f32 {
        let vp = self.vprev;
        let delta = self.dt.as_secs_f32();
        let mut _v = v;
        if (v - vp) / delta > self.max_a {
            _v = vp + self.max_a * delta;
        } else if (v - vp) / delta < -self.max_a {
            _v = vp - self.max_a * delta;
        }
        _v.clamp(-self.max_v, self.max_v)
    }

    pub fn jac_x(&self, u: Twist) -> Matrix3<f32> {
        let mut j = Matrix3::identity();
        j.m13 = -u.v * sinf(self.x.z);
        j.m23 = u.v * cosf(self.x.z);
        j
    }

    pub fn xy(&self) -> [f32; 2] {
        [self.x.x, self.x.y]
    }
}