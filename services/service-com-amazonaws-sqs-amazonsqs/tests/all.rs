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
#[path = "all/cancel_message_move_task.rs"]
mod cancel_message_move_task;
#[path = "all/delete_message_batch.rs"]
mod delete_message_batch;
#[path = "all/fifo_queue.rs"]
mod fifo_queue;
#[path = "all/fifo_receive_idempotency.rs"]
mod fifo_receive_idempotency;
#[path = "all/list_dead_letter_source_queues.rs"]
mod list_dead_letter_source_queues;
#[path = "all/list_message_move_tasks.rs"]
mod list_message_move_tasks;
#[path = "all/message_attribute_rules.rs"]
mod message_attribute_rules;
#[path = "all/message_lifecycle.rs"]
mod message_lifecycle;
#[path = "all/message_move_task.rs"]
mod message_move_task;
#[path = "all/message_plane.rs"]
mod message_plane;
#[path = "all/message_visibility_changes.rs"]
mod message_visibility_changes;
#[path = "all/purge_queue.rs"]
mod purge_queue;
#[path = "all/queue_configuration.rs"]
mod queue_configuration;
#[path = "all/queue_does_not_exist.rs"]
mod queue_does_not_exist;
#[path = "all/queue_encryption.rs"]
mod queue_encryption;
#[path = "all/queue_lifecycle.rs"]
mod queue_lifecycle;
#[path = "all/queue_permissions.rs"]
mod queue_permissions;
#[path = "all/queue_policy_authorization.rs"]
mod queue_policy_authorization;
#[path = "all/queue_tags.rs"]
mod queue_tags;
#[path = "all/receive_message_attribute_names.rs"]
mod receive_message_attribute_names;
#[path = "all/refusal_taxonomy.rs"]
mod refusal_taxonomy;
#[path = "all/send_message_batch.rs"]
mod send_message_batch;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/support.rs"]
mod support;
#[path = "all/time_driven_queue_semantics.rs"]
mod time_driven_queue_semantics;
