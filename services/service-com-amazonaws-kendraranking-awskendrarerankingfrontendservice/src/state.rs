// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-kendraranking-awskendrarerankingfrontendservice service.

use std::collections::{BTreeMap, HashMap};

use crate::ops::types::{CapacityUnitsConfiguration, Tag};

/// The `RescoreExecutionPlanStatus` value a plan that is ready to serve `Rescore`
/// carries ("When the value is `ACTIVE`, the rescore execution plan is ready for
/// use." — `RescoreExecutionPlanSummary$Status` in the model).
pub const STATUS_ACTIVE: &str = "ACTIVE";

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
    /// A `RescoreExecutionPlanStatus` value.
    pub status: String,
    /// Set only for a plan whose status is `FAILED`; absent otherwise.
    pub error_message: Option<String>,
    pub tags: Vec<Tag>,
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
