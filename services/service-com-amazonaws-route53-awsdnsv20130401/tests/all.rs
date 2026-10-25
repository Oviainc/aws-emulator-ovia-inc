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
#[path = "all/account_limits.rs"]
mod account_limits;
#[path = "all/checker_ip_ranges.rs"]
mod checker_ip_ranges;
#[path = "all/error_classification.rs"]
mod error_classification;
#[path = "all/error_envelope.rs"]
mod error_envelope;
#[path = "all/hosted_zone_config.rs"]
mod hosted_zone_config;
#[path = "all/hosted_zone_count.rs"]
mod hosted_zone_count;
#[path = "all/hosted_zone_records.rs"]
mod hosted_zone_records;
#[path = "all/hosted_zones_by_vpc.rs"]
mod hosted_zones_by_vpc;
#[path = "all/record_name_canonical_form.rs"]
mod record_name_canonical_form;
#[path = "all/private_zone_creation.rs"]
mod private_zone_creation;
#[path = "all/resource_tags.rs"]
mod resource_tags;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/traffic_policies.rs"]
mod traffic_policies;
#[path = "all/vpc_association_authorizations.rs"]
mod vpc_association_authorizations;
#[path = "all/vpc_associations.rs"]
mod vpc_associations;
#[path = "all/vpc_authorization_deletion.rs"]
mod vpc_authorization_deletion;
