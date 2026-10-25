// SPDX-License-Identifier: CC0-1.0

//! Single integration-test target for this crate.
//!
//! Put a new suite at `tests/all/<name>.rs` and declare it below, keeping the list
//! sorted.
//!
//! A suite that is not declared here compiles nowhere and its tests never run. A
//! suite left at the top level of `tests/` fails the build.

// `tests/all.rs` is a test-target CRATE ROOT, so a bare `mod` resolves to a SIBLING
// (`tests/state_roundtrip.rs`) — which the layout check refuses. The `#[path]` is what
// points it into `tests/all/`.
#[path = "all/decode_authorization_message.rs"]
mod decode_authorization_message;

#[path = "all/malformed_session_policy.rs"]
mod malformed_session_policy;

#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;

#[path = "all/sts_authorization.rs"]
mod sts_authorization;

#[path = "all/sts_sessions.rs"]
mod sts_sessions;

#[path = "all/trust_policy_convergence.rs"]
mod trust_policy_convergence;
