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
#[path = "all/account_customizations.rs"]
mod account_customizations;
#[path = "all/body_boundary_refusal.rs"]
mod body_boundary_refusal;
#[path = "all/body_validation_refusals.rs"]
mod body_validation_refusals;
#[path = "all/catalog_published_spellings.rs"]
mod catalog_published_spellings;
#[path = "all/console_identifier_namespace.rs"]
mod console_identifier_namespace;
#[path = "all/list_setting_order.rs"]
mod list_setting_order;
#[path = "all/list_setting_reset.rs"]
mod list_setting_reset;
#[path = "all/member_constraint_field_list.rs"]
mod member_constraint_field_list;
#[path = "all/non_object_body_refusal.rs"]
mod non_object_body_refusal;
#[path = "all/one_constraint_refusal_path.rs"]
mod one_constraint_refusal_path;
#[path = "all/page_cursor_positions.rs"]
mod page_cursor_positions;
#[path = "all/pattern_refusal_rendering.rs"]
mod pattern_refusal_rendering;
#[path = "all/reported_account_color.rs"]
mod reported_account_color;
#[path = "all/service_catalog.rs"]
mod service_catalog;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/support.rs"]
mod support;
#[path = "all/unreadable_body_refusal.rs"]
mod unreadable_body_refusal;
#[path = "all/visible_service_catalog_membership.rs"]
mod visible_service_catalog_membership;
#[path = "all/visible_services_round_trip.rs"]
mod visible_services_round_trip;
#[path = "all/wire.rs"]
mod wire;
