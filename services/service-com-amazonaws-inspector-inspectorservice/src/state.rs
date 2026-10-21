// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-inspector-inspectorservice service.
//!
//! The store holds the Inspector-classic resource hierarchy — resource group ->
//! assessment target -> assessment template -> assessment run — keyed by ARN, plus the
//! counter the `0-xxxxxxxx` ARN suffixes are minted from. Records hold the MODELLED
//! shapes from [`crate::ops::types`] so a describe hands back exactly what the create
//! wrote, with the emulator's own bookkeeping (the instants the clock owes a run's state
//! changes at) kept beside them rather than inside them.

use std::collections::HashMap;
use std::time::{Duration, SystemTime};

use crate::ops::types::{
    AssessmentRun, AssessmentRunStateChange, AssessmentTarget, AssessmentTemplate, ResourceGroup,
};

/// The region every ARN this service mints carries when the request is unsigned and so
/// names no credential-scope region.
pub const DEFAULT_REGION: &str = "us-east-1";

/// This service's ARN namespace / endpoint prefix, as `smithy_query(query="service_info")`
/// reports it (`awsService.arnNamespace`).
pub const ARN_NAMESPACE: &str = "inspector";

/// `AssessmentRunState` values, as the model's `AssessmentRunState` enum spells them.
pub const RUN_STATE_CREATED: &str = "CREATED";
/// Data collection under way: the state a newly started run moves to once the clock owes
/// the transition.
pub const RUN_STATE_COLLECTING_DATA: &str = "COLLECTING_DATA";
/// Terminal state a run reaches once its collection window has elapsed.
pub const RUN_STATE_COMPLETED: &str = "COMPLETED";

/// The `AssessmentRunState` values from which a run makes no further progress; every
/// other value in the model's `AssessmentRunState` enum is a run still in progress.
pub const TERMINAL_RUN_STATES: &[&str] = &[
    "COMPLETED",
    "COMPLETED_WITH_ERRORS",
    "FAILED",
    "ERROR",
    "CANCELED",
];

/// How long after `StartAssessmentRun` the run owes its `CREATED` -> `COLLECTING_DATA`
/// transition. A CHOSEN span (the service documents no figure), inside the emulator's
/// 250-750 ms window, so a caller that describes the run immediately still observes
/// `CREATED`.
pub const DATA_COLLECTION_START_DELAY: Duration = Duration::from_millis(500);

/// `FailedItemErrorCode` for an ARN the store does not hold, as the model's
/// `FailedItemErrorCode` enum spells it.
pub const FAILED_ITEM_DOES_NOT_EXIST: &str = "ITEM_DOES_NOT_EXIST";

// ---------------------------------------------------------------------------------------
// Quotas. Every map in this store is bounded, so no sequence of create calls can grow the
// process without a limit the code itself enforces: each `create`/`start` handler compares
// the relevant count against the constant below BEFORE inserting and refuses with the
// modelled `LimitExceededException`, whose `LimitExceededErrorCode` enum declares one code
// per resource (`RESOURCE_GROUP_LIMIT_EXCEEDED`, `ASSESSMENT_TARGET_LIMIT_EXCEEDED`,
// `ASSESSMENT_TEMPLATE_LIMIT_EXCEEDED`, `ASSESSMENT_RUN_LIMIT_EXCEEDED`) — proof that the
// real service enforces a quota for each of them. Nothing here is per-request state: a
// record is only ever added by an explicit create, and removed by an explicit delete.
// ---------------------------------------------------------------------------------------

/// Assessment targets per account: "You can create up to 50 assessment targets per AWS
/// account", per `CreateAssessmentTarget`'s modelled documentation.
pub const MAX_ASSESSMENT_TARGETS: usize = 50;

/// Resource groups per account. The model declares `RESOURCE_GROUP_LIMIT_EXCEEDED`, so the
/// service caps them; AWS publishes no figure, so this cap is CHOSEN — at the same 50 the
/// target quota uses, since a resource group exists to be named by an assessment target.
/// Its own literal, so changing either quota does not move the other.
pub const MAX_RESOURCE_GROUPS: usize = 50;

/// Assessment templates per account (the Inspector-classic account quota). It also bounds
/// the templates under any ONE target, so no single target can grow the map without limit.
pub const MAX_ASSESSMENT_TEMPLATES: usize = 500;

/// Assessment runs per template. Bounds the runs one template can accumulate, so a caller
/// that keeps starting runs against the same template is refused rather than growing the map.
pub const MAX_ASSESSMENT_RUNS_PER_TEMPLATE: usize = 500;

/// Assessment runs per account: the product of the two caps above is far larger than the
/// store should ever hold, so the account cap is what bounds it (and so bounds every scan
/// over the run map).
pub const MAX_ASSESSMENT_RUNS: usize = 5_000;

/// A stored assessment run: the modelled snapshot a describe returns, plus the instants
/// at which the clock owes its state changes.
///
/// The due instants are stored, never slept on: whichever call next reads the run
/// materialises every transition the clock has already made due.
#[derive(Debug, Clone)]
pub struct AssessmentRunRecord {
    /// The run exactly as `DescribeAssessmentRuns` reports it.
    pub run: AssessmentRun,
    /// When the `CREATED` -> `COLLECTING_DATA` transition falls due.
    pub collecting_data_due_at: SystemTime,
    /// When the collection window closes and the run completes: `startedAt` plus the
    /// template's `durationInSeconds` ("The duration of the assessment run", per the
    /// model's `AssessmentRun$durationInSeconds` documentation).
    pub completion_due_at: SystemTime,
}

impl AssessmentRunRecord {
    /// Record a state change at `at`, the way the service's own `stateChanges` list does.
    fn transition(&mut self, state: &str, at: SystemTime) {
        self.run.state = state.to_string();
        self.run.state_changed_at = date_time(at);
        self.run.state_changes.push(AssessmentRunStateChange {
            state_changed_at: date_time(at),
            state: state.to_string(),
        });
    }

    /// Materialise every transition the clock owed this run by `now`.
    ///
    /// Each change is stamped with the instant it fell DUE, not with `now`, so the
    /// recorded history does not depend on when somebody happened to look.
    pub fn advance(&mut self, now: SystemTime) {
        if self.run.state == RUN_STATE_CREATED && now >= self.collecting_data_due_at {
            let due = self.collecting_data_due_at;
            self.transition(RUN_STATE_COLLECTING_DATA, due);
        }
        if self.run.state == RUN_STATE_COLLECTING_DATA && now >= self.completion_due_at {
            // No Inspector agent exists in the emulator, so the window closes having
            // collected from zero agents: the run completes, and reports no findings.
            let due = self.completion_due_at;
            self.run.data_collected = true;
            self.run.completed_at = Some(date_time(due));
            self.transition(RUN_STATE_COMPLETED, due);
        }
    }

    /// Whether this run is still in progress, i.e. has not reached a terminal state.
    pub fn in_progress(&self) -> bool {
        !TERMINAL_RUN_STATES.contains(&self.run.state.as_str())
    }
}

/// Per-service state for com-amazonaws-inspector-inspectorservice. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsInspectorInspectorserviceState {
    /// Resource groups by ARN (`…:resourcegroup/0-xxxxxxxx`).
    pub resource_groups: HashMap<String, ResourceGroup>,
    /// Assessment targets by ARN (`…:target/0-xxxxxxxx`).
    pub assessment_targets: HashMap<String, AssessmentTarget>,
    /// Assessment templates by ARN (`<target arn>/template/0-xxxxxxxx`).
    pub assessment_templates: HashMap<String, AssessmentTemplate>,
    /// Assessment runs by ARN (`<template arn>/run/0-xxxxxxxx`).
    pub assessment_runs: HashMap<String, AssessmentRunRecord>,
    /// Counter the `0-xxxxxxxx` id suffixes are minted from, so ids are unique and
    /// deterministic per store.
    pub next_id: u64,
}

impl ComAmazonawsInspectorInspectorserviceState {
    /// Mint the next `0-xxxxxxxx` resource id: the `0-` generation prefix and eight
    /// characters of Inspector's id alphabet, as every Inspector-classic ARN carries.
    ///
    /// The counter is reduced modulo 62^8 and multiplied by a factor coprime with
    /// 62 = 2·31 in 128-bit arithmetic, which is a bijection on that range: so ids are
    /// spread across the alphabet the way the service's own are while staying DISTINCT
    /// (for the first 62^8 mints, far past the quotas above) and deterministic per store.
    pub fn mint_id(&mut self) -> String {
        /// Inspector id characters: digits and both cases, as its published ARNs use
        /// (e.g. the `rulespackage/0-gEjTy7T7` ARNs AWS documents).
        const ALPHABET: &[u8] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
        /// 62^8 — the number of distinct eight-character ids.
        const SPAN: u128 = 218_340_105_584_896;
        /// Odd and not a multiple of 31, hence coprime with 62^8.
        const SPREAD: u128 = 2_654_435_761;

        self.next_id = self.next_id.wrapping_add(1);
        let spread = (u128::from(self.next_id) % SPAN) * SPREAD % SPAN;
        let mut value = u64::try_from(spread).unwrap_or_default();
        let base = ALPHABET.len() as u64;
        let mut id = String::with_capacity(10);
        id.push_str("0-");
        for _ in 0..8 {
            let digit = usize::try_from(value % base).unwrap_or_default();
            value /= base;
            id.push(char::from(ALPHABET[digit]));
        }
        id
    }

    /// Materialise the transitions the clock owes the runs `arns` names, by `now`.
    ///
    /// Called by the operations that report run state: the change was owed at `now` whether
    /// or not anybody called, so applying it on read is not a mutation the caller caused.
    /// Only the named runs are touched — at most the requested batch, never the whole map.
    pub fn advance_runs(&mut self, arns: &[String], now: SystemTime) {
        for arn in arns {
            if let Some(record) = self.assessment_runs.get_mut(arn) {
                record.advance(now);
            }
        }
    }

    /// Materialise the transitions the clock owes every run of `template_arns`, by `now`.
    ///
    /// One pass over the run map, which [`MAX_ASSESSMENT_RUNS`] bounds, with a set lookup
    /// per run — not a scan per template.
    pub fn advance_runs_of_templates(&mut self, template_arns: &[String], now: SystemTime) {
        let wanted: ::std::collections::HashSet<&str> =
            template_arns.iter().map(String::as_str).collect();
        for record in self.assessment_runs.values_mut() {
            if wanted.contains(record.run.assessment_template_arn.as_str()) {
                record.advance(now);
            }
        }
    }

    /// How many runs `template_arn` has accumulated. Bounded by [`MAX_ASSESSMENT_RUNS`].
    pub fn run_count_of_template(&self, template_arn: &str) -> usize {
        self.assessment_runs
            .values()
            .filter(|record| record.run.assessment_template_arn == template_arn)
            .count()
    }

    /// The ARNs of the runs of `template_arns` that are still in progress, in ARN order
    /// so the answer does not depend on map iteration order.
    ///
    /// One pass over the run map, which [`MAX_ASSESSMENT_RUNS`] bounds, with a set lookup
    /// per run.
    pub fn runs_in_progress(&self, template_arns: &[String]) -> Vec<String> {
        let wanted: ::std::collections::HashSet<&str> =
            template_arns.iter().map(String::as_str).collect();
        let mut arns: Vec<String> = self
            .assessment_runs
            .values()
            .filter(|record| {
                record.in_progress()
                    && wanted.contains(record.run.assessment_template_arn.as_str())
            })
            .map(|record| record.run.arn.clone())
            .collect();
        arns.sort();
        arns
    }

    /// The ARNs of the templates belonging to `target_arn`, in ARN order.
    pub fn templates_of_target(&self, target_arn: &str) -> Vec<String> {
        let mut arns: Vec<String> = self
            .assessment_templates
            .values()
            .filter(|template| template.assessment_target_arn == target_arn)
            .map(|template| template.arn.clone())
            .collect();
        arns.sort();
        arns
    }
}

/// The modelled timestamp for a stored instant.
///
/// Never panics on a pre-epoch instant: such a clock yields the epoch rather than
/// taking the whole request down.
pub fn date_time(at: SystemTime) -> ::aws_smithy_types::DateTime {
    let secs = at
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64();
    ::aws_smithy_types::DateTime::from_secs_f64(secs)
}

/// The region the caller signed for, which every ARN this service mints carries.
///
/// Reads the SigV4 credential scope of the request, falling back to [`DEFAULT_REGION`]
/// for an unsigned request that names no region. The scope parser itself is shared
/// (`emulator_server::sigv4_credential_scope`); only the default lives here, because the
/// crate that owns the parser is vendored and this workspace's root `Cargo.toml` cannot be
/// edited to add a shared service-side crate for it.
pub fn request_region(ctx: &smithy_server_runtime::OperationContext<'_>) -> String {
    ::emulator_server::sigv4_credential_scope(&ctx.parts().headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// An Inspector ARN for `resource` in `region`, carrying the emulator's canonical
/// account id.
pub fn inspector_arn(region: &str, resource: &str) -> String {
    ::engine::emulator_arn(ARN_NAMESPACE, region, resource)
}

/// `s` shortened to at most `max` characters, respecting character boundaries.
pub fn truncate_chars(s: &str, max: usize) -> String {
    s.chars().take(max).collect()
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsInspectorInspectorserviceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsInspectorInspectorserviceState>)
}
