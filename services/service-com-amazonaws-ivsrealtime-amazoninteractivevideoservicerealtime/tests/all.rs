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
#[path = "all/empty_response_framing.rs"]
mod empty_response_framing;
#[path = "all/encoder_configurations.rs"]
mod encoder_configurations;
#[path = "all/error_envelope.rs"]
mod error_envelope;
#[path = "all/ingest_configurations.rs"]
mod ingest_configurations;
#[path = "all/participant_replication.rs"]
mod participant_replication;
#[path = "all/participants.rs"]
mod participants;
#[path = "all/signing_name_routing.rs"]
mod signing_name_routing;
#[path = "all/stages.rs"]
mod stages;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/storage_configurations.rs"]
mod storage_configurations;
#[path = "all/tagging.rs"]
mod tagging;
#[path = "all/wire_probe.rs"]
mod wire_probe;
