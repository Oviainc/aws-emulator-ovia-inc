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
#[path = "all/firewall_configs.rs"]
mod firewall_configs;
#[path = "all/firewall_domain_list_deletion.rs"]
mod firewall_domain_list_deletion;
#[path = "all/firewall_domain_list_listing.rs"]
mod firewall_domain_list_listing;
#[path = "all/firewall_domain_lists.rs"]
mod firewall_domain_lists;
#[path = "all/firewall_domains.rs"]
mod firewall_domains;
#[path = "all/harness.rs"]
mod harness;
#[path = "all/outpost_resolvers.rs"]
mod outpost_resolvers;
#[path = "all/resolver_dnssec_configs.rs"]
mod resolver_dnssec_configs;
#[path = "all/resolver_endpoints.rs"]
mod resolver_endpoints;
#[path = "all/resolver_rules.rs"]
mod resolver_rules;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
