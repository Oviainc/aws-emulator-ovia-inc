// SPDX-License-Identifier: CC0-1.0

//! The second template this service answers a `ReplicationGroupNotFoundFault` with, and
//! which operations take it.
//!
//! ElastiCache words that fault two ways, and the two are observable apart on the live
//! service, so one service-wide template cannot serve them all:
//!
//! ```text
//! ReplicationGroup <id> not found.         <- the shape's spelling, a trailing stop
//! Replication group '<id>' not found       <- prose, the id quoted, no trailing stop
//! ```
//!
//! The first is [`crate::state::replication_group_not_found_message`], which the describe,
//! the failover test and the migration answer. [`quoted_message`] is the second, which the
//! settings change (`ModifyReplicationGroup`) and the Global datastore create
//! (`CreateGlobalReplicationGroup`) answer for the group each resolves.
//!
//! Stated here once so the two handlers answering it cannot drift apart about its text: the
//! message is the only place a client reads WHICH group could not be resolved.

/// The template `Replication group '<id>' not found` — the id single-quoted, and no
/// trailing stop.
///
/// The id is the one variable part and comes from the request, as the request spelled it,
/// because the message is built from the request and from nothing else.
pub fn quoted_message(replication_group_id: &str) -> String {
    format!("Replication group '{replication_group_id}' not found")
}
