// SPDX-License-Identifier: CC0-1.0

//! The compute-allocation slice of this service's state: the [`ComputeQuota`] record,
//! its version and id mint, its activation default and its clock-owed span.

use super::*;

/// How long after `CreateComputeQuota` the clock owes the `Creating` -> `Created`
/// transition, for the same reason [`ENDPOINT_IN_SERVICE_AFTER`] exists: a caller that
/// describes or lists the quota straight after creating it still observes `Creating`,
/// one that waits observes `Created`. Real SageMaker publishes no span for it; this one
/// is the emulator's own, compressed and never zero.
pub const COMPUTE_QUOTA_CREATED_AFTER: Duration = Duration::from_millis(300);

/// `SchedulerResourceStatus`'s members a compute quota reaches in this emulator: a
/// create is reported `Creating` until the clock owes `Created`. The modelled enum also
/// carries the update, rollback and delete states, which this slice does not reach.
/// Named separately from [`STATUS_CREATING`] (an `EndpointStatus` member that happens to
/// share the literal) so each handler compares against its own modelled enum.
pub const STATUS_QUOTA_CREATING: &str = "Creating";
/// See [`STATUS_QUOTA_CREATING`].
pub const STATUS_QUOTA_CREATED: &str = "Created";

/// `ActivationState`'s documented default ("Use to enable or disable compute
/// allocation. Default is `Enabled`.").
pub const DEFAULT_ACTIVATION_STATE: &str = "Enabled";

/// The version `CreateComputeQuota` stamps on a new compute allocation definition;
/// `UpdateComputeQuota` is the only operation that moves it, so every quota this slice
/// holds carries this one.
pub const FIRST_COMPUTE_QUOTA_VERSION: i32 = 1;

/// The length `ComputeQuotaId`'s modelled pattern `^[a-z0-9]{12}$` fixes.
pub(super) const COMPUTE_QUOTA_ID_LEN: usize = 12;

/// How many draws [`ComAmazonawsSagemakerSagemakerState::mint_compute_quota_id`] takes
/// from [`mint_opaque_resource_id`] before falling back. One suffices unless its cut
/// collides with a stored id; the bound is what keeps the minting terminating.
pub(super) const COMPUTE_QUOTA_ID_DRAWS: u32 = 16;

/// A compute allocation definition created by `CreateComputeQuota`, as
/// `DescribeComputeQuota` replays it and `ListComputeQuotas` summarises it.
///
/// `created_status_at` is the instant the clock owes the `Creating` -> `Created`
/// transition; it is stored, never slept on, and materialised by whichever reporting
/// call first runs at or after it (see [`ComputeQuota::settle_status`]).
///
/// The two configuration shapes are named by full path rather than through the module's
/// shared import list, the way the slices above do it: that list is the hottest line in
/// this file, and a slice that stays out of it stays out of its merge conflicts.
#[derive(Debug, Clone)]
pub struct ComputeQuota {
    pub id: String,
    pub arn: String,
    pub name: String,
    pub description: Option<String>,
    pub version: i32,
    pub status: String,
    pub failure_reason: Option<String>,
    pub cluster_arn: String,
    pub config: crate::ops::types::ComputeQuotaConfig,
    pub target: crate::ops::types::ComputeQuotaTarget,
    pub activation_state: String,
    pub tags: Vec<Tag>,
    pub creation_time: SystemTime,
    pub last_modified_time: SystemTime,
    pub created_status_at: SystemTime,
}

impl ComputeQuota {
    /// Materialise the `Creating` -> `Created` transition the clock already owes, the
    /// way [`Endpoint::settle_status`] does: it was due at `created_status_at` whether
    /// or not anybody read the quota, so the observer makes no difference to the
    /// result, and `LastModifiedTime` moves to the instant it was due rather than to
    /// "now".
    ///
    /// Answers whether the transition happened on this call, so the caller can record it.
    pub fn settle_status(&mut self, now: SystemTime) -> bool {
        if self.status == STATUS_QUOTA_CREATING && now >= self.created_status_at {
            self.status = STATUS_QUOTA_CREATED.to_string();
            self.last_modified_time = self.created_status_at;
            return true;
        }
        false
    }
}

impl ComAmazonawsSagemakerSagemakerState {
    /// A `ComputeQuotaId` no record in this store holds. A second inherent `impl` block,
    /// beside this slice's own types rather than in the one above.
    ///
    /// The id is the server's, not derived from the request: `CreateComputeQuota` twice
    /// with the same input in two clusters mints two distinct quotas. It comes from this
    /// module's ONE id mint, [`mint_opaque_resource_id`] (32 lowercase hex digits), cut
    /// to the 12 characters `ComputeQuotaId`'s `^[a-z0-9]{12}$` fixes — hex is a subset
    /// of that class, so the id satisfies the pattern by construction — the way
    /// [`mint_cluster_id`] derives its own. The loop re-draws on the vanishing
    /// chance that the cut collides with a stored id, and is bounded: past
    /// [`COMPUTE_QUOTA_ID_DRAWS`] draws the fallback walks the 12-digit lowercase-hex
    /// numbers until one is free, which terminates because the store is finite.
    ///
    /// `CreateComputeQuota` is still a scaffold stub (its own issue owns it, together
    /// with the `ClusterArn`-existence precondition SageMaker applies before it stores
    /// anything), so the minting seam is exercised today by this crate's compute-quota
    /// tests, which seed the store the create will write.
    pub fn mint_compute_quota_id(&self) -> String {
        for _ in 0..COMPUTE_QUOTA_ID_DRAWS {
            let minted = mint_opaque_resource_id();
            // The mint yields 32 hex digits, so the cut is always `COMPUTE_QUOTA_ID_LEN`
            // long; only a collision with a stored id makes this draw unusable.
            let id: String = minted.chars().take(COMPUTE_QUOTA_ID_LEN).collect();
            if !self.compute_quotas.contains_key(&id) {
                return id;
            }
        }
        (0..=u64::try_from(self.compute_quotas.len()).unwrap_or(u64::MAX))
            .map(|sequence| format!("{sequence:012x}"))
            .find(|id| !self.compute_quotas.contains_key(id))
            .unwrap_or_else(|| "0".repeat(COMPUTE_QUOTA_ID_LEN))
    }
}
