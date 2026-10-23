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
#[path = "all/bucket_acl.rs"]
mod bucket_acl;
#[path = "all/bucket_encryption.rs"]
mod bucket_encryption;
#[path = "all/bucket_lifecycle.rs"]
mod bucket_lifecycle;
#[path = "all/bucket_location.rs"]
mod bucket_location;
#[path = "all/bucket_notification_configuration.rs"]
mod bucket_notification_configuration;
#[path = "all/bucket_ownership_controls.rs"]
mod bucket_ownership_controls;
#[path = "all/bucket_policy_and_tagging.rs"]
mod bucket_policy_and_tagging;
#[path = "all/bucket_policy_put_status.rs"]
mod bucket_policy_put_status;
#[path = "all/bucket_public_access_block.rs"]
mod bucket_public_access_block;
#[path = "all/bucket_tagging.rs"]
mod bucket_tagging;
#[path = "all/bucket_versioning.rs"]
mod bucket_versioning;
#[path = "all/bucket_website.rs"]
mod bucket_website;
#[path = "all/error_message_envelope.rs"]
mod error_message_envelope;
// Deliberately empty: this fix's own table for the error-message rule was folded into
// `error_message_envelope`, which spans the same rule from the model's own catalog.
#[path = "all/error_messages.rs"]
mod error_messages;
#[path = "all/head_object_read_back.rs"]
mod head_object_read_back;
#[path = "all/list_buckets.rs"]
mod list_buckets;
#[path = "all/listing_encoding_type.rs"]
mod listing_encoding_type;
#[path = "all/object_and_policy.rs"]
mod object_and_policy;
#[path = "all/object_and_tagging_roundtrip.rs"]
mod object_and_tagging_roundtrip;
#[path = "all/object_encryption.rs"]
mod object_encryption;
#[path = "all/object_listing_and_deletion.rs"]
mod object_listing_and_deletion;
#[path = "all/object_storage.rs"]
mod object_storage;
#[path = "all/object_tagging.rs"]
mod object_tagging;
#[path = "all/object_tagging_on_put.rs"]
mod object_tagging_on_put;
#[path = "all/object_versioning.rs"]
mod object_versioning;
#[path = "all/object_write_and_read_back.rs"]
mod object_write_and_read_back;
#[path = "all/object_write_tagging_header.rs"]
mod object_write_tagging_header;
#[path = "all/objects.rs"]
mod objects;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
