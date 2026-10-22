// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-kendraranking-awskendrarerankingfrontendservice service.

use std::collections::{BTreeMap, HashMap};

use smithy_server_runtime::{Violation, ViolationKind};

use crate::ops::types::{
    CapacityUnitsConfiguration, ResourceNotFoundException, Tag, ValidationException,
};

/// The `RescoreExecutionPlanStatus` value a plan that is ready to serve `Rescore`
/// carries ("When the value is `ACTIVE`, the rescore execution plan is ready for
/// use." — `RescoreExecutionPlanSummary$Status` in the model).
pub const STATUS_ACTIVE: &str = "ACTIVE";

/// The `RescoreExecutionPlanStatus` value a plan carries while the service is still
/// provisioning a freshly created plan (`CREATING` is one of the five values
/// `RescoreExecutionPlanStatus` models). Creating a rescore execution plan is
/// asynchronous: the plan is reported in this non-terminal status first and only then
/// becomes [`STATUS_ACTIVE`], which is what makes polling
/// `DescribeRescoreExecutionPlan` until `ACTIVE` the documented way to use one
/// (`DescribeRescoreExecutionPlanResponse$Status`: "When the value is ACTIVE, the
/// rescore execution plan is ready for use.").
pub const STATUS_CREATING: &str = "CREATING";

/// How this service states that a rescore execution plan id names no plan it holds.
///
/// A CONSTANT, request-independent sentence: the service names the resource class that
/// was not found and never echoes the identifier it was asked for, so the same text
/// answers an id that never existed and an id whose plan was deleted.
pub const PLAN_NOT_FOUND_MESSAGE: &str = "The specified Resource Execution Plan ID does not exist.";

/// How this service states that a plan exists but is not [`STATUS_ACTIVE`] yet, so it
/// cannot serve the data plane. Request-independent, like [`PLAN_NOT_FOUND_MESSAGE`].
pub const PLAN_NOT_ACTIVE_MESSAGE: &str =
    "Rescore Execution Plan status is invalid, must be in ACTIVE state";

/// The modelled `ResourceNotFoundException` (HTTP 404) for a plan id the store does not
/// hold — the ONE place that message is built, so every plan-lookup path states it
/// identically.
pub fn plan_not_found() -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(PLAN_NOT_FOUND_MESSAGE.to_string()),
    }
}

/// The modelled `ValidationException` (HTTP 400) a data-plane call against a plan that
/// is not [`STATUS_ACTIVE`] is refused with.
pub fn plan_not_active() -> ValidationException {
    ValidationException {
        message: Some(PLAN_NOT_ACTIVE_MESSAGE.to_string()),
    }
}

// ---------------------------------------------------------------------------
// The validation report this service answers a constraint violation with.
//
// Two rules, both this service's own rendering of the standard AWS report, and
// both applied to EVERY constraint class and every operation (the boundary
// validator reaches them through `service.rs`'s `override_refusal`):
//
// * a clause names WHERE the offending value was and WHAT it had to satisfy, and
//   never renders the value itself;
// * a `@pattern` requirement quotes the expression WITHOUT the `^` / `$` the model
//   spells it with, which is the expression the service advertises.
// ---------------------------------------------------------------------------

/// The error code every refusal this service rewords answers with: the same code the
/// modelled `ValidationException` (HTTP 400) already carries, so rewording the body
/// never moves a client off the code it branches on.
pub const VALIDATION_EXCEPTION: &str = "ValidationException";

/// The text [`Violation::message`] puts between the offending value's position and the
/// requirement it failed.
///
/// The LAST occurrence is the real separator: a requirement never contains this text,
/// while a caller-supplied value interpolated before it might.
const CONSTRAINT_SEPARATOR: &str = " failed to satisfy constraint: ";

/// A `@pattern` expression without the anchors a model may spell it with. A `$` escaped
/// by a backslash is a literal dollar sign inside the expression, not the end anchor, so
/// it stays.
fn unanchored(pattern: &str) -> &str {
    let body = pattern.strip_prefix('^').unwrap_or(pattern);
    match body.strip_suffix('$') {
        Some(trimmed) => {
            let backslashes = trimmed.len() - trimmed.trim_end_matches('\\').len();
            if backslashes % 2 == 0 {
                trimmed
            } else {
                body
            }
        }
        None => body,
    }
}

/// One clause of the validation report: WHERE the offending value was, and WHAT it had
/// to satisfy. The value itself is never part of a clause.
pub fn clause(member: &str, requirement: &str) -> String {
    format!("Value at '{member}' failed to satisfy constraint: {requirement}")
}

/// The standard AWS validation report around `clauses`: the violation count, then every
/// clause joined with `; `.
pub fn validation_report(clauses: &[String]) -> String {
    match clauses {
        [single] => format!("1 validation error detected: {single}"),
        many => format!(
            "{} validation errors detected: {}",
            many.len(),
            many.join("; ")
        ),
    }
}

/// The clause one [`Violation`] states under this service's rules.
///
/// The requirement is lifted out of the framework's OWN rendering of the violation
/// rather than re-spelled here, so every constraint class is worded exactly as the
/// boundary validator words it; only the `@pattern` expression is rewritten, and only to
/// drop its anchors. What the clause drops is the rendering of the value.
fn violation_clause(violation: &Violation) -> String {
    let advertised = match &violation.kind {
        ViolationKind::Pattern(pattern) => Violation {
            path: violation.path.clone(),
            kind: ViolationKind::Pattern(unanchored(pattern).to_string()),
            value: violation.value.clone(),
        },
        _ => violation.clone(),
    };
    let rendered = advertised.message();
    match rendered.rsplit_once(CONSTRAINT_SEPARATOR) {
        // A member that was ABSENT has no value to omit, and the framework's own clause
        // already says exactly that (`Value null at '<member>' …`), so it stands.
        Some((_, requirement)) if violation.value.is_some() => clause(&violation.path, requirement),
        _ => rendered,
    }
}

/// The report for every constraint violation the boundary validator found, in the order
/// it found them. This is what `service.rs`'s `override_refusal` answers with.
pub fn refusal_report(violations: &[Violation]) -> String {
    let clauses: Vec<String> = violations.iter().map(violation_clause).collect();
    validation_report(&clauses)
}

/// The report for one member that failed `kind`, for a handler stating a constraint the
/// model cannot express itself.
pub fn constraint_report(member: &str, kind: ViolationKind) -> String {
    validation_report(&[violation_clause(&Violation {
        path: member.to_string(),
        kind,
        // A PRESENT value selects the value-omitting clause rather than the `Value null`
        // one an absent member gets. What it holds is irrelevant: the value never
        // reaches the body.
        value: Some(String::new()),
    })])
}

/// The `RescoreExecutionPlanStatus` value a plan carries while the service
/// re-provisions it for an `UpdateRescoreExecutionPlan` call (`UPDATING` is one of the
/// five values `RescoreExecutionPlanStatus` models).
pub const STATUS_UPDATING: &str = "UPDATING";

/// How long a plan the service put into a transient status stays there before the
/// clock owes it its settled status — a freshly created plan's `CREATING` -> `ACTIVE`
/// and an updated plan's `UPDATING` -> `ACTIVE` alike.
///
/// The model publishes no duration for either transition and the service provisions the
/// plan after the call has returned, so this span is the emulator's own. It is never
/// zero: a caller acting straight after the call observes the transient status — and the
/// refusals it causes — exactly as it would on the real service. Nothing sleeps for it:
/// the due instant is stored and read back through
/// [`status_at`](RescoreExecutionPlan::status_at).
pub const TRANSIENT_STATUS_MILLIS: i64 = 500;

/// A status the clock owes a plan: the value the plan takes once `due_millis` has
/// passed, with no further call needed to make it due.
#[derive(Debug, Clone)]
pub struct PendingStatus {
    /// The `RescoreExecutionPlanStatus` value the plan settles into.
    pub status: String,
    /// The instant the settled status comes due, Unix epoch milliseconds.
    pub due_millis: i64,
}

impl PendingStatus {
    /// `settled`, due [`TRANSIENT_STATUS_MILLIS`] after `now_millis` — the one place the
    /// transient span is turned into a due instant.
    pub fn due_after_transient_span(settled: &str, now_millis: i64) -> Self {
        Self {
            status: settled.to_string(),
            due_millis: now_millis.saturating_add(TRANSIENT_STATUS_MILLIS),
        }
    }
}

/// The signing name / ARN namespace of this service, as the model's
/// `aws.api#service` trait states it (see `OPERATIONS.md`).
pub const ARN_NAMESPACE: &str = "kendra-ranking";

/// The region a request carries no SigV4 credential scope for. The emulator's
/// documented default region (see `emulator_arn` in `vendor/server/src/lib.rs`:
/// "`region` is caller-supplied (emulators default to `us-east-1`)").
pub const DEFAULT_REGION: &str = "us-east-1";

/// One rescore execution plan, exactly as `CreateRescoreExecutionPlan` persisted
/// it. `DescribeRescoreExecutionPlan` and `ListRescoreExecutionPlans` read their
/// whole answer from here.
#[derive(Debug, Clone)]
pub struct RescoreExecutionPlan {
    pub id: String,
    pub arn: String,
    pub name: String,
    pub description: Option<String>,
    pub capacity_units: Option<CapacityUnitsConfiguration>,
    /// Creation instant, Unix epoch milliseconds (the model types `CreatedAt` as a
    /// timestamp; milliseconds keep the stored value independent of the wire form).
    pub created_at_millis: i64,
    /// Last-update instant, Unix epoch milliseconds.
    pub updated_at_millis: i64,
    /// A `RescoreExecutionPlanStatus` value. Read it through
    /// [`status_at`](RescoreExecutionPlan::status_at), never directly: a transition the
    /// clock already owes is held in `pending_status` until some call materialises it.
    pub status: String,
    /// The status the clock owes this plan, while a transition is in flight; `None`
    /// once the plan has settled.
    pub pending_status: Option<PendingStatus>,
    /// Set only for a plan whose status is `FAILED`; absent otherwise.
    pub error_message: Option<String>,
    pub tags: Vec<Tag>,
}

impl RescoreExecutionPlan {
    /// This plan's status as of `now_millis`: the pending status once the clock owes it,
    /// otherwise the stored one.
    ///
    /// A reporting operation reads the status through this, so a transition that came
    /// due is visible without the read having to write anything.
    pub fn status_at(&self, now_millis: i64) -> &str {
        match &self.pending_status {
            Some(pending) if now_millis >= pending.due_millis => pending.status.as_str(),
            _ => self.status.as_str(),
        }
    }

    /// Materialise a transition the clock already owes, so a mutating call decides
    /// against the same status a reader sees rather than a stale one.
    pub fn settle_status(&mut self, now_millis: i64) {
        if let Some(pending) = self.pending_status.take() {
            if now_millis >= pending.due_millis {
                self.status = pending.status;
            } else {
                self.pending_status = Some(pending);
            }
        }
    }

    /// Put this plan into `transient` now, with `settled` the status the clock owes it
    /// [`TRANSIENT_STATUS_MILLIS`] later.
    pub fn begin_transition(&mut self, transient: &str, settled: &str, now_millis: i64) {
        self.status = transient.to_string();
        self.pending_status = Some(PendingStatus::due_after_transient_span(settled, now_millis));
    }
}

/// The number of rescore execution plans this emulator holds.
///
/// The service quotas these plans per account (the modelled
/// `ServiceQuotaExceededException`, HTTP 402, documents itself as "You have exceeded
/// the set limits for your Amazon Kendra Intelligent Ranking service. Please see
/// Quotas for more information"), so a create past the quota is refused with that
/// error rather than stored. The model does not carry the published number, so the
/// count here is the emulator's own; enforcing it is what keeps this store from
/// growing without bound in a long-lived emulator, since each plan's size is already
/// bounded by the modelled `@length` constraints on its members.
pub const MAX_RESCORE_EXECUTION_PLANS: usize = 100;

/// The number of documents one `Rescore` request may carry.
///
/// `Documents` is modelled with a minimum of 1 and no maximum, so the batch size — and
/// with it the scoring work, which is the batch size times the query's distinct term
/// count — is entirely caller-chosen. This ceiling is the emulator's own (the model
/// publishes none): a longer batch is refused with the modelled `ValidationException`
/// before any scoring runs.
pub const MAX_DOCUMENTS_PER_RESCORE: usize = 100;

/// Per-service state for com-amazonaws-kendraranking-awskendrarerankingfrontendservice. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsKendrarankingAwskendrarerankingfrontendserviceState {
    /// Every live plan, keyed by its 36-character id. A `BTreeMap` so
    /// `ListRescoreExecutionPlans` has one stable total order to page through.
    pub plans: BTreeMap<String, RescoreExecutionPlan>,
    /// Plan ARN -> plan id, so an ARN-addressed call (the tagging operations)
    /// resolves a `ResourceARN` without scanning every plan.
    pub arn_index: HashMap<String, String>,
    /// `ClientToken` -> plan id: backs `CreateRescoreExecutionPlan` idempotency. One
    /// entry per plan the token created, dropped with that plan, so this map is bounded
    /// by [`MAX_RESCORE_EXECUTION_PLANS`] like `plans` and `arn_index`.
    pub client_tokens: HashMap<String, String>,
    /// How many identifiers this store has minted; see [`next_id`](Self::next_id).
    pub ids_minted: u64,
}

impl ComAmazonawsKendrarankingAwskendrarerankingfrontendserviceState {
    /// The next identifier this service hands out, in the 36-character `8-4-4-4-12`
    /// hexadecimal shape `RescoreExecutionPlanId` models (`length 36..36`, pattern
    /// `^[a-zA-Z0-9][a-zA-Z0-9-]*$`), also within `RescoreId`'s `1..36`.
    ///
    /// It carries the minting instant and a per-store sequence number, so no two ids a
    /// store mints are ever equal; an id is an opaque handle, so nothing about a
    /// resource can be read out of it. Written by hand rather than taken from a UUID
    /// crate because adding a dependency means editing `Cargo.lock`, which every
    /// concurrently landing fix also rewrites — a change this branch cannot land.
    pub fn next_id(&mut self, now_millis: i64) -> String {
        self.ids_minted = self.ids_minted.wrapping_add(1);
        let instant = now_millis as u64;
        let sequence = self.ids_minted;
        format!(
            "{:08x}-{:04x}-{:04x}-{:04x}-{:012x}",
            ((instant >> 16) & 0xffff_ffff) as u32,
            (instant & 0xffff) as u16,
            ((sequence >> 32) & 0xffff) as u16,
            ((sequence >> 16) & 0xffff) as u16,
            ((sequence & 0xffff) << 32) | (instant & 0xffff_ffff),
        )
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsKendrarankingAwskendrarerankingfrontendserviceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsKendrarankingAwskendrarerankingfrontendserviceState>)
}

/// Now, as Unix epoch milliseconds.
pub fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX))
}

/// A stored epoch-millisecond instant as the modelled timestamp type.
pub fn timestamp(millis: i64) -> ::aws_smithy_types::DateTime {
    ::aws_smithy_types::DateTime::from_millis(millis)
}

/// The region this request is addressed to: the region field of its SigV4
/// credential scope, or [`DEFAULT_REGION`] when the request carries none.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map_or_else(|| DEFAULT_REGION.to_string(), |scope| scope.region)
}

/// The ARN of a rescore execution plan in `region`:
/// `arn:aws:kendra-ranking:<region>:<account>:rescore-execution-plan/<id>`.
pub fn plan_arn(region: &str, id: &str) -> String {
    ::engine::emulator_arn(
        ARN_NAMESPACE,
        region,
        &format!("rescore-execution-plan/{id}"),
    )
}
