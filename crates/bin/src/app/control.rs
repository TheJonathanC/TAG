//! App-loop side of the control plane: handles [`ControlEnvelope`]s forwarded by the control
//! server. Each handler runs a short critical section and replies; slow work is spawned and
//! replies when done.

#[allow(unused_imports)]
use crate::{app::*, cli::*, session_io::*, sockets::*, tmux::*};
use serde::de::DeserializeOwned;
use serde_json::Value;
use tag_control::{ControlEnvelope, ControlError, ControlResult};

/// Handle one control request and send its reply.
#[allow(clippy::match_single_binding)] // methods land in T1.3+
pub(crate) fn handle_control(state: &mut AppState, env: ControlEnvelope) {
    let ControlEnvelope {
        client,
        method,
        params,
        reply,
    } = env;
    let result: ControlResult = match method.as_str() {
        _ => Err(ControlError::new(
            tag_control::codes::METHOD_NOT_FOUND,
            format!("{method} is not implemented yet"),
        )),
    };
    let _ = (&client, &params, &state);
    reply.send(result);
}

/// Parse typed params, mapping errors to `InvalidParams`.
#[allow(dead_code)]
pub(crate) fn parse_params<T: DeserializeOwned>(params: Value) -> Result<T, ControlError> {
    serde_json::from_value(params).map_err(|e| ControlError::invalid_params(e.to_string()))
}
