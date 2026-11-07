// SPDX-License-Identifier: CC0-1.0

//! The A/B testing plane's state: the A/B tests this service holds.
//!
//! An A/B test is MINTED by `CreateABTest` — the call that creates it chooses the
//! variants, the gateway whose traffic is split between them and the evaluation
//! configuration that measures them, and this emulator mints the `abTestId` and the ARN
//! it is addressed by ([`mint_ab_test_id`], [`ab_test_arn`]). `ListABTests` reports the
//! tests held here and nothing else, so a test that was never created is never listed.
//!
//! A test's `status` is read off the CLOCK rather than stored: `CreateABTest` answers
//! 202 Accepted, so a freshly created test is still `CREATING` and becomes `ACTIVE`
//! once [`AB_TEST_PROVISIONING_SPAN`] has passed, with no call needed to make it so
//! ([`ABTestRecord::status_as_of`]). Its `executionStatus` is stored, because what
//! starts and stops traffic splitting is a call, not the clock.
//!
//! The plane has an explicit ceiling: `CreateABTest` is its growth site and a caller
//! chooses both the names it creates under and the `clientToken` each insert is recorded
//! with, so it is bounded by [`MAX_AB_TESTS`] and
//! [`MAX_AB_TEST_IDEMPOTENCY_RECORDS`], each evicting its least recently written record
//! once that maximum is passed.

use std::collections::HashMap;
use std::time::{Duration, SystemTime};

use crate::ops::types::{ABTestEvaluationConfig, ABTestSummary, GatewayFilter, Variant};

use super::{ComAmazonawsBedrockagentcoreAmazonbedrockagentcoreState, ARN_SERVICE};

/// The most A/B tests this emulator retains; the least recently created one is evicted
/// once the store passes it.
///
/// An EMULATOR ceiling, not an AWS quota: every `CreateABTest` appends a record, so
/// without one a caller looping on the create would grow the process heap without end.
pub const MAX_AB_TESTS: usize = 256;

/// The most `CreateABTest` `clientToken` records this emulator retains.
pub const MAX_AB_TEST_IDEMPOTENCY_RECORDS: usize = 1_000;

/// How long a created A/B test stays `CREATING` before it is `ACTIVE`.
///
/// `CreateABTest` answers 202 Accepted — the test is accepted for creation rather than
/// ready — and AWS publishes no duration for how long that provisioning takes, so this
/// span is the EMULATOR's own: long enough that a caller which creates a test and reads
/// it back at once still observes the transient status, short enough not to hold a test
/// suite up.
pub const AB_TEST_PROVISIONING_SPAN: Duration = Duration::from_secs(3);

/// `ABTestStatus` for a test whose creation is still being provisioned (source:
/// smithy_query query="shape" shape="ABTestStatus", values CREATING | ACTIVE |
/// CREATE_FAILED | UPDATING | UPDATE_FAILED | DELETING | DELETE_FAILED | FAILED).
pub const AB_TEST_STATUS_CREATING: &str = "CREATING";

/// `ABTestStatus` for a provisioned test, from the same enum.
pub const AB_TEST_STATUS_ACTIVE: &str = "ACTIVE";

/// `ABTestExecutionStatus` for a test whose traffic splitting has never run (source:
/// smithy_query query="shape" shape="ABTestExecutionStatus", values PAUSED | RUNNING |
/// STOPPED | NOT_STARTED).
pub const AB_TEST_EXECUTION_NOT_STARTED: &str = "NOT_STARTED";

/// `ABTestExecutionStatus` for a test that is splitting traffic, from the same enum.
pub const AB_TEST_EXECUTION_RUNNING: &str = "RUNNING";

/// The weight every variant's share of the traffic is a percentage of: `Variant$weight`
/// is documented as "the percentage of traffic to route to this variant", and "weights
/// across all variants must sum to 100" (source: smithy_query query="shape"
/// shape="Variant", member="weight").
pub const TOTAL_VARIANT_WEIGHT: i64 = 100;

/// The control variant's name, and the treatment variant's: `VariantName` is pattern
/// `^(C|T1)$`, documented as "must be `C` for control or `T1` for treatment" (source:
/// smithy_query query="shape" shape="Variant", member="name").
pub const CONTROL_VARIANT: &str = "C";
/// See [`CONTROL_VARIANT`].
pub const TREATMENT_VARIANT: &str = "T1";

/// One A/B test: what `CreateABTest` persisted, which is what every later read of the
/// test reports.
#[derive(Debug, Clone)]
pub struct ABTestRecord {
    /// Minted by [`mint_ab_test_id`]; never client-supplied.
    pub ab_test_id: String,
    /// Minted by [`ab_test_arn`] for the region the creating request was signed for.
    pub ab_test_arn: String,
    pub name: String,
    pub description: Option<String>,
    pub gateway_arn: String,
    pub variants: Vec<Variant>,
    pub gateway_filter: Option<GatewayFilter>,
    pub evaluation_config: ABTestEvaluationConfig,
    pub role_arn: String,
    pub tags: Option<HashMap<String, String>>,
    pub created_at: SystemTime,
    /// When this test was last changed by a call: its creation instant until one does.
    pub updated_at: SystemTime,
    /// When traffic splitting last started, or `None` while it never has — the creating
    /// request's `enableOnCreate` is documented as "if true, traffic splitting begins
    /// automatically", so a test created enabled starts at its creation instant.
    pub started_at: Option<SystemTime>,
    /// This test's `ABTestExecutionStatus`. Stored rather than read off the clock:
    /// traffic splitting starts and stops because a call says so.
    pub execution_status: String,
    /// The value [`ComAmazonawsBedrockagentcoreAmazonbedrockagentcoreState::next_seq`]
    /// handed this test: its eviction order, and what keeps two tests created in the
    /// same millisecond apart.
    pub seq: u64,
}

impl ABTestRecord {
    /// A test as the call that creates it leaves it: the configuration that call asked
    /// for, created at `created_at`, splitting traffic from that instant when the call
    /// asked for it to be enabled.
    #[allow(clippy::too_many_arguments)]
    pub fn created(
        seq: u64,
        region: &str,
        name: String,
        description: Option<String>,
        gateway_arn: String,
        variants: Vec<Variant>,
        gateway_filter: Option<GatewayFilter>,
        evaluation_config: ABTestEvaluationConfig,
        role_arn: String,
        tags: Option<HashMap<String, String>>,
        enable_on_create: bool,
        created_at: SystemTime,
    ) -> Self {
        let ab_test_id = mint_ab_test_id(&name, created_at, seq);
        Self {
            ab_test_arn: ab_test_arn(region, &ab_test_id),
            ab_test_id,
            name,
            description,
            gateway_arn,
            variants,
            gateway_filter,
            evaluation_config,
            role_arn,
            tags,
            created_at,
            // Nothing has changed this test yet, so the last instant it changed is the
            // instant it came into being.
            updated_at: created_at,
            started_at: enable_on_create.then_some(created_at),
            execution_status: if enable_on_create {
                AB_TEST_EXECUTION_RUNNING.to_string()
            } else {
                AB_TEST_EXECUTION_NOT_STARTED.to_string()
            },
            seq,
        }
    }

    /// The instant this test's creation is provisioned, or `None` when that instant is
    /// not representable (so nothing can be past it).
    pub fn active_at(&self) -> Option<SystemTime> {
        self.created_at.checked_add(AB_TEST_PROVISIONING_SPAN)
    }

    /// This test's `ABTestStatus` at `now`: `CREATING` until its creation is
    /// provisioned, `ACTIVE` from then on. Read off stored state and the clock, so a
    /// report of the test states the transition that was already due without writing
    /// anything.
    pub fn status_as_of(&self, now: SystemTime) -> &'static str {
        match self.active_at() {
            Some(active_at) if now >= active_at => AB_TEST_STATUS_ACTIVE,
            _ => AB_TEST_STATUS_CREATING,
        }
    }

    /// This test as the modelled `ABTestSummary`, as of `now`.
    pub fn summary(&self, now: SystemTime) -> ABTestSummary {
        ABTestSummary {
            ab_test_id: self.ab_test_id.clone(),
            ab_test_arn: self.ab_test_arn.clone(),
            name: self.name.clone(),
            status: self.status_as_of(now).to_string(),
            execution_status: self.execution_status.clone(),
            description: self.description.clone(),
            gateway_arn: Some(self.gateway_arn.clone()),
            created_at: ::aws_smithy_types::DateTime::from(self.created_at),
            updated_at: ::aws_smithy_types::DateTime::from(self.updated_at),
        }
    }

    /// The total order A/B tests are reported in: newest first, with the creation
    /// sequence breaking a tie between two tests created in the same millisecond, so one
    /// state always paginates the same way.
    pub fn order_key(&self) -> (SystemTime, u64) {
        (self.created_at, self.seq)
    }
}

/// Mint an `abTestId` for a test of `name` created at `created_at`.
///
/// `ABTestId`'s pattern is `^[a-zA-Z][a-zA-Z0-9-_]{0,99}-[a-zA-Z0-9]{10}$`: the name the
/// request asked for (itself `^[a-zA-Z][a-zA-Z0-9_]{0,47}$`, so every character of it is
/// one the id admits), then a ten-character discriminator — the low bits of the creation
/// instant in epoch milliseconds and of the engine-wide sequence, five lowercase hex
/// digits each, which keeps two tests created in one millisecond apart.
pub fn mint_ab_test_id(name: &str, created_at: SystemTime, seq: u64) -> String {
    let millis = created_at
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |since_epoch| since_epoch.as_millis());
    format!("{name}-{:05x}{:05x}", millis & 0xf_ffff, seq & 0xf_ffff)
}

/// The ARN an A/B test is addressed by in `region`: `ABTestArn`'s `ab-test/<abTestId>`
/// resource form under this emulator's account.
pub fn ab_test_arn(region: &str, ab_test_id: &str) -> String {
    ::engine::emulator_arn(ARN_SERVICE, region, &format!("ab-test/{ab_test_id}"))
}

/// The traffic share `variants` add up to, which the service requires to be
/// [`TOTAL_VARIANT_WEIGHT`].
pub fn total_variant_weight(variants: &[Variant]) -> i64 {
    variants
        .iter()
        .map(|variant| i64::from(variant.weight))
        .sum()
}

/// Whether `variants` name one control and one treatment, which is what an A/B test
/// splits traffic between ([`CONTROL_VARIANT`], [`TREATMENT_VARIANT`]).
pub fn names_one_control_and_one_treatment(variants: &[Variant]) -> bool {
    let named = |wanted: &str| {
        variants
            .iter()
            .filter(|variant| variant.name == wanted)
            .count()
            == 1
    };
    named(CONTROL_VARIANT) && named(TREATMENT_VARIANT)
}

impl ComAmazonawsBedrockagentcoreAmazonbedrockagentcoreState {
    /// The A/B test this `CreateABTest` `clientToken` already created, or `None` when it
    /// has created none this store still holds.
    pub fn ab_test_for_token(&self, client_token: &str) -> Option<&ABTestRecord> {
        self.ab_test_idempotency
            .get(client_token)
            .and_then(|ab_test_id| self.ab_tests.get(ab_test_id))
    }

    /// Store a created A/B test, recording `client_token` as the token that created it,
    /// and hold every bound this plane declares.
    pub fn insert_ab_test(&mut self, record: ABTestRecord, client_token: Option<String>) {
        if let Some(token) = client_token {
            self.ab_test_idempotency
                .insert(token, record.ab_test_id.clone());
        }
        self.ab_tests.insert(record.ab_test_id.clone(), record);

        while self.ab_tests.len() > MAX_AB_TESTS {
            // The least recently created test gives way first; the tests a live
            // workflow just created are the ones that stay listable.
            let Some(oldest) = self
                .ab_tests
                .values()
                .min_by_key(|held| held.seq)
                .map(|held| held.ab_test_id.clone())
            else {
                break;
            };
            self.ab_tests.remove(&oldest);
        }
        while self.ab_test_idempotency.len() > MAX_AB_TEST_IDEMPOTENCY_RECORDS {
            // A token whose test is already gone is of no use to a replay, so it gives
            // way before one that can still be answered.
            let Some(oldest) = self
                .ab_test_idempotency
                .iter()
                .min_by_key(|(token, ab_test_id)| {
                    (
                        self.ab_tests.get(*ab_test_id).map_or(0, |held| held.seq),
                        (*token).clone(),
                    )
                })
                .map(|(token, _)| token.clone())
            else {
                break;
            };
            self.ab_test_idempotency.remove(&oldest);
        }
    }

    /// Every A/B test this store holds, newest first ([`ABTestRecord::order_key`]).
    pub fn ab_tests_newest_first(&self) -> Vec<&ABTestRecord> {
        let mut tests: Vec<&ABTestRecord> = self.ab_tests.values().collect();
        tests.sort_by_key(|test| std::cmp::Reverse(test.order_key()));
        tests
    }
}
