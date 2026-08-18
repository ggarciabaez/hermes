#[macro_export]
/// A macro to quickly and nicely declare all the endpoints of a node.
/// # Usage example:
/// ```
/// setup_endpoints!(
///     node,
///     pubs: [
///         state_node = "state",
///         drive_node = "drive",
///         params_changed = "params/changed"
///     ],
///     queries: [
///         params_list = "params/list",
///         params_set = "params/set",
///         params_get = "params/get"
///     ]);
/// ```
macro_rules! setup_endpoints {
    (
        $node:ident,
        pubs: [ $( $p_var:ident = $p_path:expr ),* $(,)? ],
        queries: [ $( $q_var:ident = $q_path:expr ),* $(,)? ]
    ) => {
        $( let $p_var = $node.declare_pub($p_path).await.unwrap(); )*
        $( let $q_var = $node.declare_queryable($q_path).await.unwrap(); )*
    };
}
