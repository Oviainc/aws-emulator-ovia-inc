// SPDX-License-Identifier: CC0-1.0

//! Intentionally empty, and declared by nothing.
//!
//! This file held an earlier draft of this change: a pre-dispatch stage that refused a
//! telemetry-pipeline mutation whose `PipelineIdentifier` was not an ARN. That rule was
//! wrong — whether a request is authorized does not depend on the FORM of a payload
//! member, and the identifier of those operations resolves through the one name-or-ARN
//! lookup every pipeline-scoped operation goes through
//! (`crate::state::ComAmazonawsObservabilityadminObservabilityadminState::resolve_pipeline_key`).
//! The authorization pre-check this service does state lives in `super::front_door`, and
//! is independent of everything a request's payload says.
