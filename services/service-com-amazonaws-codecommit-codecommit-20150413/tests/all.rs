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
#[path = "all/approval_rule_template_content_canonicalization.rs"]
mod approval_rule_template_content_canonicalization;
#[path = "all/approval_rule_template_content_update.rs"]
mod approval_rule_template_content_update;
#[path = "all/approval_rule_template_description.rs"]
mod approval_rule_template_description;
#[path = "all/approval_rule_templates.rs"]
mod approval_rule_templates;
#[path = "all/approval_rules_and_commits.rs"]
mod approval_rules_and_commits;
#[path = "all/aws_error_message_text.rs"]
mod aws_error_message_text;
#[path = "all/batch_associate_approval_rule_template.rs"]
mod batch_associate_approval_rule_template;
#[path = "all/batch_get_repositories.rs"]
mod batch_get_repositories;
#[path = "all/blob_content.rs"]
mod blob_content;
#[path = "all/branch_create.rs"]
mod branch_create;
#[path = "all/branch_creation.rs"]
mod branch_creation;
#[path = "all/branch_delete.rs"]
mod branch_delete;
#[path = "all/branch_listing.rs"]
mod branch_listing;
#[path = "all/branch_listing_writes.rs"]
mod branch_listing_writes;
#[path = "all/client_error_messages.rs"]
mod client_error_messages;
#[path = "all/comment_content_deletion.rs"]
mod comment_content_deletion;
#[path = "all/comment_deleted_refusals.rs"]
mod comment_deleted_refusals;
#[path = "all/comment_lifecycle.rs"]
mod comment_lifecycle;
#[path = "all/comment_thread_traversal.rs"]
mod comment_thread_traversal;
#[path = "all/commit_reads.rs"]
mod commit_reads;
#[path = "all/compared_commit_comments.rs"]
mod compared_commit_comments;
#[path = "all/default_branch.rs"]
mod default_branch;
#[path = "all/differences.rs"]
mod differences;
#[path = "all/file_commit_history.rs"]
mod file_commit_history;
#[path = "all/folder_listing.rs"]
mod folder_listing;
#[path = "all/git_object_ids.rs"]
mod git_object_ids;
#[path = "all/git_transport.rs"]
mod git_transport;
#[path = "all/harness.rs"]
mod harness;
#[path = "all/object_store_bounds.rs"]
mod object_store_bounds;
#[path = "all/path_depth_bound.rs"]
mod path_depth_bound;
#[path = "all/repositories_for_approval_rule_template.rs"]
mod repositories_for_approval_rule_template;
#[path = "all/repository_description.rs"]
mod repository_description;
#[path = "all/repository_encryption_key.rs"]
mod repository_encryption_key;
#[path = "all/repository_lifecycle.rs"]
mod repository_lifecycle;
#[path = "all/repository_rename.rs"]
mod repository_rename;
#[path = "all/resource_tagging.rs"]
mod resource_tagging;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/wire.rs"]
mod wire;
