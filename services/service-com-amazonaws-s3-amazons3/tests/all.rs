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
#[path = "all/anonymous_object_reads.rs"]
mod anonymous_object_reads;
#[path = "all/aws_published_sample_data.rs"]
mod aws_published_sample_data;
#[path = "all/bucket_abac.rs"]
mod bucket_abac;
#[path = "all/bucket_accelerate_configuration.rs"]
mod bucket_accelerate_configuration;
#[path = "all/bucket_acl.rs"]
mod bucket_acl;
#[path = "all/bucket_analytics_configuration.rs"]
mod bucket_analytics_configuration;
#[path = "all/bucket_analytics_configuration_listing.rs"]
mod bucket_analytics_configuration_listing;
#[path = "all/bucket_cors.rs"]
mod bucket_cors;
#[path = "all/bucket_cors_delete.rs"]
mod bucket_cors_delete;
#[path = "all/bucket_deletion_emptiness.rs"]
mod bucket_deletion_emptiness;
#[path = "all/bucket_encryption.rs"]
mod bucket_encryption;
#[path = "all/bucket_error_envelope.rs"]
mod bucket_error_envelope;
#[path = "all/bucket_intelligent_tiering.rs"]
mod bucket_intelligent_tiering;
#[path = "all/bucket_inventory_configuration.rs"]
mod bucket_inventory_configuration;
#[path = "all/bucket_inventory_configuration_deletion.rs"]
mod bucket_inventory_configuration_deletion;
#[path = "all/bucket_inventory_configuration_listing.rs"]
mod bucket_inventory_configuration_listing;
#[path = "all/bucket_lifecycle.rs"]
mod bucket_lifecycle;
#[path = "all/bucket_lifecycle_configuration.rs"]
mod bucket_lifecycle_configuration;
#[path = "all/bucket_location.rs"]
mod bucket_location;
#[path = "all/bucket_logging.rs"]
mod bucket_logging;
#[path = "all/bucket_metadata_annotation_table_update.rs"]
mod bucket_metadata_annotation_table_update;
#[path = "all/bucket_metadata_configuration.rs"]
mod bucket_metadata_configuration;
#[path = "all/bucket_metadata_configuration_deletion.rs"]
mod bucket_metadata_configuration_deletion;
#[path = "all/bucket_metadata_inventory_table_namespace.rs"]
mod bucket_metadata_inventory_table_namespace;
#[path = "all/bucket_metadata_inventory_table_update.rs"]
mod bucket_metadata_inventory_table_update;
#[path = "all/bucket_metadata_journal_table_update.rs"]
mod bucket_metadata_journal_table_update;
#[path = "all/bucket_metadata_namespace_retention.rs"]
mod bucket_metadata_namespace_retention;
#[path = "all/bucket_metrics_configuration.rs"]
mod bucket_metrics_configuration;
#[path = "all/bucket_metrics_configuration_listing.rs"]
mod bucket_metrics_configuration_listing;
#[path = "all/bucket_name_namespace.rs"]
mod bucket_name_namespace;
#[path = "all/bucket_notification_configuration.rs"]
mod bucket_notification_configuration;
#[path = "all/bucket_object_lock_configuration.rs"]
mod bucket_object_lock_configuration;
#[path = "all/bucket_ownership_controls.rs"]
mod bucket_ownership_controls;
#[path = "all/bucket_policy_and_tagging.rs"]
mod bucket_policy_and_tagging;
#[path = "all/bucket_policy_delete.rs"]
mod bucket_policy_delete;
#[path = "all/bucket_policy_put_status.rs"]
mod bucket_policy_put_status;
#[path = "all/bucket_policy_status.rs"]
mod bucket_policy_status;
#[path = "all/bucket_public_access_block.rs"]
mod bucket_public_access_block;
#[path = "all/bucket_replication.rs"]
mod bucket_replication;
#[path = "all/bucket_replication_delete.rs"]
mod bucket_replication_delete;
#[path = "all/bucket_request_payment.rs"]
mod bucket_request_payment;
#[path = "all/bucket_subresource_absent_messages.rs"]
mod bucket_subresource_absent_messages;
#[path = "all/bucket_tagging.rs"]
mod bucket_tagging;
#[path = "all/bucket_tagging_put_status.rs"]
mod bucket_tagging_put_status;
#[path = "all/bucket_versioning.rs"]
mod bucket_versioning;
#[path = "all/bucket_website.rs"]
mod bucket_website;
#[path = "all/bucket_website_delete.rs"]
mod bucket_website_delete;
#[path = "all/checksum_mode_header_case.rs"]
mod checksum_mode_header_case;
#[path = "all/cli_download_legs.rs"]
mod cli_download_legs;
#[path = "all/directory_bucket_conditional_deletes.rs"]
mod directory_bucket_conditional_deletes;
#[path = "all/directory_bucket_sessions_and_appends.rs"]
mod directory_bucket_sessions_and_appends;
#[path = "all/error_message_envelope.rs"]
mod error_message_envelope;
// Deliberately empty: this fix's own table for the error-message rule was folded into
// `error_message_envelope`, which spans the same rule from the model's own catalog.
#[path = "all/error_messages.rs"]
mod error_messages;
#[path = "all/general_purpose_bucket_names.rs"]
mod general_purpose_bucket_names;
#[path = "all/head_object_read_back.rs"]
mod head_object_read_back;
#[path = "all/json_payload_media_type.rs"]
mod json_payload_media_type;
#[path = "all/list_buckets.rs"]
mod list_buckets;
#[path = "all/listing_encoding_type.rs"]
mod listing_encoding_type;
#[path = "all/listing_optional_members.rs"]
mod listing_optional_members;
#[path = "all/multipart_checksum_type.rs"]
mod multipart_checksum_type;
#[path = "all/multipart_part_checksums.rs"]
mod multipart_part_checksums;
#[path = "all/multipart_part_copy.rs"]
mod multipart_part_copy;
#[path = "all/multipart_part_upload.rs"]
mod multipart_part_upload;
#[path = "all/multipart_upload_abort.rs"]
mod multipart_upload_abort;
#[path = "all/multipart_upload_completion.rs"]
mod multipart_upload_completion;
#[path = "all/multipart_upload_event_notifications.rs"]
mod multipart_upload_event_notifications;
#[path = "all/multipart_upload_initiation.rs"]
mod multipart_upload_initiation;
#[path = "all/multipart_upload_listing.rs"]
mod multipart_upload_listing;
#[path = "all/notification_destination_validation.rs"]
mod notification_destination_validation;
#[path = "all/object_acl.rs"]
mod object_acl;
#[path = "all/object_and_policy.rs"]
mod object_and_policy;
#[path = "all/object_and_tagging_roundtrip.rs"]
mod object_and_tagging_roundtrip;
#[path = "all/object_annotation_listing.rs"]
mod object_annotation_listing;
#[path = "all/object_annotation_removal.rs"]
mod object_annotation_removal;
#[path = "all/object_annotations.rs"]
mod object_annotations;
#[path = "all/object_attributes.rs"]
mod object_attributes;
#[path = "all/object_batch_deletion.rs"]
mod object_batch_deletion;
#[path = "all/object_copy.rs"]
mod object_copy;
#[path = "all/object_content_type.rs"]
mod object_content_type;
#[path = "all/object_conditional_deletes.rs"]
mod object_conditional_deletes;
#[path = "all/object_conditional_writes.rs"]
mod object_conditional_writes;
#[path = "all/object_default_encryption.rs"]
mod object_default_encryption;
#[path = "all/object_encryption.rs"]
mod object_encryption;
#[path = "all/object_encryption_update.rs"]
mod object_encryption_update;
#[path = "all/object_event_notifications.rs"]
mod object_event_notifications;
#[path = "all/object_event_sequencer_order.rs"]
mod object_event_sequencer_order;
#[path = "all/object_legal_hold.rs"]
mod object_legal_hold;
#[path = "all/object_listing_and_deletion.rs"]
mod object_listing_and_deletion;
#[path = "all/object_listing_encoding.rs"]
mod object_listing_encoding;
#[path = "all/object_listing_v1.rs"]
mod object_listing_v1;
#[path = "all/object_lock_enforcement.rs"]
mod object_lock_enforcement;
#[path = "all/object_lock_on_write.rs"]
mod object_lock_on_write;
#[path = "all/object_partial_reads.rs"]
mod object_partial_reads;
#[path = "all/object_plane_byte_budget.rs"]
mod object_plane_byte_budget;
#[path = "all/object_read_authorization.rs"]
mod object_read_authorization;
#[path = "all/object_replication.rs"]
mod object_replication;
#[path = "all/object_retention.rs"]
mod object_retention;
#[path = "all/object_restore.rs"]
mod object_restore;
#[path = "all/object_select_content.rs"]
mod object_select_content;
#[path = "all/object_storage.rs"]
mod object_storage;
#[path = "all/object_subresource_version_id.rs"]
mod object_subresource_version_id;
#[path = "all/object_tag_set_removal.rs"]
mod object_tag_set_removal;
#[path = "all/object_tagging.rs"]
mod object_tagging;
#[path = "all/object_tagging_event_sub_types.rs"]
mod object_tagging_event_sub_types;
#[path = "all/object_tagging_on_put.rs"]
mod object_tagging_on_put;
#[path = "all/object_torrent.rs"]
mod object_torrent;
#[path = "all/object_versioning.rs"]
mod object_versioning;
#[path = "all/object_write_and_read_back.rs"]
mod object_write_and_read_back;
#[path = "all/object_write_tagging_header.rs"]
mod object_write_tagging_header;
#[path = "all/objects.rs"]
mod objects;
#[path = "all/partial_read_checksums.rs"]
mod partial_read_checksums;
#[path = "all/presigned_object_reads.rs"]
mod presigned_object_reads;
#[path = "all/presigned_query_authentication.rs"]
mod presigned_query_authentication;
#[path = "all/published_refusal_wording.rs"]
mod published_refusal_wording;
#[path = "all/public_access_block_default.rs"]
mod public_access_block_default;
#[path = "all/public_access_guardrails.rs"]
mod public_access_guardrails;
#[path = "all/refusal_class_fidelity.rs"]
mod refusal_class_fidelity;
#[path = "all/refusal_codes.rs"]
mod refusal_codes;
#[path = "all/request_integrity_headers.rs"]
mod request_integrity_headers;
#[path = "all/refusal_messages_published.rs"]
mod refusal_messages_published;
#[path = "all/refusal_resource_naming.rs"]
mod refusal_resource_naming;
#[path = "all/refused_argument_envelope.rs"]
mod refused_argument_envelope;
#[path = "all/reserved_tag_key_prefix.rs"]
mod reserved_tag_key_prefix;
#[path = "all/signature_refusal_wording.rs"]
mod signature_refusal_wording;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/version_id_classification.rs"]
mod version_id_classification;
#[path = "all/write_checksum_header_count.rs"]
mod write_checksum_header_count;
#[path = "all/write_response_identity.rs"]
mod write_response_identity;
#[path = "all/zz_probe.rs"]
mod zz_probe;
