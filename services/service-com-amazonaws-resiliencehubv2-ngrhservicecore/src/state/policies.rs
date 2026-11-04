// SPDX-License-Identifier: CC0-1.0

//! The resilience-policy side of this service's store.
//!
//! One [`PolicyRecord`] per created policy, keyed by its ARN in
//! [`super::ComAmazonawsResiliencehubv2NgrhservicecoreState::policies`], plus the two
//! secondary indexes the policy operations need (a name index for the duplicate-name
//! `ConflictException`, and a `clientToken` index for idempotent replay) — the same
//! shape the `Service` side of the store uses.
//!
//! Every `Policy` body this crate returns is projected from a record here through
//! [`PolicyRecord::to_policy`], so `CreatePolicy` and `GetPolicy` cannot drift from one
//! another or from what was stored.

use std::collections::BTreeMap;
use std::time::SystemTime;

use crate::ops::types::{
    AvailabilitySlo, DataRecoveryTargets, MultiAzTargets, MultiRegionTargets, Policy,
};

use super::{
    tag_map_view, to_date_time, ComAmazonawsResiliencehubv2NgrhservicecoreState,
    ARN_SERVICE_NAMESPACE,
};

/// The resource type a resilience policy's ARN carries.
///
/// From the model's own `IamPolicyResource`, whose `@iamResource` name is `policy`: the
/// resource segment of an ARN is that resource name, the way a service's is `service`.
pub const POLICY_RESOURCE: &str = "policy";

/// The ceiling on how many policies this store will hold, so an unauthenticated caller
/// cannot grow the process's memory without limit by repeating `CreatePolicy`.
///
/// CHOSEN, not documented: the model publishes no policy quota, so this mirrors the
/// ceiling the `Service` side of this store uses ([`super::MAX_SERVICES`]).
/// `CreatePolicy` refuses a create that would pass it with the
/// `ServiceQuotaExceededException` (HTTP 402) its model declares. The two secondary
/// indexes are bounded by the same ceiling: `policy_names` holds exactly one entry per
/// stored policy, and `policy_idempotency` at most one (a replayed token returns the
/// stored policy instead of inserting a second one).
pub const MAX_POLICIES: usize = super::MAX_SERVICES;

/// One created resilience policy, and the sole source of every `Policy` body this crate
/// returns.
///
/// The four target blocks (`availabilitySlo`, `multiAz`, `multiRegion`, `dataRecovery`)
/// are stored exactly as the caller set them — each is optional in the model, and a
/// policy that sets none of them is a policy with no targets, not one with zeroed ones.
#[derive(Debug, Clone)]
pub struct PolicyRecord {
    /// The minted ARN; also this record's key in
    /// [`super::ComAmazonawsResiliencehubv2NgrhservicecoreState::policies`].
    pub policy_arn: String,
    pub name: String,
    pub description: Option<String>,
    pub availability_slo: Option<AvailabilitySlo>,
    pub multi_az: Option<MultiAzTargets>,
    pub multi_region: Option<MultiRegionTargets>,
    pub data_recovery: Option<DataRecoveryTargets>,
    /// Whether cross-account sharing is enabled for this policy. A policy created
    /// without the member has sharing off, so this is the flag's stored state rather
    /// than a member the projection withholds.
    pub sharing_enabled: bool,
    pub kms_key_id: Option<String>,
    /// Stored ordered so a tag map reads back the same way every time.
    pub tags: BTreeMap<String, String>,
    pub created_at: SystemTime,
    pub updated_at: SystemTime,
    /// Creation order, from the store's monotonic counter — the order a listing scans
    /// in, and the position a page token names.
    pub sequence: u64,
}

impl PolicyRecord {
    /// This record as the `Policy` shape `CreatePolicy` / `GetPolicy` return.
    ///
    /// `associatedServiceCount` is "the number of services associated with this policy",
    /// so it is COUNTED from the services in the store rather than kept as a counter
    /// beside them — see
    /// [`ComAmazonawsResiliencehubv2NgrhservicecoreState::associated_service_count`] —
    /// and is passed in because a record does not reach the services that reference it.
    ///
    /// `organizationId` is left absent: nothing in this crate shares a policy with an
    /// organization, so there is no organization to name.
    pub fn to_policy(&self, associated_service_count: i32) -> Policy {
        Policy {
            policy_arn: self.policy_arn.clone(),
            name: self.name.clone(),
            description: self.description.clone(),
            availability_slo: self.availability_slo.clone(),
            multi_az: self.multi_az.clone(),
            multi_region: self.multi_region.clone(),
            data_recovery: self.data_recovery.clone(),
            sharing_enabled: Some(self.sharing_enabled),
            organization_id: None,
            kms_key_id: self.kms_key_id.clone(),
            tags: tag_map_view(&self.tags),
            associated_service_count: Some(associated_service_count),
            created_at: Some(to_date_time(self.created_at)),
            updated_at: Some(to_date_time(self.updated_at)),
        }
    }
}

impl ComAmazonawsResiliencehubv2NgrhservicecoreState {
    /// How many stored services reference `policy_arn` as their policy.
    ///
    /// Counted on demand from the services themselves, so attaching or detaching a
    /// policy, or deleting a service, cannot leave a stale count behind.
    pub fn associated_service_count(&self, policy_arn: &str) -> i32 {
        let count = self
            .services
            .values()
            .filter(|service| service.policy_arn.as_deref() == Some(policy_arn))
            .count();
        i32::try_from(count).unwrap_or(i32::MAX)
    }

    /// The `Policy` body for `policy_arn`, or `None` when no policy is stored under it.
    ///
    /// The one projection path every policy reader answers from.
    pub fn policy_view(&self, policy_arn: &str) -> Option<Policy> {
        let record = self.policies.get(policy_arn)?;
        Some(record.to_policy(self.associated_service_count(policy_arn)))
    }

    /// The policy stored under `name`, if any — what a create consults before storing a
    /// second policy under a name already taken.
    pub fn policy_named(&self, name: &str) -> Option<&PolicyRecord> {
        let arn = self.policy_names.get(name)?;
        self.policies.get(arn)
    }

    /// Every stored policy in creation order.
    pub fn policies_in_creation_order(&self) -> Vec<&PolicyRecord> {
        let mut records: Vec<&PolicyRecord> = self.policies.values().collect();
        records.sort_by_key(|record| record.sequence);
        records
    }
}

/// Mint a fresh policy ARN: `arn:aws:resiliencehub:<region>:<account>:policy/<uuid>`.
///
/// The account segment is the emulator's canonical one, through `engine`'s
/// [`emulator_arn`](::engine::emulator_arn); the resource id is a fresh UUID, so two
/// policies never collide even under the same name after a delete.
pub fn mint_policy_arn(region: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE_NAMESPACE,
        region,
        &format!("{POLICY_RESOURCE}/{}", super::mint_uuid(region)),
    )
}
