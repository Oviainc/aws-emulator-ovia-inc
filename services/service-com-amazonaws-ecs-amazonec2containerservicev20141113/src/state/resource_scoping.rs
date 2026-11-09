// SPDX-License-Identifier: CC0-1.0

//! Where a request may act: the SCOPE every reference it carries has to be inside.
//!
//! Amazon ECS scopes almost everything by CLUSTER, and the scopes decided here are decided
//! in one place, so no two operations can disagree about them:
//!
//! * an ARN of a cluster-scoped resource carries the cluster that holds it
//!   (`task/<cluster>/<id>`, `service/<cluster>/<name>`,
//!   `service-deployment/<cluster>/<service>/<id>`), so a request that ALSO names a
//!   `cluster` names two scopes — and when they disagree it names no resource at all
//!   ([`cluster_from_scoped_arn`]). That is a bad parameter, not a resource that could not
//!   be found, which is how `DeleteService` and `UpdateService` already answer a service
//!   ARN of another cluster;
//! * a resource whose identifier names a cluster this account holds NO record of cannot be
//!   reported on at all, which is the condition `ClusterNotFoundException` states
//!   ([`ComAmazonawsEcsAmazonec2containerservicev20141113State::names_a_missing_cluster`])
//!   — the same resolution every operation that takes a `cluster` member makes, for the
//!   operations that take the cluster inside an ARN.
//!
//! The capacity providers a cluster may place on are the same kind of scope, decided beside
//! the association itself
//! ([`ComAmazonawsEcsAmazonec2containerservicev20141113State::unassociated_capacity_provider_refusal`]);
//! what the call that SETS the association may leave behind is
//! [`unassociated_default_strategy`] below.
//!
//! Nothing here reads the request's shape: each rule is one comparison between a reference
//! and the state the cluster, or the task, is actually in.

use super::*;

/// `InvalidParameterException$message` for a `service` reference whose ARN names another
/// cluster than the request's `cluster`: Amazon ECS reports that the service is outside the
/// cluster the call was made against. Fixed text, naming neither of the two.
pub const SERVICE_NOT_WITHIN_CLUSTER_MESSAGE: &str = "Service is not within the provided Cluster.";

/// `InvalidParameterException$message` for a `task` reference whose ARN names another
/// cluster than the request's `cluster`: the two identifiers one request carries disagree,
/// which is what Amazon ECS reports. Fixed text, naming neither of the two.
pub const CLUSTER_IDENTIFIERS_MISMATCH_MESSAGE: &str =
    "Invalid identifier: cluster identifiers mismatch";

/// The CLUSTER an ARN of a cluster-scoped resource carries, or `None` when `arn` is not an
/// ARN of this service naming a `resource_type` resource under a cluster.
///
/// Every such ARN this service mints puts the cluster FIRST in the resource path
/// (`<type>/<cluster>/<rest>`), so the cluster is the segment after the resource type — and
/// the older, cluster-less form of a task ARN (`task/<id>`, which carries no cluster at
/// all) answers `None` rather than reading the id as a cluster name.
pub fn cluster_from_scoped_arn(arn: &str, resource_type: &str) -> Option<String> {
    // The ONE parse of an ARN this project reads a caller-supplied value with
    // (`service_common::arn`), so the fields are not split again here: the vendor says the
    // ARN is this service's, and the resource field keeps every `/` of its own.
    let parsed = ::service_common::arn::parse(arn)?;
    if parsed.vendor != ARN_NAMESPACE {
        return None;
    }
    let path = ::service_common::arn::resource_id_of_type(arn, resource_type)?;
    let mut segments = path.split('/');
    let cluster_name = segments.next()?;
    // A path of ONE segment names the resource and no cluster.
    segments.next().is_some().then(|| cluster_name.to_string())
}

/// The cluster a reference of `resource_type` names that is NOT `cluster_name`, or `None`
/// when no reference names another cluster — because none of them carries a cluster at all,
/// or because the one each carries is this one.
///
/// The one reading of [`cluster_from_scoped_arn`] for an operation that resolves a LIST of
/// references inside one `cluster`.
pub fn foreign_cluster_reference<'a>(
    references: impl IntoIterator<Item = &'a str>,
    resource_type: &str,
    cluster_name: &str,
) -> Option<String> {
    references.into_iter().find_map(|reference| {
        cluster_from_scoped_arn(reference.trim(), resource_type)
            .filter(|from_arn| from_arn != cluster_name)
    })
}

/// The refusal a `defaultCapacityProviderStrategy` earns when an entry of it names a
/// provider the same request does NOT associate with the cluster, or `None` when every
/// entry names one of `providers`.
///
/// The call that SETS a cluster's association sets both lists at once, and the strategy is
/// the one the cluster places through — so "A capacity provider must be associated with the
/// cluster to be used in a capacity provider strategy" is, for those calls, a question about
/// the request's own two lists. Each name is read as the provider it REFERS to
/// ([`capacity_provider_name_from_reference`]) on both sides, so naming one provider by
/// short name on one side and by ARN on the other is still one provider.
pub fn unassociated_default_strategy(
    providers: &[String],
    strategy: &[CapacityProviderStrategyItem],
) -> Option<&'static str> {
    let associated: HashSet<String> = providers
        .iter()
        .map(|reference| capacity_provider_name_from_reference(reference))
        .collect();
    strategy
        .iter()
        .any(|item| {
            !associated.contains(&capacity_provider_name_from_reference(&item.capacity_provider))
        })
        .then_some(CAPACITY_PROVIDER_NOT_ASSOCIATED_MESSAGE)
}

impl ComAmazonawsEcsAmazonec2containerservicev20141113State {
    /// Whether a reference of `resource_type` names a cluster this store holds NO record of
    /// — in any status, so a tombstone counts ([`Self::holds_cluster`]) — for the operations
    /// that take the cluster inside a resource ARN rather than in a `cluster` member.
    ///
    /// `false` when `references` names no cluster at all (every reference is cluster-less, or
    /// is not an ARN of `resource_type`) and when every cluster they name is one this
    /// account holds: a read cannot report on the resources of a cluster that was never
    /// created, which is the condition `ClusterNotFoundException` states with one fixed
    /// sentence naming no cluster — so the ANSWER is the condition and not the name.
    pub fn names_a_missing_cluster<'a>(
        &self,
        references: impl IntoIterator<Item = &'a str>,
        resource_type: &str,
    ) -> bool {
        references.into_iter().any(|reference| {
            cluster_from_scoped_arn(reference.trim(), resource_type)
                .is_some_and(|cluster_name| !self.holds_cluster(&cluster_name))
        })
    }
}

impl TaskRecord {
    /// Whether this task runs on FARGATE — the infrastructure Amazon Web Services manages,
    /// rather than capacity registered to the cluster.
    ///
    /// The launch type the task was placed with, and otherwise the capacity provider it was
    /// placed through: "When you use Fargate, the capacity providers are FARGATE or
    /// FARGATE_SPOT" ([`MANAGED_CAPACITY_PROVIDERS`]). Read through the same predicate every
    /// other Fargate question of this service is ([`places_on_fargate`]), so a task cannot
    /// be Fargate for one reader and not for another.
    pub fn runs_on_fargate(&self) -> bool {
        let placed_through: Vec<CapacityProviderStrategyItem> = self
            .capacity_provider_name
            .iter()
            .map(|name| CapacityProviderStrategyItem {
                capacity_provider: name.clone(),
                ..CapacityProviderStrategyItem::default()
            })
            .collect();
        places_on_fargate(self.launch_type.as_deref(), &placed_through)
    }
}

