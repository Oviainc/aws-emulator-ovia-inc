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
#[path = "all/byte_match_set_lifecycle.rs"]
mod byte_match_set_lifecycle;
// Not one resource kind's lifecycle but the refusal messages of EVERY kind.
#[path = "all/exception_messages.rs"]
mod exception_messages;
#[path = "all/geo_match_set_lifecycle.rs"]
mod geo_match_set_lifecycle;
// The WebACL fixtures the web-ACL suites share, on top of the wire harness.
#[path = "all/harness.rs"]
mod harness;
#[path = "all/ip_set_lifecycle.rs"]
mod ip_set_lifecycle;
#[path = "all/permission_policy.rs"]
mod permission_policy;
#[path = "all/rate_based_rule_lifecycle.rs"]
mod rate_based_rule_lifecycle;
#[path = "all/rate_based_rule_updates.rs"]
mod rate_based_rule_updates;
#[path = "all/regex_match_set.rs"]
mod regex_match_set;
#[path = "all/regex_pattern_set.rs"]
mod regex_pattern_set;
#[path = "all/regex_pattern_set_updates.rs"]
mod regex_pattern_set_updates;
#[path = "all/resource_tags.rs"]
mod resource_tags;
#[path = "all/rule_deletion.rs"]
mod rule_deletion;
#[path = "all/rule_group_lifecycle.rs"]
mod rule_group_lifecycle;
#[path = "all/rule_lifecycle.rs"]
mod rule_lifecycle;
#[path = "all/rule_predicates.rs"]
mod rule_predicates;
#[path = "all/size_constraint_set.rs"]
mod size_constraint_set;
#[path = "all/size_constraint_set_updates.rs"]
mod size_constraint_set_updates;
#[path = "all/sql_injection_match_set.rs"]
mod sql_injection_match_set;
#[path = "all/sql_injection_match_set_updates.rs"]
mod sql_injection_match_set_updates;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/web_acl_default_action.rs"]
mod web_acl_default_action;
#[path = "all/web_acl_lifecycle.rs"]
mod web_acl_lifecycle;
#[path = "all/wire.rs"]
mod wire;
#[path = "all/xss_match_set_lifecycle.rs"]
mod xss_match_set_lifecycle;
