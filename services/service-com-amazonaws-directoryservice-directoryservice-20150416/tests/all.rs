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
#[path = "all/ad_assessment.rs"]
mod ad_assessment;
#[path = "all/certificates.rs"]
mod certificates;
#[path = "all/computers.rs"]
mod computers;
#[path = "all/conditional_forwarders.rs"]
mod conditional_forwarders;
#[path = "all/directory_data_access.rs"]
mod directory_data_access;
#[path = "all/directory_lifecycle.rs"]
mod directory_lifecycle;
#[path = "all/directory_limits.rs"]
mod directory_limits;
#[path = "all/directory_networking_and_settings.rs"]
mod directory_networking_and_settings;
#[path = "all/error_envelopes.rs"]
mod error_envelopes;
#[path = "all/event_topics.rs"]
mod event_topics;
#[path = "all/harness.rs"]
mod harness;
#[path = "all/ip_route_removal.rs"]
mod ip_route_removal;
#[path = "all/microsoft_ad.rs"]
mod microsoft_ad;
#[path = "all/shared_directories.rs"]
mod shared_directories;
#[path = "all/snapshots.rs"]
mod snapshots;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/trusts.rs"]
mod trusts;
