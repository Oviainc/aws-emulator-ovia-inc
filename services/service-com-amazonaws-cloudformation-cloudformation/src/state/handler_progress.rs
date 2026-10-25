// SPDX-License-Identifier: CC0-1.0

//! The progress a resource handler reports for ONE handler invocation, which
//! `RecordHandlerProgress` records.
//!
//! ## The rule this module implements
//!
//! `RecordHandlerProgress` "reports progress of a resource handler to CloudFormation". A
//! report is about one handler INVOCATION, and the only thing that names that invocation is
//! the `BearerToken` CloudFormation issued when it invoked the handler — the report carries
//! no stack, no resource and no operation id. So a report is judged against the status the
//! named invocation currently holds ([`StoredHandlerInvocation::operation_status`]):
//!
//! * the `CurrentOperationStatus` the request asserts has to BE that status — the
//!   conditional check the modelled `OperationStatusCheckFailedException` is declared for,
//!   whose `awsQueryError` code is `ConditionalCheckFailed`;
//! * the `OperationStatus` it reports has to be that status again — `RecordHandlerProgress`
//!   is `@idempotent`, so a report sent twice is one report — or a status that comes AFTER
//!   it in `PENDING` → `IN_PROGRESS` → `SUCCESS`/`FAILED` ([`follows`]), the transition the
//!   modelled `InvalidStateTransitionException` is declared for. A terminal status is the
//!   invocation's last: there is no progress after the handler reported how it finished.
//!
//! Both of those modelled errors are documented as "reserved for use by the CloudFormation
//! CLI. CloudFormation doesn't return this error to users", so neither is what a caller
//! sees: a refused report answers HTTP 400 with this service's own wire code
//! `ValidationError` ([`crate::scaffolded::Scaffolded::validation_error`]), which is what
//! the refusals worded here ([`unknown_bearer_token`],
//! [`operation_status_check_failed`], [`invalid_state_transition`]) are carried by.
//!
//! A refused report says which member it is about and quotes the value the request carried
//! for it, as the service's own refusals do — a token that names no invocation is
//! `BearerToken '<the token>' is invalid`, the wording the real service answers: "invalid"
//! rather than "unknown", because to a caller there is no difference between a token this
//! account never had and one it may not report under. Only the token the request itself sent
//! is quoted back, to the caller that sent it.
//!
//! ## What writes it
//!
//! CloudFormation issues a bearer token when it INVOKES a resource type's handler, and the
//! handler package is the extension's own code. Nothing in this emulator invokes one: a
//! stack's resources are created by calling the owning service's own API
//! ([`crate::state::provisioning`]), never by running an extension's handler package, so no
//! bearer token is issued here and a report arriving under one names no invocation — which
//! is the refusal [`unknown_bearer_token`] words. An account whose handlers have never been
//! invoked is exactly what this store holds.
//!
//! [`crate::state::ComAmazonawsCloudformationCloudformationState::record_handler_invocation`]
//! is the one way a record enters the store, so the invocation path this emulator grows
//! later — and the suite that covers reporting over records no wire call can create — write
//! through it rather than reaching into the vector.
//!
//! ## What a report does NOT leave behind
//!
//! Beyond the status, a report carries `StatusMessage`, `ErrorCode`, `ResourceModel` and
//! `ClientRequestToken`, all of them "reserved for use by the CloudFormation CLI". None is
//! persisted: `RecordHandlerProgressOutput` has no members and no modelled read reports any
//! of them back, so keeping them would be storing what nothing can answer.

/// The model's `OperationStatus` enum — the statuses one handler invocation moves through
/// (`smithy_query(query="shape", shape="OperationStatus")`).
pub mod operation_status {
    pub const PENDING: &str = "PENDING";
    pub const IN_PROGRESS: &str = "IN_PROGRESS";
    pub const SUCCESS: &str = "SUCCESS";
    pub const FAILED: &str = "FAILED";
}

/// How many handler invocations this store keeps a progress record for, oldest reclaimed
/// first.
///
/// Every growth site in this service's store is bounded. The model documents no retention
/// for a handler invocation, so this is a COUNT rather than an age: a bound on the store,
/// not a claim about how long the real service keeps an invocation addressable.
pub const MAX_HANDLER_INVOCATIONS: usize = 500;

/// Where a status sits in the order one invocation's statuses come in: `PENDING` before
/// `IN_PROGRESS` before either way of finishing, with `SUCCESS` and `FAILED` sharing the
/// last rank because they are alternatives rather than successive.
///
/// `None` for anything the modelled enum does not admit, which [`follows`] reads as a
/// transition that is not one of this invocation's.
fn rank(status: &str) -> Option<u8> {
    match status {
        operation_status::PENDING => Some(0),
        operation_status::IN_PROGRESS => Some(1),
        operation_status::SUCCESS | operation_status::FAILED => Some(2),
        _ => None,
    }
}

/// The rank a terminal status has: the handler has reported how the invocation finished.
const TERMINAL_RANK: u8 = 2;

/// Whether `status` is one a handler reports as its LAST: `SUCCESS` or `FAILED`.
pub fn is_terminal(status: &str) -> bool {
    rank(status) == Some(TERMINAL_RANK)
}

/// Whether an invocation holding `recorded` may be reported as `reported`.
///
/// The same status again is always a legal report — `RecordHandlerProgress` is
/// `@idempotent`, so a retried report is the same report and changes nothing — and any
/// other report has to move FORWARD in the order [`rank`] gives, which is what leaves a
/// terminal status with nothing legal after it.
pub fn follows(recorded: &str, reported: &str) -> bool {
    if recorded == reported {
        return true;
    }
    match (rank(recorded), rank(reported)) {
        (Some(from), Some(to)) => from < to,
        _ => false,
    }
}

/// The refusal of a report whose `BearerToken` names no handler invocation of this account
/// and Region: the token the request carried is not one a report can be made under.
pub fn unknown_bearer_token(bearer_token: &str) -> String {
    format!("BearerToken '{bearer_token}' is invalid")
}

/// The refusal of a report whose `CurrentOperationStatus` is not the status the named
/// invocation holds: the conditional check on the write failed, so nothing was recorded.
pub fn operation_status_check_failed(recorded: &str, asserted: &str) -> String {
    format!("The operation status of the specified BearerToken is {recorded}, not {asserted}")
}

/// The refusal of a report whose `OperationStatus` does not [`follows`] the status the
/// named invocation holds.
pub fn invalid_state_transition(recorded: &str, reported: &str) -> String {
    format!("An operation status of {recorded} can't transition to {reported}")
}

/// ONE handler invocation CloudFormation issued a bearer token for, and the status the
/// handler has reported for it so far.
#[derive(Debug, Clone)]
pub struct StoredHandlerInvocation {
    /// The Region this invocation belongs to. A report is judged for the Region its request
    /// was signed for, the way every other read and write of this service is.
    pub region: String,
    /// The `BearerToken` CloudFormation issued for this invocation (`ClientToken`,
    /// `@length(1..128)`), which is the only thing a progress report names it by.
    pub bearer_token: String,
    /// The `OperationStatus` this invocation holds right now: what a report's
    /// `CurrentOperationStatus` is checked against, what a reported status has to
    /// [`follows`], and what an accepted report replaces.
    pub operation_status: String,
}

impl StoredHandlerInvocation {
    /// An invocation of `region`, addressable by `bearer_token`, holding
    /// `operation_status`.
    pub fn new(
        region: impl Into<String>,
        bearer_token: impl Into<String>,
        operation_status: impl Into<String>,
    ) -> Self {
        Self {
            region: region.into(),
            bearer_token: bearer_token.into(),
            operation_status: operation_status.into(),
        }
    }

    /// Whether this is the invocation a report signed for `region` and carrying
    /// `bearer_token` names. A token issued for another Region names nothing here, so no
    /// report can reach across Regions by accident.
    pub fn is_addressed_by(&self, region: &str, bearer_token: &str) -> bool {
        self.region == region && self.bearer_token == bearer_token
    }
}
