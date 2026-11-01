// SPDX-License-Identifier: CC0-1.0

//! How this service words the refusal of a `cluster` an operation cannot act in.
//!
//! Amazon ECS answers ONE error code (`ClusterNotFoundException`, and on the agent plane
//! the `ClientException` those operations model instead) for two different cluster
//! states, so the sentence it carries is the only thing that tells them apart:
//!
//! * a cluster record this account still HOLDS, deleted and so left
//!   [`super::CLUSTER_INACTIVE`] — the `Cluster$status` doc: "The cluster has been
//!   deleted. Clusters with an INACTIVE status may remain discoverable in your account
//!   for a period of time" — is reported as the inactive cluster it is;
//! * a reference that resolves to NO cluster record is reported as not found.
//!
//! The selector is the cluster's own stored state, never the request: the same request
//! earns a different sentence only because the cluster behind it is in a different state.
//! Both sentences are fixed text — no name, id or limit from the request reaches either —
//! so a client can branch on which of the two states it is in (re-create or wait for a
//! deleted cluster; check the name or the Region for one this account never held).
//!
//! Two operations state the TOMBSTONE in their own terms instead, under their own error
//! code, because the condition they report is the cluster's STATE rather than its absence:
//! a write of the cluster's own settings ([`cluster_not_active_message`]) and a tagging
//! call on the deleted cluster's ARN ([`CLUSTER_INACTIVE_FOR_TAGGING_MESSAGE`], selected
//! by [`names_an_inactive_cluster`]).

use super::{ClusterRecord, CLUSTER_INACTIVE, CLUSTER_NOT_FOUND_MESSAGE};

/// The message a cluster-resolution refusal carries for a cluster record this store still
/// holds, in the status a delete leaves it in. Like its neighbours in [`super`] it names
/// no resource from the request.
pub const CLUSTER_INACTIVE_MESSAGE: &str = "The referenced cluster was inactive.";

/// `ClientException$message` for a write of a cluster's own SETTINGS against a cluster
/// record this store holds in a status that is not ACTIVE — a tombstone a delete left
/// [`CLUSTER_INACTIVE`].
///
/// The condition is the cluster's STATE, not its absence, so it is not the not-found
/// refusal: the settings write states which cluster is in the wrong state, taking the name
/// from the `cluster` the request named, and says which state it is not in. The fixed
/// words are everything but that name.
pub fn cluster_not_active_message(cluster_name: &str) -> String {
    format!("Cluster {cluster_name} is not in ACTIVE state")
}

/// `InvalidParameterException$message` for a TAGGING call whose `resourceArn` names a
/// cluster this store holds as a tombstone ([`CLUSTER_INACTIVE`]).
///
/// A deleted cluster is still discoverable, so its tag set is not read or written as if
/// the cluster were live: the call is refused as the bad parameter it carries. One fixed
/// sentence, naming no resource from the request.
pub const CLUSTER_INACTIVE_FOR_TAGGING_MESSAGE: &str =
    "The specified cluster is inactive. Specify an active cluster and try again.";

/// Whether `resource_arn` names one of the `clusters` this store holds as a TOMBSTONE — a
/// record a delete left [`CLUSTER_INACTIVE`].
///
/// `false` for an ARN that names no cluster at all: "this cluster is deleted" and "this
/// account never held this cluster" are different conditions, and the second one is the
/// not-found refusal every cluster resolution already answers.
pub fn names_an_inactive_cluster<'a>(
    clusters: impl IntoIterator<Item = &'a ClusterRecord>,
    resource_arn: &str,
) -> bool {
    clusters
        .into_iter()
        .any(|cluster| cluster.arn == resource_arn && cluster.status == CLUSTER_INACTIVE)
}

/// The sentence the refusal of a `cluster` that is not one
/// [`active_cluster`](super::ComAmazonawsEcsAmazonec2containerservicev20141113State::active_cluster)
/// names carries, chosen from the record this store holds under that name — which the
/// caller hands in, already looked up, so the choice reads off stored state and nothing
/// else: [`CLUSTER_INACTIVE_MESSAGE`] for a record whose status is [`CLUSTER_INACTIVE`],
/// and [`CLUSTER_NOT_FOUND_MESSAGE`] when there is no record at all.
///
/// One comparison over one borrowed record: every operation that resolves a `cluster`
/// words the same condition the same way, and the wording follows the stored record
/// rather than whatever the caller sent.
pub fn cluster_refusal_message(stored: Option<&ClusterRecord>) -> String {
    match stored.is_some_and(|cluster| cluster.status == CLUSTER_INACTIVE) {
        true => CLUSTER_INACTIVE_MESSAGE,
        false => CLUSTER_NOT_FOUND_MESSAGE,
    }
    .to_string()
}
