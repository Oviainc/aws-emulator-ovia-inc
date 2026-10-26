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
#[path = "all/async_invoke_model_contract.rs"]
mod async_invoke_model_contract;
#[path = "all/bedrock_cloudwatch_request_metrics.rs"]
mod bedrock_cloudwatch_request_metrics;
#[path = "all/bedrock_cloudwatch_token_metrics.rs"]
mod bedrock_cloudwatch_token_metrics;
#[path = "all/bedrock_runtime_inference.rs"]
mod bedrock_runtime_inference;
#[path = "all/bedrock_runtime_request_claim.rs"]
mod bedrock_runtime_request_claim;
#[path = "all/bedrock_runtime_streaming.rs"]
mod bedrock_runtime_streaming;
#[path = "all/converse_tool_choice.rs"]
mod converse_tool_choice;
#[path = "all/guardrail_checks_inline.rs"]
mod guardrail_checks_inline;
#[path = "all/resource_reference_refusals.rs"]
mod resource_reference_refusals;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
