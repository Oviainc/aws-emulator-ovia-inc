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
#[path = "all/channel_deletion.rs"]
mod channel_deletion;
#[path = "all/channel_error_envelope.rs"]
mod channel_error_envelope;
#[path = "all/channel_lifecycle.rs"]
mod channel_lifecycle;
#[path = "all/channel_listing.rs"]
mod channel_listing;
#[path = "all/channel_updates.rs"]
mod channel_updates;
#[path = "all/dashboard_deletion.rs"]
mod dashboard_deletion;
#[path = "all/dashboard_enumeration.rs"]
mod dashboard_enumeration;
#[path = "all/dashboard_lifecycle.rs"]
mod dashboard_lifecycle;
#[path = "all/dashboard_refresh.rs"]
mod dashboard_refresh;
#[path = "all/dashboard_updates.rs"]
mod dashboard_updates;
#[path = "all/disable_federation.rs"]
mod disable_federation;
#[path = "all/emulator.rs"]
mod emulator;
#[path = "all/enum_refusal_wording.rs"]
mod enum_refusal_wording;
#[path = "all/event_configuration.rs"]
mod event_configuration;
#[path = "all/event_data_store_ingestion.rs"]
mod event_data_store_ingestion;
#[path = "all/event_data_store_ingestion_start.rs"]
mod event_data_store_ingestion_start;
#[path = "all/event_data_store_lifecycle.rs"]
mod event_data_store_lifecycle;
#[path = "all/event_data_store_reads.rs"]
mod event_data_store_reads;
#[path = "all/event_data_store_update.rs"]
mod event_data_store_update;
#[path = "all/event_selectors.rs"]
mod event_selectors;
#[path = "all/federation_lifecycle.rs"]
mod federation_lifecycle;
#[path = "all/generate_query.rs"]
mod generate_query;
#[path = "all/import_list.rs"]
mod import_list;
#[path = "all/import_reads.rs"]
mod import_reads;
#[path = "all/import_start.rs"]
mod import_start;
#[path = "all/import_stop.rs"]
mod import_stop;
#[path = "all/insight_selectors.rs"]
mod insight_selectors;
#[path = "all/insights_data.rs"]
mod insights_data;
#[path = "all/insights_metric_data.rs"]
mod insights_metric_data;
#[path = "all/lake_queries.rs"]
mod lake_queries;
#[path = "all/list_tags_reader.rs"]
mod list_tags_reader;
#[path = "all/query_cancellation.rs"]
mod query_cancellation;
#[path = "all/range_refusal_wording.rs"]
mod range_refusal_wording;
#[path = "all/refusal_fidelity.rs"]
mod refusal_fidelity;
#[path = "all/resource_policies.rs"]
mod resource_policies;
#[path = "all/resource_tagging.rs"]
mod resource_tagging;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
// Declared HERE rather than in the `import_*` span of the sorted list above: that span
// is where every import suite landing in the same window inserts a line, so it is the
// contended region, while these neighbours are stable. `harness` holds this suite's
// import-registry fixtures.
#[path = "all/harness.rs"]
mod harness;
#[path = "all/import_failures.rs"]
mod import_failures;
// Declared beside `import_failures` for the same reason: the sorted span above is the
// contended region. `pattern_refusal_wording` pins how this service words a boundary
// refusal for a `@pattern` violation, over every operation that has one.
#[path = "all/pattern_refusal_wording.rs"]
mod pattern_refusal_wording;
#[path = "all/tag_refusal_envelope.rs"]
mod tag_refusal_envelope;
#[path = "all/tagging_arn_spelling.rs"]
mod tagging_arn_spelling;
#[path = "all/target_prefix_dispatch.rs"]
mod target_prefix_dispatch;
#[path = "all/target_prefix_routing.rs"]
mod target_prefix_routing;
#[path = "all/target_routing.rs"]
mod target_routing;
#[path = "all/trail_lifecycle.rs"]
mod trail_lifecycle;
#[path = "all/trail_refusals.rs"]
mod trail_refusals;
#[path = "all/trail_update.rs"]
mod trail_update;
// Declared only because the build's layout check requires every file under `tests/all/`
// to be declared: these two are empty leftovers (see their own doc comments), kept
// because the fixers' tool cannot delete a file.
#[path = "all/wire.rs"]
mod wire;
#[path = "all/zz_diagnostic.rs"]
mod zz_diagnostic;
// Declared after the alphabetical block above, not inside it: this file is edited by
// every fix that adds a suite to this crate, and a declaration appended here cannot
// collide with one inserted alphabetically beside a neighbouring suite.
#[path = "all/resource_policy.rs"]
mod resource_policy;
#[path = "all/resource_policy_isolation.rs"]
mod resource_policy_isolation;
