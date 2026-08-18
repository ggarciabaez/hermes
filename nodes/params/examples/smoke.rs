use std::time::Duration;
use hermes_types::params::{
    Bounds, Param, ParamError, ParamInfo, ParamValue, Tunable,
};
use hermes_types::rpc::query_with;
use hermes_types::{Node, NodeName, wait_for_liveliness};
use hermes_types::params::ParamsGetRequest;

struct GamepadParams {
    deadzone: Param<f64>,
    v_min: Param<f64>,
    v_max: Param<f64>,
}

impl GamepadParams {
    fn defaults() -> Self {
        Self {
            deadzone: Param {
                name: "deadzone".to_owned(),
                value: 0.1,
                bounds: Some(Bounds { low: 0.0, high: 0.5 }),
            },
            v_min: Param { name: "v_min".to_owned(), value: -1.0, bounds: None },
            v_max: Param { name: "v_max".to_owned(), value: 1.0, bounds: None },
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

#[tokio::main]
async fn main() {
    let session = zenoh::open(zenoh::Config::default()).await.unwrap();

    let constants = query_with::<ParamsGetRequest, Vec<(String, ParamValue)>>(
        &session,
        "robot/params/get",
        ParamsGetRequest::Constants,
        Duration::from_millis(500),
    )
    .await
    .expect("constants fetch failed");
    println!("constants: {:?}", constants.payload);

    // Simulate a param-owning node's boot load.
    let node = Node::new(&session, &NodeName::Gamepad).await.unwrap();
    assert!(wait_for_liveliness(&session, &NodeName::Params, Duration::from_millis(500)).await);
    let params_list = node.fetch_params().await.expect("fetch params");
    let mut params = GamepadParams::defaults();
    for (key, value) in params_list {
        params.set(&key, value).unwrap();
    }
    println!("boot-loaded deadzone: {:?}", params.get("deadzone"));
    println!("boot-loaded v_min: {:?}", params.get("v_min"));
    println!("list: {:?}", params.list());

    // Live tweak validation through the same set path.
    params.set("deadzone", ParamValue::Float(0.2)).unwrap();
    println!("after live tweak: {:?}", params.get("deadzone"));
    println!("out-of-range rejected: {:?}", params.set("deadzone", ParamValue::Float(9.0)));
    println!("type mismatch rejected: {:?}", params.set("deadzone", ParamValue::Bool(true)));
    println!("unknown key rejected: {:?}", params.set("nope", ParamValue::Float(1.0)));
}
