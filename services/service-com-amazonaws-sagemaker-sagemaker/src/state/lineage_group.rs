// SPDX-License-Identifier: CC0-1.0

//! The lineage-group slice of this service's state: the [`LineageGroup`] record
//! `DescribeLineageGroup` reports, and the lookups its readers share.

use super::*;

/// A lineage group, as `DescribeLineageGroup` reports it.
///
/// SageMaker models no `CreateLineageGroup`: a lineage group comes into being with the
/// account's lineage entities (every artifact, action, context and trial component
/// belongs to one), so a record here is materialised through
/// [`ComAmazonawsSagemakerSagemakerState::ensure_lineage_group`] rather than by a create
/// handler — and a group nothing has materialised is simply absent from the store, which
/// is what the modelled `ResourceNotFound` reports.
#[derive(Debug, Clone)]
pub struct LineageGroup {
    pub name: String,
    pub arn: String,
    pub display_name: Option<String>,
    pub description: Option<String>,
    pub created_by: Option<UserContext>,
    pub last_modified_by: Option<UserContext>,
    pub creation_time: SystemTime,
    pub last_modified_time: SystemTime,
    /// The resource-based policy document that gives another account access to this
    /// group, as `GetLineageGroupPolicy` reports it: an IAM policy document, and
    /// [`EMPTY_RESOURCE_POLICY`] while nothing has shared the group.
    ///
    /// Held on the group's own record rather than in a second map, so the policy a group
    /// carries lives beside the group a describe reports and cannot outlive it. The whole
    /// SageMaker surface models no operation that attaches one — a lineage group is shared
    /// with another account by a resource share, not by a call on this service — so a
    /// group this service materialises carries the empty document.
    pub resource_policy: String,
}

impl ComAmazonawsSagemakerSagemakerState {
    /// The lineage group `region` holds under `name`, or `None` when no lineage entity
    /// has materialised it there.
    ///
    /// A lineage group is regional, so the Region is half the key: a group materialised
    /// by an entity created in one Region is not reported to a caller signing for
    /// another, which is what keeps a replayed `LineageGroupArn` naming the Region the
    /// reader signed for.
    pub fn lineage_group(&self, region: &str, name: &str) -> Option<&LineageGroup> {
        self.lineage_groups
            .get(&(region.to_string(), name.to_string()))
    }

    /// Every lineage group `region` holds, in `LineageGroupName` order.
    ///
    /// The Region is half the key (see [`Self::lineage_group`]), so a listing reports only
    /// the groups the caller's Region holds: a group an entity materialised elsewhere is
    /// not this caller's, exactly as the describe does not report it. One accessor for it,
    /// so `ListLineageGroups` and `DescribeLineageGroup` cannot disagree about which
    /// groups a Region has.
    pub fn lineage_groups_in<'a>(
        &'a self,
        region: &'a str,
    ) -> impl Iterator<Item = &'a LineageGroup> + 'a {
        self.lineage_groups
            .iter()
            .filter(move |((group_region, _), _)| group_region.as_str() == region)
            .map(|(_, group)| group)
    }

    /// Materialise the lineage group `name` in `region` unless the store already holds
    /// it, and answer the ARN of the group this call CREATED.
    ///
    /// This is the ONLY way a lineage group enters the store, because SageMaker models no
    /// `CreateLineageGroup`: the service materialises a group when the account's lineage
    /// entities put something in it, so the lineage-entity creates call this with
    /// [`DEFAULT_LINEAGE_GROUP`] — `CreateArtifact`, `CreateAction`, `CreateContext` and
    /// `CreateTrialComponent` all do. Readers never call it: a reporting operation must
    /// not mint a resource.
    ///
    /// `DisplayName` is the group's own name: nothing in the API supplies one for a group
    /// the service materialises itself, and "if `DisplayName` isn't specified, the name is
    /// displayed" is the rule every other SageMaker entity's model doc states.
    /// `CreatedBy`/`LastModifiedBy` stay absent — there is no Studio user context behind a
    /// group the service created, and an invented one would be a value no request made.
    ///
    /// `None` means nothing changed: either that Region already held that group (so a
    /// second entity does not move its stored instants), or the store is at
    /// [`LINEAGE_GROUP_QUOTA`] and cannot take another, which bounds the growth a caller
    /// varying the Region or the name could otherwise cause. A caller that gets
    /// `Some(arn)` records the state change under that ARN, so a materialised group is
    /// auditable like any create. The at-the-bound case is WARNED about here rather than
    /// by each caller, so every lineage-entity create reports it the same way and none
    /// can drift.
    pub fn ensure_lineage_group(
        &mut self,
        region: &str,
        name: &str,
        now: SystemTime,
    ) -> Option<String> {
        // Read before the entry() borrow, so the bound is checked against the same store
        // this insert grows.
        let at_capacity = self.lineage_groups.len() >= LINEAGE_GROUP_QUOTA;
        match self
            .lineage_groups
            .entry((region.to_string(), name.to_string()))
        {
            ::std::collections::btree_map::Entry::Occupied(_) => None,
            ::std::collections::btree_map::Entry::Vacant(_) if at_capacity => {
                // Visible rather than silent: past the bound the entity this call creates
                // still reports its Region's lineage-group ARN, which
                // `DescribeLineageGroup` cannot answer until the store is back under it.
                tracing::warn!(
                    lineage_group = name,
                    region,
                    bound = LINEAGE_GROUP_QUOTA,
                    "the lineage-group store is at its bound, so this Region's lineage \
                     group was not materialised"
                );
                None
            }
            ::std::collections::btree_map::Entry::Vacant(slot) => {
                let arn = named_lineage_group_arn(region, name);
                slot.insert(LineageGroup {
                    name: name.to_string(),
                    arn: arn.clone(),
                    display_name: Some(name.to_string()),
                    description: None,
                    created_by: None,
                    last_modified_by: None,
                    creation_time: now,
                    last_modified_time: now,
                    // Nothing has shared a group the service has just materialised, so it
                    // carries the empty policy document.
                    resource_policy: EMPTY_RESOURCE_POLICY.to_string(),
                });
                Some(arn)
            }
        }
    }
}
