// SPDX-License-Identifier: CC0-1.0

//! The resize a cluster's last shape change asked for, and how a read reports its
//! progress.
//!
//! `DescribeResize` "returns information about the LAST resize operation for the
//! specified cluster. If no resize operation has ever been initiated for the
//! specified cluster, a HTTP 404 error is returned. If a resize operation was
//! initiated and completed, the status of the resize remains as `SUCCEEDED` until
//! the next resize" (`smithy_query(query="doc", operation="DescribeResize")`). So
//! a cluster holds at most ONE such record — the last one — it is created by the
//! call that changes the cluster's shape, and it never leaves the cluster again:
//! what moves is only its status, which the clock owes.
//!
//! Which call creates one is the shape change itself: `ModifyCluster`'s own
//! documentation says that when "you decide to change your cluster's node type or
//! number of nodes … you can use `DescribeResize` to track the progress of the
//! resize request" (`smithy_query(query="doc", operation="ModifyCluster")`), so a
//! modify that changes the node type, the cluster type or the node count IS a
//! resize of the cluster and a modify that changes anything else is not.
//!
//! Every number a read reports is computed from this record and the clock —
//! nothing is drawn per run and nothing is invented about data that was moved,
//! because this control plane moves none: a resize reports WHICH shape the
//! cluster is moving to, how long the move has been running and how long is left,
//! and says nothing about table imports or megabytes.

use super::*;

/// `ResizeProgressMessage$Status` while the resize is still running.
///
/// Source: `smithy_query(query="doc", shape="ResizeProgressMessage",
/// member="Status")` — "Valid Values: `NONE` | `IN_PROGRESS` | `FAILED` |
/// `SUCCEEDED` | `CANCELLING`".
pub const RESIZE_STATUS_IN_PROGRESS: &str = "IN_PROGRESS";
/// `ResizeProgressMessage$Status` once the resize has finished, which it stays at
/// "until the next resize". See [`RESIZE_STATUS_IN_PROGRESS`].
pub const RESIZE_STATUS_SUCCEEDED: &str = "SUCCEEDED";

/// `ResizeProgressMessage$ResizeType` of a resize that moves the cluster onto new
/// nodes.
///
/// Source: `smithy_query(query="doc", shape="ResizeProgressMessage",
/// member="ResizeType")` — "An enum with possible values of `ClassicResize` and
/// `ElasticResize`" — and `ResizeInfo$ResizeType`, whose own doc is "Returns the
/// value `ClassicResize`". A `ModifyCluster` that changes the node type or the
/// cluster type rebuilds the cluster on new nodes, which is the classic resize;
/// the elastic one is what `ResizeCluster` performs by default.
pub const RESIZE_TYPE_CLASSIC: &str = "ClassicResize";

/// `ResizeProgressMessage$TargetEncryptionType` of a resize whose target cluster
/// is encrypted, and of one whose target cluster is not.
///
/// Source: `smithy_query(query="doc", shape="ResizeProgressMessage",
/// member="TargetEncryptionType")` — "The type of encryption for the cluster after
/// the resize is complete. Possible values are `KMS` and `None`."
pub const TARGET_ENCRYPTION_KMS: &str = "KMS";
/// See [`TARGET_ENCRYPTION_KMS`].
pub const TARGET_ENCRYPTION_NONE: &str = "None";

/// The resize a cluster's last shape change started, as this service stores it.
#[derive(Debug, Clone)]
pub struct StoredResize {
    /// `ResizeProgressMessage$TargetNodeType` — "the node type that the cluster
    /// will have after the resize operation is complete".
    pub target_node_type: Option<String>,
    /// `ResizeProgressMessage$TargetNumberOfNodes` — "the number of nodes that
    /// the cluster will have after the resize operation is complete".
    pub target_number_of_nodes: Option<i32>,
    /// `ResizeProgressMessage$TargetClusterType` — "the cluster type after the
    /// resize operation is complete".
    pub target_cluster_type: Option<String>,
    /// `ResizeProgressMessage$TargetEncryptionType`, one of
    /// [`TARGET_ENCRYPTION_KMS`] / [`TARGET_ENCRYPTION_NONE`].
    pub target_encryption_type: String,
    /// `ResizeProgressMessage$ResizeType`.
    pub resize_type: String,
    /// The instant the call that asked for the resize was served at, which every
    /// elapsed time is measured from.
    pub started_at: Instant,
    /// When the clock-owed [`RESIZE_STATUS_IN_PROGRESS`] ->
    /// [`RESIZE_STATUS_SUCCEEDED`] transition falls due. See
    /// [`PROVISIONING_SPAN`]: moving a cluster onto new nodes is the same slow
    /// work provisioning one is.
    pub completed_at: Instant,
}

impl StoredResize {
    /// The resize a shape change to `target_cluster_type` / `target_number_of_nodes`
    /// / `target_node_type` starts at `started`, on a cluster whose data is
    /// `encrypted`.
    pub fn started(
        started: Instant,
        target_node_type: Option<String>,
        target_cluster_type: Option<String>,
        target_number_of_nodes: Option<i32>,
        encrypted: bool,
    ) -> Self {
        Self {
            target_node_type,
            target_number_of_nodes,
            target_cluster_type,
            target_encryption_type: if encrypted {
                TARGET_ENCRYPTION_KMS
            } else {
                TARGET_ENCRYPTION_NONE
            }
            .to_string(),
            resize_type: RESIZE_TYPE_CLASSIC.to_string(),
            started_at: started,
            completed_at: started + PROVISIONING_SPAN,
        }
    }

    /// The status this resize is in at `now`: running until its completion falls
    /// due, and `SUCCEEDED` from then on — which it stays at until the next
    /// resize REPLACES this record.
    pub fn status_at(&self, now: Instant) -> &'static str {
        if now >= self.completed_at {
            RESIZE_STATUS_SUCCEEDED
        } else {
            RESIZE_STATUS_IN_PROGRESS
        }
    }

    /// `ResizeProgressMessage$ElapsedTimeInSeconds` at `now` — "the amount of
    /// seconds that have elapsed since the resize operation began. After the
    /// resize operation completes, this value shows the total actual time, in
    /// seconds, for the resize operation", so it stops growing when the resize
    /// does.
    ///
    /// The member counts WHOLE seconds, and the span this emulator compresses a
    /// resize into is shorter than one ([`PROVISIONING_SPAN`]) — so a resize that
    /// has finished reports no seconds elapsed, which is what that span honestly
    /// comes to rather than a duration nothing took.
    pub fn elapsed_seconds(&self, now: Instant) -> i64 {
        let until = now.min(self.completed_at);
        seconds_between(self.started_at, until)
    }

    /// `ResizeProgressMessage$EstimatedTimeToCompletionInSeconds` at `now` — "the
    /// estimated time remaining, in seconds, until the resize operation is
    /// complete … Once the resize operation is complete, this value will be 0".
    pub fn remaining_seconds(&self, now: Instant) -> i64 {
        seconds_between(now, self.completed_at)
    }
}

/// Whole seconds from `from` to `to`, and zero where `to` is not after `from`.
///
/// Saturating on both ends: an instant pair this cannot subtract reports no time
/// rather than panicking, and the count cannot overflow the modelled `long`.
fn seconds_between(from: Instant, to: Instant) -> i64 {
    i64::try_from(to.saturating_duration_since(from).as_secs()).unwrap_or(i64::MAX)
}

/// Why a cluster that has never been resized reports no resize.
///
/// Recorded Amazon Redshift wording for `ResizeNotFoundFault` (whose own doc is
/// "A resize operation for the specified cluster is not found"): the fixed text is
/// `No Resize found for Cluster ` followed by the cluster identifier the request
/// named.
pub fn resize_not_found_message(cluster_identifier: &str) -> String {
    format!("No Resize found for Cluster {cluster_identifier}")
}
