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
#[path = "all/code_interpreter_sessions.rs"]
mod code_interpreter_sessions;
#[path = "all/code_interpreter_tools.rs"]
mod code_interpreter_tools;
#[path = "all/memory_events.rs"]
mod memory_events;
#[path = "all/memory_ingestion.rs"]
mod memory_ingestion;
#[path = "all/memory_records.rs"]
mod memory_records;
#[path = "all/on_demand_evaluation.rs"]
mod on_demand_evaluation;
#[path = "all/service_claim.rs"]
mod service_claim;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/workload_access_tokens.rs"]
mod workload_access_tokens;
