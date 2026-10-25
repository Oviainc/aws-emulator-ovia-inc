// SPDX-License-Identifier: CC0-1.0

//! The WAIT CONDITIONS of one stack operation: the barrier an
//! `AWS::CloudFormation::WaitCondition` puts in a create or an update, and the signals
//! that release it.
//!
//! CloudFormation "doesn't proceed with a stack creation or update until resources receive
//! the required number of signals or the timeout period is exceeded"
//! (`SignalResource`): a template that declares a wait condition therefore has an
//! operation whose completion is not owed by the clock alone — it is owed once the wait
//! condition has the success signals its `Count` asks for, and the resources that depend
//! on the wait condition are not started before that.
//!
//! This module is the model of that barrier, and nothing else:
//!
//! * [`DeclaredWaitCondition`] is what a DEPLOYMENT computes — one entry per wait
//!   condition the template declares, with the number of signals it waits for, how long
//!   it waits (bounded by the documented maximum, so no template leaves a stack in
//!   progress for ever), and the resources of the same template whose creation waits on
//!   it. It is
//!   computed where the template is read ([`crate::state::StoredStack::deploy_template`]),
//!   outside the store lock, because reading a caller-sized body is work the request pays
//!   for.
//! * [`WaitConditionBarrier`] is one of those, OPENED by the operation now in flight: the
//!   plan plus the instant that operation started.
//!   [`crate::state::StoredStack::settle`] will not complete an operation that still holds
//!   an unsatisfied barrier, and whether one is satisfied is read off the signals
//!   `SignalResource` recorded on the stack.
//!
//! ## Bounds
//!
//! Nothing here grows with what a caller sends: a barrier is the plan plus one instant,
//! and the signals themselves are the stack's own bounded log
//! ([`crate::state::MAX_SIGNALS_PER_STACK`]). The `Timeout` a template may declare is
//! bounded by its own documented maximum ([`MAX_TIMEOUT`]), and reading the template's
//! wait conditions out of a caller-supplied body is ONE pass over it whatever it declares
//! ([`declared`]), charged to that deployment's evaluation budget.

use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, SystemTime};

use super::template;
use super::StoredResource;

/// The resource type whose creation waits for signals, as this crate's template reader
/// names it ([`template::WAIT_CONDITION`]).
pub use super::template::WAIT_CONDITION as WAIT_CONDITION_TYPE;

/// How many success signals a wait condition that declares no `Count` waits for.
///
/// `AWS::CloudFormation::WaitCondition`'s `Count` — "the number of success signals that
/// CloudFormation must receive before it continues the stack creation process" — is not a
/// required property, and a wait condition that states no count continues on one signal.
pub const DEFAULT_SIGNAL_COUNT: usize = 1;

/// The longest a wait condition waits: "the maximum time that can be specified for this
/// property is 12 hours (43200 seconds)" (`AWS::CloudFormation::WaitCondition`'s
/// `Timeout`).
///
/// The documented ceiling of a duration the TEMPLATE asks for, so a declaration past it
/// waits this long rather than as long as it asked.
pub const MAX_TIMEOUT: Duration = Duration::from_secs(43_200);

/// One wait condition a deployment found in the template it is deploying.
///
/// Computed once, where the template is read, from the resource record the deployment
/// materialised for it: the properties here are the declaration's own with every intrinsic
/// function already evaluated, so nothing re-reads the body later.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeclaredWaitCondition {
    /// The logical id the template declares the wait condition under, which is the
    /// `LogicalResourceId` a signal names.
    pub logical_id: String,
    /// `Count`: how many success signals release it.
    pub count: usize,
    /// `Timeout`: how long it waits for them, bounded by [`MAX_TIMEOUT`].
    pub timeout: Duration,
    /// The logical ids of the resources of the same template whose creation waits on this
    /// one: every resource that depends on it, directly or through another resource.
    /// CloudFormation creates a resource only once the resources it needs exist, so these
    /// are the ones the operation has not started while the barrier holds.
    pub blocks: BTreeSet<String>,
}

impl DeclaredWaitCondition {
    /// The wait condition `resource` declares, with the resources it blocks, or `None` for
    /// a resource that is not a wait condition.
    pub fn of(resource: &StoredResource, blocks: BTreeSet<String>) -> Option<Self> {
        if resource.resource_type != WAIT_CONDITION_TYPE {
            return None;
        }
        Some(Self {
            logical_id: resource.logical_id.clone(),
            count: declared_count(resource),
            timeout: declared_timeout(resource),
            blocks,
        })
    }
}

/// `Count` as the declaration states it, or [`DEFAULT_SIGNAL_COUNT`].
///
/// A template may spell a numeric property as a number or as a string (both forms deploy),
/// so both are read; a value that is neither is no count at all and the default stands.
/// Zero is not a number of signals to wait for, so it is read as the default too.
fn declared_count(resource: &StoredResource) -> usize {
    let declared = resource
        .properties
        .get("Count")
        .and_then(|count| match count {
            ::serde_json::Value::Number(number) => number.as_u64(),
            ::serde_json::Value::String(text) => text.parse::<u64>().ok(),
            _ => None,
        })
        .unwrap_or(DEFAULT_SIGNAL_COUNT as u64);
    let declared = usize::try_from(declared).unwrap_or(usize::MAX);
    declared.max(DEFAULT_SIGNAL_COUNT)
}

/// `Timeout` as the declaration states it, in seconds, bounded by [`MAX_TIMEOUT`].
///
/// `Timeout` is a REQUIRED property of `AWS::CloudFormation::WaitCondition`, so a
/// declaration that omits it — or states something that is not a number of seconds — is
/// held to the documented maximum rather than waiting without a deadline: no template can
/// leave a stack in progress for ever.
fn declared_timeout(resource: &StoredResource) -> Duration {
    resource
        .properties
        .get("Timeout")
        .and_then(|timeout| match timeout {
            ::serde_json::Value::Number(number) => number.as_u64(),
            ::serde_json::Value::String(text) => text.parse::<u64>().ok(),
            _ => None,
        })
        .map_or(MAX_TIMEOUT, |seconds| {
            Duration::from_secs(seconds).min(MAX_TIMEOUT)
        })
}

/// Every wait condition `resources` carries, with the resources each one blocks.
///
/// `template` is the body those records were materialised from, which is what says what
/// depends on what: a resource that names a wait condition in a `DependsOn`, a `Ref` or an
/// `Fn::GetAtt` — or that names something which does — is one the operation cannot start
/// until that wait condition is satisfied.
///
/// ONE pass over the template for the whole set ([`template::waits_on_each`]), charged to
/// the deployment's budget, however many wait conditions the body declares: the relation
/// is computed once and inverted, rather than walked per wait condition.
pub fn declared(
    template: &template::Template,
    resources: &[StoredResource],
    budget: &mut template::Budget,
) -> Vec<DeclaredWaitCondition> {
    let roots: BTreeSet<String> = resources
        .iter()
        .filter(|resource| resource.resource_type == WAIT_CONDITION_TYPE)
        .map(|resource| resource.logical_id.clone())
        .collect();
    if roots.is_empty() {
        return Vec::new();
    }
    // The relation the other way round: per wait condition, the resources that wait on it.
    let mut blocked: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for (logical_id, waits_on) in template::waits_on_each(template, &roots, budget) {
        for root in waits_on {
            blocked.entry(root).or_default().insert(logical_id.clone());
        }
    }
    resources
        .iter()
        .filter(|resource| resource.resource_type == WAIT_CONDITION_TYPE)
        .filter_map(|resource| {
            let blocks = blocked.remove(&resource.logical_id).unwrap_or_default();
            DeclaredWaitCondition::of(resource, blocks)
        })
        .collect()
}

/// One wait condition the operation in flight is waiting at: what the template declared,
/// and when the wait started.
///
/// The SIGNALS it waits for are not held here: `SignalResource` records every signal on
/// the stack itself ([`crate::state::StoredStack::record_signal`], keyed by the signalled
/// logical id and the signal's `UniqueId` and bounded there), so the one record of what
/// has arrived is that log, and whether this barrier is satisfied is a question about it
/// ([`crate::state::StoredStack::barrier_satisfied`]).
#[derive(Debug, Clone)]
pub struct WaitConditionBarrier {
    /// What the deployment computed for this wait condition.
    pub declared: DeclaredWaitCondition,
    /// The instant the operation that opened the barrier started — which is when the wait
    /// began, and therefore what its `Timeout` runs from.
    pub opened_at: SystemTime,
}

impl WaitConditionBarrier {
    /// Open the barrier of an operation starting at `now`.
    pub fn open(declared: DeclaredWaitCondition, now: SystemTime) -> Self {
        Self {
            declared,
            opened_at: now,
        }
    }

    /// The logical id a signal names this wait condition by.
    pub fn logical_id(&self) -> &str {
        &self.declared.logical_id
    }

    /// How many success signals release it (`Count`).
    pub fn count(&self) -> usize {
        self.declared.count
    }

    /// Whether this resource's creation waits on this wait condition.
    pub fn blocks(&self, logical_id: &str) -> bool {
        self.declared.blocks.contains(logical_id)
    }

    /// The instant the wait gives up.
    pub fn deadline(&self) -> SystemTime {
        self.opened_at + self.declared.timeout
    }

    /// Whether the wait has run out at `now` — which fails the operation for a wait
    /// condition that is not satisfied by then: "if the wait condition doesn't receive the
    /// specified number of success signals before the `Timeout` period expires,
    /// CloudFormation assumes that the wait condition has failed".
    pub fn has_expired(&self, now: SystemTime) -> bool {
        self.deadline() <= now
    }

    /// Why the operation failed, for a wait condition whose wait ran out: what it was
    /// waiting for and what it had `received`.
    pub fn timed_out_reason(&self, received: usize) -> String {
        format!(
            "WaitCondition timed out. Received {received} conditions when expecting {}",
            self.declared.count
        )
    }
}
