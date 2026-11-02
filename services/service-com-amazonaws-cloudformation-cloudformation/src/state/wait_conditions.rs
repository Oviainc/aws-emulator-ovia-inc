// SPDX-License-Identifier: CC0-1.0

//! The RESOURCE-SIGNAL WAITS of one stack operation: the barrier a resource whose
//! declaration asks for signals puts in a create or an update, and the signals that release
//! it.
//!
//! CloudFormation "doesn't proceed with a stack creation or update until resources receive
//! the required number of signals or the timeout period is exceeded"
//! (`SignalResource`). TWO declarations ask for that wait, and this module models both,
//! because the wait is the same thing either way — the resource stays in its
//! `*_IN_PROGRESS` status until the signals arrive, and nothing that depends on it is
//! started before that:
//!
//! * an `AWS::CloudFormation::WaitCondition`, whose `Count` and `Timeout` properties say
//!   how many signals it waits for and for how long; and
//! * a `CreationPolicy`'s `ResourceSignal` on a resource of ANY type — "when
//!   CloudFormation creates the associated resource, it waits for a success signal" — whose
//!   `Count` and `Timeout` members say the same two things, the timeout as an ISO 8601
//!   duration.
//!
//! A template that declares either therefore has an operation whose completion is not owed
//! by the clock alone: it is owed once every such resource has the success signals its
//! `Count` asks for, and the resources that depend on one are not started before that.
//!
//! This module is the model of that barrier, and nothing else:
//!
//! * [`DeclaredWaitCondition`] is what a DEPLOYMENT computes — one entry per resource of the
//!   template whose declaration waits for signals, with the number of signals it waits for,
//!   how long it waits (bounded by the documented maximum, so no template leaves a stack in
//!   progress for ever), which declaration opened it, and the resources of the same template
//!   whose creation waits on it. It is
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
//! bounded by its own documented maximum ([`MAX_TIMEOUT`]), and a declaration that states
//! none is held to the documented default ([`DEFAULT_TIMEOUT`]) rather than to no deadline,
//! so every wait has an instant it gives up at and no declaration holds a stack in progress
//! for longer than the published ceiling of the property that opened it. Reading the waits
//! out of a caller-supplied body is ONE pass over it whatever it declares ([`declared`]),
//! charged to that deployment's evaluation budget.

use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, SystemTime};

use super::template;
use super::StoredResource;

/// The resource type whose creation waits for signals by TYPE, as this crate's template
/// reader names it ([`template::WAIT_CONDITION`]).
pub use super::template::WAIT_CONDITION as WAIT_CONDITION_TYPE;

/// How many success signals a declaration that states no `Count` waits for.
///
/// `AWS::CloudFormation::WaitCondition`'s `Count` — "the number of success signals that
/// CloudFormation must receive before it continues the stack creation process" — is not a
/// required property, and a wait condition that states no count continues on one signal.
/// `CreationPolicy`'s `ResourceSignal.Count` documents the same default ("Default: 1").
pub const DEFAULT_SIGNAL_COUNT: usize = 1;

/// The longest a wait runs: "the maximum time that can be specified for this property is 12
/// hours (43200 seconds)" (`AWS::CloudFormation::WaitCondition`'s `Timeout`), which is the
/// same ceiling `CreationPolicy`'s `ResourceSignal.Timeout` documents ("Maximum: PT12H").
///
/// The documented ceiling of a duration the TEMPLATE asks for, so a declaration past it
/// waits this long rather than as long as it asked.
pub const MAX_TIMEOUT: Duration = Duration::from_secs(43_200);

/// How long a wait runs for when the declaration that opened it states no timeout at all.
///
/// `CreationPolicy`'s `ResourceSignal.Timeout` — "the amount of time that CloudFormation
/// waits for the number of signals that the `Count` property specifies" — documents
/// "Default: PT5M", which is this span.
/// `AWS::CloudFormation::WaitCondition`'s own `Timeout` is a REQUIRED property, so a
/// declaration that omits it has asked for no particular wait and is held to that same
/// documented default rather than left without a deadline: no template leaves a stack in
/// progress for ever.
///
/// A declaration that DOES state a timeout waits exactly as long as it asked, bounded by
/// [`MAX_TIMEOUT`] ([`WaitConditionBarrier::deadline`]).
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(300);

/// Which declaration opened a wait, which is what decides the operations it applies to and
/// how its expiry is worded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeclaredBy {
    /// An `AWS::CloudFormation::WaitCondition`: a resource that exists to wait for signals,
    /// so it waits whichever operation deploys it.
    WaitCondition,
    /// A `CreationPolicy`'s `ResourceSignal` on a resource of another type: the attribute
    /// "is invoked during resource creation", so only a CREATE waits at it.
    CreationPolicy,
}

/// One resource-signal wait a deployment found in the template it is deploying.
///
/// Computed once, where the template is read, from the resource record the deployment
/// materialised for it and the declaration that record came from: the values here are the
/// declaration's own with every intrinsic function already evaluated, so nothing re-reads
/// the body later.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeclaredWaitCondition {
    /// The logical id the template declares the resource under, which is the
    /// `LogicalResourceId` a signal names.
    pub logical_id: String,
    /// `Count`: how many success signals release it.
    pub count: usize,
    /// `Timeout`: how long it waits for them, bounded by [`MAX_TIMEOUT`] and defaulted to
    /// [`DEFAULT_TIMEOUT`].
    pub timeout: Duration,
    /// The logical ids of the resources of the same template whose creation waits on this
    /// one: every resource that depends on it, directly or through another resource.
    /// CloudFormation creates a resource only once the resources it needs exist, so these
    /// are the ones the operation has not started while the barrier holds.
    pub blocks: BTreeSet<String>,
    /// Which declaration asked for the wait.
    pub declared_by: DeclaredBy,
}

impl DeclaredWaitCondition {
    /// Whether only a CREATE waits here, so an update that re-deploys the same resource is
    /// not held at it: a `CreationPolicy` governs the create alone, while a wait condition
    /// waits whichever operation deploys it.
    pub fn only_on_create(&self) -> bool {
        self.declared_by == DeclaredBy::CreationPolicy
    }
}

/// What one declaration asks of a wait, before the resources it blocks are known.
struct SignalPlan {
    count: usize,
    timeout: Duration,
    declared_by: DeclaredBy,
}

/// The wait `resource`'s declaration asks for, or `None` for one that asks for none.
///
/// `declaration` is the `Resources` entry the record was materialised from, which is where
/// the `CreationPolicy` resource attribute lives — a stored resource carries the evaluated
/// `Properties` alone. A `CreationPolicy`'s `ResourceSignal` is read in preference to the
/// properties, because that is the attribute AWS documents the signalling of a create on
/// ("we recommend that you use the `CreationPolicy` attribute"), and a wait condition that
/// carries one is a wait condition whose count and timeout the attribute states.
fn signal_plan(
    declaration: Option<&template::DeclaredResource>,
    resource: &StoredResource,
) -> Option<SignalPlan> {
    let wait_condition = resource.resource_type == WAIT_CONDITION_TYPE;
    let declared_by = if wait_condition {
        DeclaredBy::WaitCondition
    } else {
        DeclaredBy::CreationPolicy
    };
    if let Some(resource_signal) = declaration.and_then(creation_policy_resource_signal) {
        return Some(SignalPlan {
            count: signal_count(resource_signal.get("Count")),
            timeout: policy_timeout(resource_signal.get("Timeout")),
            declared_by,
        });
    }
    if !wait_condition {
        return None;
    }
    Some(SignalPlan {
        count: signal_count(resource.properties.get("Count")),
        timeout: declared_timeout(resource.properties.get("Timeout")),
        declared_by,
    })
}

/// The `ResourceSignal` member of a declaration's `CreationPolicy`, where it carries one.
fn creation_policy_resource_signal(
    declared: &template::DeclaredResource,
) -> Option<&::serde_json::Value> {
    declared
        .declaration
        .get("CreationPolicy")?
        .get("ResourceSignal")
}

/// `Count` as a declaration states it, or [`DEFAULT_SIGNAL_COUNT`].
///
/// A template may spell a numeric value as a number or as a string (both forms deploy), so
/// both are read; a value that is neither is no count at all and the default stands. Zero is
/// not a number of signals to wait for, so it is read as the default too.
fn signal_count(declared: Option<&::serde_json::Value>) -> usize {
    let declared = whole_number(declared).unwrap_or(DEFAULT_SIGNAL_COUNT as u64);
    let declared = usize::try_from(declared).unwrap_or(usize::MAX);
    declared.max(DEFAULT_SIGNAL_COUNT)
}

/// An `AWS::CloudFormation::WaitCondition`'s `Timeout` — "the number of seconds to wait" —
/// as the declaration states it, bounded by [`MAX_TIMEOUT`] and defaulted to
/// [`DEFAULT_TIMEOUT`].
fn declared_timeout(declared: Option<&::serde_json::Value>) -> Duration {
    whole_number(declared).map_or(DEFAULT_TIMEOUT, |seconds| {
        Duration::from_secs(seconds).min(MAX_TIMEOUT)
    })
}

/// A `CreationPolicy`'s `ResourceSignal.Timeout` as the declaration states it, bounded by
/// [`MAX_TIMEOUT`] and defaulted to [`DEFAULT_TIMEOUT`].
///
/// This one is "in ISO8601 duration format" — `PT10M` is ten minutes — so it is read through
/// the one ISO 8601 duration reading this workspace has rather than as a number of seconds:
/// a value the grammar does not admit states no wait and the default stands.
fn policy_timeout(declared: Option<&::serde_json::Value>) -> Duration {
    declared
        .and_then(::serde_json::Value::as_str)
        .and_then(::service_common::iso8601_span::iso8601_span)
        .map_or(DEFAULT_TIMEOUT, |timeout| timeout.min(MAX_TIMEOUT))
}

/// A whole number a template value states, spelled as a number or as a string.
fn whole_number(declared: Option<&::serde_json::Value>) -> Option<u64> {
    match declared? {
        ::serde_json::Value::Number(number) => number.as_u64(),
        ::serde_json::Value::String(text) => text.parse::<u64>().ok(),
        _ => None,
    }
}

/// Every resource-signal wait `resources` carries, with the resources each one blocks.
///
/// `template` is the body those records were materialised from, which is both where each
/// declaration's `CreationPolicy` lives and what says what depends on what: a resource that
/// names a waiting resource in a `DependsOn`, a `Ref` or an `Fn::GetAtt` — or that names
/// something which does — is one the operation cannot start until that wait is satisfied.
///
/// ONE pass over the template for the whole set ([`template::waits_on_each`]), charged to
/// the deployment's budget, however many waits the body declares: the relation is computed
/// once and inverted, rather than walked per wait.
pub fn declared(
    template: &template::Template,
    resources: &[StoredResource],
    budget: &mut template::Budget,
) -> Vec<DeclaredWaitCondition> {
    let declarations: BTreeMap<&str, &template::DeclaredResource> = template
        .resources
        .iter()
        .map(|declared| (declared.logical_id.as_str(), declared))
        .collect();
    let waiting: Vec<(&StoredResource, SignalPlan)> = resources
        .iter()
        .filter_map(|resource| {
            let declaration = declarations.get(resource.logical_id.as_str()).copied();
            signal_plan(declaration, resource).map(|plan| (resource, plan))
        })
        .collect();
    if waiting.is_empty() {
        return Vec::new();
    }
    let roots: BTreeSet<String> = waiting
        .iter()
        .map(|(resource, _)| resource.logical_id.clone())
        .collect();
    // The relation the other way round: per waiting resource, the resources that wait on it.
    let mut blocked: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for (logical_id, waits_on) in template::waits_on_each(template, &roots, budget) {
        for root in waits_on {
            blocked.entry(root).or_default().insert(logical_id.clone());
        }
    }
    waiting
        .into_iter()
        .map(|(resource, plan)| DeclaredWaitCondition {
            logical_id: resource.logical_id.clone(),
            count: plan.count,
            timeout: plan.timeout,
            blocks: blocked.remove(&resource.logical_id).unwrap_or_default(),
            declared_by: plan.declared_by,
        })
        .collect()
}

/// One resource-signal wait the operation in flight is waiting at: what the template
/// declared, and when the wait started.
///
/// The SIGNALS it waits for are not held here: `SignalResource` records every signal on
/// the stack itself ([`crate::state::StoredStack::record_signal`], keyed by the signalled
/// logical id and the signal's `UniqueId` and bounded there), so the one record of what
/// has arrived is that log, and whether this barrier is satisfied is a question about it
/// ([`crate::state::StoredStack::barrier_satisfied`]).
#[derive(Debug, Clone)]
pub struct WaitConditionBarrier {
    /// What the deployment computed for this wait.
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

    /// The logical id a signal names this wait by.
    pub fn logical_id(&self) -> &str {
        &self.declared.logical_id
    }

    /// How many success signals release it (`Count`).
    pub fn count(&self) -> usize {
        self.declared.count
    }

    /// Whether this resource's creation waits on this one.
    pub fn blocks(&self, logical_id: &str) -> bool {
        self.declared.blocks.contains(logical_id)
    }

    /// The instant the wait gives up.
    ///
    /// The wait is HELD for the span the declaration asked for, which is the span the
    /// resource it gates stays `*_IN_PROGRESS` — and reportable — for: nothing but a client's
    /// own `SignalResource` call ends it before then, and that is the point of it.
    /// "CloudFormation doesn't proceed with a stack creation or update until resources receive
    /// the required number of signals or the timeout period is exceeded" (`SignalResource`),
    /// so the span a client has to poll the gate in and signal it is the span its own
    /// declaration stated. That value is already held to the maximum its property publishes
    /// ([`MAX_TIMEOUT`]) and to the documented default where it states none
    /// ([`DEFAULT_TIMEOUT`]) where the declaration is read ([`declared`]), so the wait runs
    /// for exactly what the caller asked for and still has an instant it gives up at, after
    /// which the stack reaches a terminal status.
    ///
    /// Added without unwinding: `SystemTime`'s own addition panics past the clock's range,
    /// which no bounded span can reach from an instant the operation really started at, so
    /// an instant that could not be computed is read as a wait already over rather than as a
    /// failure of the read.
    pub fn deadline(&self) -> SystemTime {
        self.opened_at
            .checked_add(self.declared.timeout)
            .unwrap_or(self.opened_at)
    }

    /// Whether the wait has run out at `now` — which fails the operation for a wait that is
    /// not satisfied by then: "if the wait condition doesn't receive the specified number of
    /// success signals before the `Timeout` period expires, CloudFormation assumes that the
    /// wait condition has failed".
    pub fn has_expired(&self, now: SystemTime) -> bool {
        self.deadline() <= now
    }

    /// Why the operation failed, for a wait that ran out: as CloudFormation words it for the
    /// declaration that opened the wait — a wait condition reports what it was waiting for
    /// against what it had `received`, while a creation policy reports the signals it never
    /// got.
    pub fn timed_out_reason(&self, received: usize) -> String {
        match self.declared.declared_by {
            DeclaredBy::WaitCondition => format!(
                "WaitCondition timed out. Received {received} conditions when expecting {}",
                self.declared.count
            ),
            DeclaredBy::CreationPolicy => format!(
                "Failed to receive {} resource signal(s) within the specified duration",
                self.declared.count.saturating_sub(received)
            ),
        }
    }
}
