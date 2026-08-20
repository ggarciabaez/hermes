use postcard::from_bytes;
use serde::de::DeserializeOwned;
use zenoh::bytes::ZBytes;

use hermes_types::gpad::GPadDrive;
use hermes_types::params::ParamInfo;
use hermes_types::sm::Mode;

/// Decode a pub/sub sample payload into a human-readable string.
///
/// The manifest is untyped, so Achilles maps known key exprs to the concrete
/// hermes-types they carry. Anything unknown falls back to a hex dump.
pub fn decode(key_expr: &str, payload: &ZBytes) -> String {
    let bytes = payload.to_bytes();
    match key_expr {
        "robot/state_machine/current" => try_decode::<Mode>(&bytes),
        "robot/gamepad/state" => try_decode::<botpad::ControllerState>(&bytes),
        "robot/gamepad/drive" => try_decode::<GPadDrive>(&bytes),
        _ => {
            if key_expr.ends_with("/params/changed") {
                try_decode::<ParamInfo>(&bytes)
            } else {
                fallback(&bytes)
            }
        }
    }
}

fn try_decode<T: DeserializeOwned + std::fmt::Debug>(bytes: &[u8]) -> String {
    match from_bytes::<T>(bytes) {
        Ok(v) => format!("{:?}", v),
        Err(_) => fallback(bytes),
    }
}

fn fallback(bytes: &[u8]) -> String {
    if bytes.is_empty() {
        return format!("<unknown type> ({} bytes, empty)", bytes.len());
    }
    let hex: String = bytes.iter().map(|b| format!("{:02x}", b)).collect();
    format!(
        "<unknown type> ({} bytes)\nhex: {}",
        bytes.len(),
        hex.chars()
            .collect::<Vec<_>>()
            .chunks(64)
            .map(|c| c.iter().collect::<String>())
            .collect::<Vec<_>>()
            .join("\n     ")
    )
}