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
#[path = "all/arn_read_precondition.rs"]
mod arn_read_precondition;
#[path = "all/contact_channel_deactivation.rs"]
mod contact_channel_deactivation;
#[path = "all/contact_channel_name_uniqueness.rs"]
mod contact_channel_name_uniqueness;
#[path = "all/contact_lifecycle.rs"]
mod contact_lifecycle;
#[path = "all/contact_updates.rs"]
mod contact_updates;
#[path = "all/cross_service_onboarding.rs"]
mod cross_service_onboarding;
#[path = "all/data_plane_precondition.rs"]
mod data_plane_precondition;
#[path = "all/engagement_lifecycle.rs"]
mod engagement_lifecycle;
#[path = "all/onboarding.rs"]
mod onboarding;
#[path = "all/page_acknowledgement.rs"]
mod page_acknowledgement;
#[path = "all/page_receipts.rs"]
mod page_receipts;
#[path = "all/page_resolution.rs"]
mod page_resolution;
#[path = "all/replication_set_precondition.rs"]
mod replication_set_precondition;
#[path = "all/rotation_deletes.rs"]
mod rotation_deletes;
#[path = "all/rotation_override_listing.rs"]
mod rotation_override_listing;
#[path = "all/rotation_overrides.rs"]
mod rotation_overrides;
#[path = "all/rotation_reads.rs"]
mod rotation_reads;
#[path = "all/rotation_start_time.rs"]
mod rotation_start_time;
#[path = "all/rotation_updates.rs"]
mod rotation_updates;
#[path = "all/start_engagement.rs"]
mod start_engagement;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
