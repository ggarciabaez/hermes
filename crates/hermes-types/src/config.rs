//! Default zenoh session configuration shared by every hermes node.
//!
//! Transport/performance knobs (admin space, compression, ...) live in one
//! place so they don't drift between nodes. Nodes clone the result and tweak
//! as needed. Later an orchestrator may push per-node config over RPC instead;
//! until then this is the single source of truth.

use zenoh::Config;

/// Open the admin space on every node so tooling (e.g. Achilles) can query
/// per-node transport stats.
pub const ADMIN_SPACE_ENABLED: bool = true;

/// Enable payload compression on unicast and multicast links (requires the
/// `transport_compression` zenoh feature, which the workspace enables).
pub const COMPRESSION_ENABLED: bool = true;

/// The shared session configuration for hermes nodes.
///
/// Built from JSON5 so it stays partial: every key not mentioned here keeps
/// zenoh's default. Nodes may clone and override further.
pub fn default_config() -> Config {
    let cfg = format!(
        r#"{{
            adminspace: {{ enabled: {} }},
            transport: {{
                unicast: {{ compression: {{ enabled: {} }} }},
                multicast: {{ compression: {{ enabled: {} }} }},
            }},
        }}"#,
        ADMIN_SPACE_ENABLED, COMPRESSION_ENABLED, COMPRESSION_ENABLED
    );
    Config::from_json5(&cfg).expect("valid default config")
}