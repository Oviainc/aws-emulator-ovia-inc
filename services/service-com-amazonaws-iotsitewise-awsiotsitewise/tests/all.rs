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
//
// `property_aggregates` is declared here, ahead of the sorted list, rather than in
// its alphabetical place: every other suite this crate has gained was inserted
// into that list or appended after it, so those are the two spots a concurrent
// change also edits. Fold it into the list once the churn settles.
#[path = "all/property_aggregates.rs"]
mod property_aggregates;

// Same reasoning as `property_aggregates` above, and declared beside it rather than
// inside the list or after it, which are the two spots the comment above names as
// the ones a concurrent change also edits. Fold it into the list once the churn
// settles.
#[path = "all/interpolated_property_values.rs"]
mod interpolated_property_values;

// Same reasoning again, and the reason this one is NOT in the sorted list: its
// alphabetical place is between `asset_property_update` and `associated_assets`,
// the stretch of the list every concurrent asset-side suite lands in, and a
// declaration there conflicted with each mainline tip this fix was rebased onto.
// Fold it into the list once the churn settles.
#[path = "all/asset_relationships.rs"]
mod asset_relationships;

// Same reasoning once more: this suite's alphabetical place is inside the sorted
// list, which is where every concurrent iotsitewise change lands a declaration of
// its own, so it is declared up here with the other suites kept out of that
// stretch. Fold it into the list once the churn settles.
#[path = "all/simultaneous_writes.rs"]
mod simultaneous_writes;

#[path = "all/asset_hierarchies.rs"]
mod asset_hierarchies;
#[path = "all/asset_lifecycle.rs"]
mod asset_lifecycle;
#[path = "all/asset_model_composite_model.rs"]
mod asset_model_composite_model;
#[path = "all/asset_model_composite_model_delete.rs"]
mod asset_model_composite_model_delete;
#[path = "all/asset_model_properties.rs"]
mod asset_model_properties;
// Declared only because the build requires every file under `tests/all/` to be:
// the suite was folded into `update_asset_model.rs` and this module now carries
// no test. See the file's own note.
#[path = "all/asset_model_update.rs"]
mod asset_model_update;
#[path = "all/asset_property_update.rs"]
mod asset_property_update;
#[path = "all/associated_assets.rs"]
mod associated_assets;
#[path = "all/batch_get_latest_values.rs"]
mod batch_get_latest_values;
#[path = "all/batch_put_timestamp_and_quality.rs"]
mod batch_put_timestamp_and_quality;
#[path = "all/describe_asset_property.rs"]
mod describe_asset_property;
#[path = "all/execute_query.rs"]
mod execute_query;
#[path = "all/gateway_lifecycle.rs"]
mod gateway_lifecycle;
#[path = "all/hierarchies_etags_and_refusals.rs"]
mod hierarchies_etags_and_refusals;
#[path = "all/property_value_history.rs"]
mod property_value_history;
#[path = "all/resource_tags.rs"]
mod resource_tags;
#[path = "all/single_property_aggregates.rs"]
mod single_property_aggregates;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/support.rs"]
mod support;
#[path = "all/time_series.rs"]
mod time_series;
#[path = "all/time_series_disassociation.rs"]
mod time_series_disassociation;
#[path = "all/time_series_ingestion.rs"]
mod time_series_ingestion;
#[path = "all/update_asset.rs"]
mod update_asset;
#[path = "all/update_asset_model.rs"]
mod update_asset_model;

