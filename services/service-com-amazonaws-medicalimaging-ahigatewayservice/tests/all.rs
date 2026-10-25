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
#[path = "all/datastore_deleted_window.rs"]
mod datastore_deleted_window;
#[path = "all/datastore_lifecycle.rs"]
mod datastore_lifecycle;
#[path = "all/datastore_status_guard.rs"]
mod datastore_status_guard;
#[path = "all/error_message_texts.rs"]
mod error_message_texts;
#[path = "all/image_frames.rs"]
mod image_frames;
#[path = "all/image_set_access_time.rs"]
mod image_set_access_time;
#[path = "all/image_set_copy.rs"]
mod image_set_copy;
#[path = "all/image_set_deletion.rs"]
mod image_set_deletion;
#[path = "all/image_set_metadata.rs"]
mod image_set_metadata;
#[path = "all/image_set_promotion.rs"]
mod image_set_promotion;
#[path = "all/image_set_properties.rs"]
mod image_set_properties;
#[path = "all/image_set_search.rs"]
mod image_set_search;
#[path = "all/image_set_version_listing.rs"]
mod image_set_version_listing;
#[path = "all/import_job_listing.rs"]
mod import_job_listing;
#[path = "all/regional_isolation.rs"]
mod regional_isolation;
#[path = "all/resource_tagging.rs"]
mod resource_tagging;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/tag_map_validation.rs"]
mod tag_map_validation;
#[path = "all/tag_resource_key_pattern.rs"]
mod tag_resource_key_pattern;
#[path = "all/tagging_not_found_text.rs"]
mod tagging_not_found_text;
#[path = "all/untag_key_validation.rs"]
mod untag_key_validation;
