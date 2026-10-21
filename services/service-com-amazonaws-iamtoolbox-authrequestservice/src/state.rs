// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-iamtoolbox-authrequestservice service.
//!
//! The service models exactly one operation, the `@readonly`
//! `GetRequestAuthorizationDetails` read, so the store it reads from is the whole of
//! this service's state: one record per authorization id, written when a request was
//! denied and read back by the principal that was denied.
//!
//! There is no modeled write operation, so a record enters the store through
//! [`ComAmazonawsIamtoolboxAuthrequestserviceState::record_denial`] — the seam a host
//! embedding this emulator (or a test) uses to record the denial whose details the read
//! then serves. Nothing is seeded at construction: the store starts empty, so a GET for
//! an id this emulator never recorded answers `ResourceNotFoundException`, which is what
//! the real service answers for an id it never issued.

use std::collections::HashMap;
use std::time::{Duration, SystemTime};

use crate::ops::types::{Evaluation, PolicyInfo};

/// The context keys that apply to a whole request (or to one evaluation): the modeled
/// `AuthorizationContext` map, `map<string, document>`.
pub type AuthorizationContext = HashMap<String, ::aws_smithy_types::Document>;

/// How long a denial's authorization details stay readable.
///
/// The operation's own modeled documentation: "Authorization details are available for
/// at least 24 hours after the denial." 24 hours is therefore the floor the service
/// promises, and the window this emulator keeps a record for; a read of a record older
/// than this answers `ResourceNotFoundException`, the error the model documents for
/// details that "do not exist in this region or have expired".
pub const AUTHORIZATION_DETAILS_RETENTION: Duration = Duration::from_secs(24 * 60 * 60);

/// One denied request's authorization details, keyed in the store by its authorization
/// id (the id a supported service puts in the access-denied error message).
#[derive(Debug, Clone)]
pub struct AuthorizationDetailsRecord {
    /// The authorization id this record is keyed by.
    pub authorization_id: String,
    /// Context keys shared by every evaluation of the denied request.
    pub request_context: AuthorizationContext,
    /// The evaluations performed for the denied request, in the order they were
    /// performed; this is the list `GetRequestAuthorizationDetails` paginates.
    pub evaluations: Vec<Evaluation>,
    /// The policies that were evaluated. Request-level, so every page repeats it.
    pub policies: Vec<PolicyInfo>,
    /// When the denial happened. Drives the retention window above.
    pub created_at: SystemTime,
}

impl AuthorizationDetailsRecord {
    /// A record of a denial that happened now.
    pub fn new(
        authorization_id: impl Into<String>,
        request_context: AuthorizationContext,
        evaluations: Vec<Evaluation>,
        policies: Vec<PolicyInfo>,
    ) -> Self {
        Self {
            authorization_id: authorization_id.into(),
            request_context,
            evaluations,
            policies,
            created_at: SystemTime::now(),
        }
    }

    /// The same record, as if the denial had happened `age` ago. Lets a caller record a
    /// denial at a known age, which is how the retention window is exercised without
    /// waiting out a day.
    ///
    /// Test support: nothing serving a request calls this, and it is hidden from the
    /// crate's docs so the state API does not advertise a time-travel constructor.
    #[doc(hidden)]
    #[must_use]
    pub fn aged(mut self, age: Duration) -> Self {
        self.created_at = self
            .created_at
            .checked_sub(age)
            .unwrap_or(SystemTime::UNIX_EPOCH);
        self
    }

    /// Whether this record has fallen outside [`AUTHORIZATION_DETAILS_RETENTION`] by
    /// `now`.
    ///
    /// The comparison is strict (`>`), because the documented promise is "available for
    /// AT LEAST 24 hours after the denial": a record whose age is exactly the window is
    /// still inside the promise. A `created_at` in the future (a clock that moved
    /// backwards) is not expired.
    pub fn is_expired_at(&self, now: SystemTime) -> bool {
        now.duration_since(self.created_at)
            .is_ok_and(|age| age > AUTHORIZATION_DETAILS_RETENTION)
    }
}

/// Per-service state for com-amazonaws-iamtoolbox-authrequestservice. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsIamtoolboxAuthrequestserviceState {
    /// Authorization details by authorization id.
    pub authorization_details: HashMap<String, AuthorizationDetailsRecord>,
}

impl ComAmazonawsIamtoolboxAuthrequestserviceState {
    /// Record the authorization details of a denied request, returning the record the
    /// same authorization id held before, if any. This is the only way a record enters
    /// the store: the service models no write operation.
    ///
    /// Writing also drops every record the clock has already carried past
    /// [`AUTHORIZATION_DETAILS_RETENTION`], so a long-lived store stays bounded by the
    /// retention window rather than by the number of denials ever recorded. The sweep
    /// changes nothing a caller could still have read: an expired record is already
    /// invisible to [`live_record`](Self::live_record).
    pub fn record_denial(
        &mut self,
        record: AuthorizationDetailsRecord,
    ) -> Option<AuthorizationDetailsRecord> {
        let now = SystemTime::now();
        self.authorization_details
            .retain(|_, stored| !stored.is_expired_at(now));
        self.authorization_details
            .insert(record.authorization_id.clone(), record)
    }

    /// The record for `authorization_id` that is still within the retention window at
    /// `now`; `None` when no record was ever stored for that id or the stored one has
    /// expired. Both cases are the same answer on the wire — the details are not
    /// available — so the reader does not distinguish them.
    pub fn live_record(
        &self,
        authorization_id: &str,
        now: SystemTime,
    ) -> Option<&AuthorizationDetailsRecord> {
        self.authorization_details
            .get(authorization_id)
            .filter(|record| !record.is_expired_at(now))
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsIamtoolboxAuthrequestserviceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsIamtoolboxAuthrequestserviceState>)
}
