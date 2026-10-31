// SPDX-License-Identifier: CC0-1.0

//! The CREATE-time STABILISATION of one stack operation: the resources whose owning
//! service has answered the create but does not report the resource ready yet.
//!
//! ## The rule this module implements
//!
//! A create is not finished when the owning service's create call returns. Several AWS
//! APIs answer a create while the resource is still coming up — an ACM certificate is
//! `PENDING_VALIDATION` "upon being requested" and is not usable until it is issued — and
//! CloudFormation does not report such a resource `CREATE_COMPLETE` on the strength of the
//! accepted call: it holds the resource (and the stack operation behind it) at
//! `*_IN_PROGRESS` until that service reports the resource in the state it is ready in,
//! while already reporting the `PhysicalResourceId` that service assigned. A client that
//! reads a resource as created therefore reads a resource that can serve traffic, and a
//! stack is not reported complete while one of its resources cannot.
//!
//! What ENDS the wait is the owning service's own answer, read through that service's own
//! API ([`super::provisioner::Provisioner::readiness`]) — never a status this crate
//! guesses. The wait is bounded ([`CREATE_STABILISATION_WINDOW`]): a resource the owning
//! service has not brought up inside it fails the resource, and the stack rolls back,
//! exactly as a create the service refused outright does. So no template can leave a stack
//! in progress for ever.
//!
//! This module is the model of that wait, and nothing else. [`StabilisationBarrier`] is one
//! resource's: what the create left, the instant the wait began, and the instant the owning
//! service was first seen to report the resource ready.
//! [`super::StoredStack::settle`] will not complete an operation that still holds an
//! unreleased barrier, and what releases one is
//! [`super::provisioning::advance_stabilisations`].
//!
//! What it gates is what the STACK reports: the resources a deployment declares are created
//! in one pass ([`super::provisioning::reconcile`]), so a resource declared after one that
//! is still coming up is created in its own owning service either way — what no client is
//! told is that the operation, or the resource holding it, finished.
//!
//! ## Bounds
//!
//! One barrier per resource of the operation in flight whose create answered "not ready
//! yet", so the set is bounded by the resources a stack may hold
//! ([`super::MAX_CONCURRENT_RESOURCES`]), and every barrier is dropped when the operation
//! that opened it concludes.

use std::time::{Duration, SystemTime};

/// The longest CloudFormation holds ONE resource's creation open waiting for the service
/// that owns it to report the resource ready.
///
/// This emulator's ceiling on an open wait ([`super::MAX_HELD_WAIT`]), which is what every
/// wait of this service is bounded by — the signal wait a declaration asks for
/// ([`super::wait_conditions::MAX_HELD_TIMEOUT`]) included, so no open wait of one kind can
/// outlive a client's poll loop while another cannot. Long enough that every coming-up
/// span the services of this emulator owe a new resource falls due inside it many times
/// over — including the span of the stack operation the wait is measured from
/// ([`StabilisationBarrier::opened_at`]) — and short enough that a resource nothing will
/// ever bring up fails the stack inside a client's poll loop instead of outliving it.
pub const CREATE_STABILISATION_WINDOW: Duration = super::MAX_HELD_WAIT;

/// One resource of the operation in flight whose creation is still open: the owning service
/// accepted the create and does not report the resource ready yet.
#[derive(Debug, Clone)]
pub struct StabilisationBarrier {
    /// The logical id the template declares the resource under, which is what the stack's
    /// events and per-resource reads name it by.
    pub logical_id: String,
    /// `StackResource$ResourceType`, which is what says WHICH service's readiness answer
    /// ends this wait.
    pub resource_type: String,
    /// The identifier the owning service assigned, which the readiness read addresses the
    /// resource by — and which CloudFormation already reports for the resource while the
    /// wait holds.
    pub physical_resource_id: String,
    /// The state that service reported the resource in when the create answered, as its own
    /// API spells it. Carried so the failure a wait that runs out reports says what the
    /// resource was still in.
    pub reported_state: String,
    /// The instant the operation that created the resource started, which the wait runs
    /// from — and which the resource's own `*_IN_PROGRESS` event and reported timestamp
    /// carry, so one instant is the whole of what the stack says about this resource.
    pub opened_at: SystemTime,
    /// The instant the owning service was first seen to report the resource ready, or
    /// `None` while it does not.
    pub released_at: Option<SystemTime>,
}

impl StabilisationBarrier {
    /// Open the wait for a resource whose create `now` answered.
    pub fn open(
        logical_id: String,
        resource_type: String,
        physical_resource_id: String,
        reported_state: String,
        now: SystemTime,
    ) -> Self {
        Self {
            logical_id,
            resource_type,
            physical_resource_id,
            reported_state,
            opened_at: now,
            released_at: None,
        }
    }

    /// Whether the owning service has reported the resource ready, so the operation may
    /// proceed.
    pub fn released(&self) -> bool {
        self.released_at.is_some()
    }

    /// The instant the wait gives up.
    pub fn deadline(&self) -> SystemTime {
        self.opened_at + CREATE_STABILISATION_WINDOW
    }

    /// Whether the wait has run out at `now`, which fails the resource it is holding.
    pub fn has_expired(&self, now: SystemTime) -> bool {
        self.deadline() <= now
    }

    /// Why the resource failed, for a wait that ran out: which resource did not come up,
    /// and the state the service that owns it was still reporting.
    ///
    /// `state` is that service's latest answer, which is not always the one the create left
    /// ([`Self::reported_state`]).
    pub fn not_stabilised_reason(&self, state: &str) -> String {
        let state = if state.is_empty() {
            self.reported_state.as_str()
        } else {
            state
        };
        format!(
            "{} {} was still {state} when the wait for it to be created ran out",
            self.resource_type, self.physical_resource_id
        )
    }
}
