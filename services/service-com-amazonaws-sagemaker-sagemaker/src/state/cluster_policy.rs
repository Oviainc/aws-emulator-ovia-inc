// SPDX-License-Identifier: CC0-1.0

//! The cluster-policy slice of this service's state: the cluster policies
//! (`ClusterSchedulerConfig`s) that hang off a HyperPod cluster.
//!
//! One submodule rather than more lines in [`crate::state`], because this is one
//! resource kind with its own key, quota, status span and id mint: a reader looking for
//! how a cluster policy is stored finds all of it here, and the parent module carries a
//! single [`Store`] field for it.
//!
//! The parent module keeps the service-wide pieces this one builds on
//! (`resource_arn`, `timestamp`, `MAX_PAGE_SIZE`, the page-token codec) AND the HyperPod
//! CLUSTER a policy belongs to: a cluster is `CreateCluster`'s resource, stored once in
//! [`crate::state::ComAmazonawsSagemakerSagemakerState::clusters`] and resolved by ARN
//! through [`crate::state::ComAmazonawsSagemakerSagemakerState::cluster_by_arn`].
//! Nothing is duplicated here.

use std::collections::BTreeMap;
use std::time::{Duration, SystemTime};

use crate::ops::types::{SchedulerConfig, Tag};

/// How long after `CreateClusterSchedulerConfig` the clock owes the `Creating` ->
/// `Created` transition (`SchedulerResourceStatus` models both).
///
/// A caller that describes or lists the policy straight after creating it still observes
/// `Creating`; one that waits observes `Created`. Real SageMaker takes its own time; the
/// span here is compressed, never zero, and the due instant is STORED rather than slept
/// on — see [`Config::settle_status`].
pub const CREATED_AFTER: Duration = Duration::from_millis(300);

/// The most cluster policies this store holds, after which a create is refused with the
/// modelled `ResourceLimitExceeded`.
///
/// Same contract as [`crate::state::MODEL_QUOTA`]: the emulator's OWN bound on the
/// store, not a claim about any account's limit, and what stops the state growing
/// without end. Unlike the stores there, this one is not released yet —
/// `DeleteClusterSchedulerConfig` is still a scaffold stub with its own issue — so the
/// bound is what keeps it finite until that operation lands.
pub const QUOTA: usize = 1_000;

/// `SchedulerResourceStatus`'s members a cluster policy's create path reaches (the
/// modelled enum also carries the update, rollback and delete states, which this slice
/// does not reach). Named once here so no handler compares a bare string literal.
pub const STATUS_CREATING: &str = "Creating";
/// See [`STATUS_CREATING`].
pub const STATUS_CREATED: &str = "Created";

/// The version `CreateClusterSchedulerConfig` mints a cluster policy at;
/// `UpdateClusterSchedulerConfig` moves it on from there.
pub const FIRST_VERSION: i32 = 1;

/// The length of a `ClusterSchedulerConfigId` (modelled `len 0..12`, pattern
/// `^[a-z0-9]{12}$`), so a minted id satisfies the shape it is serialised into.
pub const ID_LEN: usize = 12;

/// A cluster policy created by `CreateClusterSchedulerConfig`, as
/// `DescribeClusterSchedulerConfig` replays it and `ListClusterSchedulerConfigs`
/// summarises it.
///
/// Keyed in the store by `ClusterSchedulerConfigId` — the id is what every other
/// operation on this resource addresses it by — while `name` stays unique within one
/// `cluster_arn`, which is what makes a duplicate-name create in the same cluster a
/// conflict.
///
/// `created_status_due` is the instant the clock owes the `Creating` -> `Created`
/// transition; it is stored, never slept on, and materialised by whichever reporting
/// call first runs at or after it.
#[derive(Debug, Clone)]
pub struct Config {
    pub id: String,
    pub arn: String,
    pub name: String,
    pub cluster_arn: String,
    pub scheduler_config: SchedulerConfig,
    pub description: Option<String>,
    pub tags: Vec<Tag>,
    pub version: i32,
    pub status: String,
    pub creation_time: SystemTime,
    pub last_modified_time: SystemTime,
    pub created_status_due: SystemTime,
}

impl Config {
    /// Materialise the `Creating` -> `Created` transition the clock already owes, the
    /// way [`crate::state::Endpoint::settle_status`] does for an endpoint: the
    /// transition was due at `created_status_due` whether or not anybody called, so the
    /// observer makes no difference to the result, and `LastModifiedTime` moves to the
    /// instant it was due rather than to "now" so the value is stable across later reads.
    ///
    /// Answers whether the transition happened on this call, so the caller can record it.
    pub fn settle_status(&mut self, now: SystemTime) -> bool {
        if self.status == STATUS_CREATING && now >= self.created_status_due {
            self.status = STATUS_CREATED.to_string();
            self.last_modified_time = self.created_status_due;
            return true;
        }
        false
    }
}

/// This slice's store, held as ONE field of
/// [`crate::state::ComAmazonawsSagemakerSagemakerState`] so every handler reaches the
/// same policies.
#[derive(Debug, Default)]
pub struct Store {
    /// Keyed by `ClusterSchedulerConfigId`.
    pub configs: BTreeMap<String, Config>,
}

impl Store {
    /// Whether another cluster policy fits, i.e. the store is below [`QUOTA`].
    pub fn below_quota(&self) -> bool {
        self.configs.len() < QUOTA
    }

    /// The cluster policy called `name` in the cluster `cluster_arn`.
    ///
    /// A policy's name is unique WITHIN its cluster, not across the store: "The name
    /// must be unique within the SageMaker AI HyperPod cluster specified by
    /// `ClusterArn`. You can use the same name in other clusters within a Region or
    /// across Regions" (`CreateClusterSchedulerConfig$Name`).
    pub fn named(&self, cluster_arn: &str, name: &str) -> Option<&Config> {
        self.configs
            .values()
            .find(|config| config.cluster_arn == cluster_arn && config.name == name)
    }

    /// Mint the next free `ClusterSchedulerConfigId`.
    ///
    /// The id comes from the service's ONE minter,
    /// [`crate::state::mint_opaque_resource_id`] (counter plus wall clock, hex rendered;
    /// opaque, per-run and derived from nothing in the request), truncated to the
    /// modelled [`ID_LEN`] — hex digits satisfy the id's `^[a-z0-9]{12}$` pattern. A
    /// truncation collision is re-minted rather than assumed away, so no two policies
    /// can share an id.
    pub fn mint_id(&self) -> String {
        loop {
            let id: String = crate::state::mint_opaque_resource_id()
                .chars()
                .take(ID_LEN)
                .collect();
            if !self.configs.contains_key(&id) {
                return id;
            }
        }
    }
}
