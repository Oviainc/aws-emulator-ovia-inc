// SPDX-License-Identifier: CC0-1.0

//! Single integration-test target for this crate.
//!
//! Put a new suite at `tests/all/<name>.rs` and declare it below, keeping the list
//! sorted.
//!
//! A suite that is not declared here compiles nowhere and its tests never run. A
//! suite left at the top level of `tests/` fails the build.

// `tests/all.rs` is a test-target CRATE ROOT, so a bare `mod` resolves to a SIBLING
// (`tests/describe_vpcs.rs`) — which the layout check refuses. The `#[path]` is what
// points it into `tests/all/`.
#[path = "all/availability_zones.rs"]
mod availability_zones;
#[path = "all/describe_subnets.rs"]
mod describe_subnets;
#[path = "all/describe_vpcs.rs"]
mod describe_vpcs;
// The shared ec2Query plumbing every describe drives the service through.
#[path = "all/ec2_query.rs"]
mod ec2_query;
#[path = "all/instance_lifecycle.rs"]
mod instance_lifecycle;
#[path = "all/instance_type_offerings.rs"]
mod instance_type_offerings;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
