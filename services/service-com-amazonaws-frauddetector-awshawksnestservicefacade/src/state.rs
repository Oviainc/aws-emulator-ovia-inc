// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-frauddetector-awshawksnestservicefacade service.
//!
//! The detector lifecycle (`PutDetector`, `CreateRule`, `CreateDetectorVersion`,
//! `GetDetectorVersion`, `GetDetectors`, `DescribeDetector`, `DeleteDetector`) is served
//! entirely out of [`ComAmazonawsFrauddetectorAwshawksnestservicefacadeState::detectors`]:
//! every id is allocated from the counters held here and every response member is read
//! back out of these records, so a write is visible to the next request and to no other
//! source of truth.

use std::collections::BTreeMap;

use crate::ops::types::{ModelVersion, Tag};

/// Per-service state for com-amazonaws-frauddetector-awshawksnestservicefacade. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsFrauddetectorAwshawksnestservicefacadeState {
    /// Every detector `PutDetector` has created, keyed by `detectorId`. A `BTreeMap`
    /// so `GetDetectors` pages over a stable, total order and a `nextToken` naming a
    /// detector id resumes exactly where the previous page stopped.
    pub detectors: BTreeMap<String, DetectorRecord>,
}

/// One stored detector, with the rules and detector versions hung off it.
#[derive(Debug, Clone)]
pub struct DetectorRecord {
    pub detector_id: String,
    pub event_type_name: String,
    pub description: Option<String>,
    pub arn: String,
    pub created_time: String,
    pub last_updated_time: String,
    pub tags: Vec<Tag>,
    /// `ruleId` -> every version of that rule, oldest first: the index in the `Vec` is
    /// `ruleVersion - 1`, so allocating the next version is `len() + 1`.
    pub rules: BTreeMap<String, Vec<RuleRecord>>,
    /// `detectorVersionId` (numeric, so the map orders `2` before `10`) -> the version.
    pub versions: BTreeMap<u32, DetectorVersionRecord>,
    /// The next `detectorVersionId` to hand out. Allocated from here and never
    /// recomputed from `versions.len()`, so a deleted version's id is not reused.
    pub next_version_id: u32,
}

/// One version of one rule of a detector.
#[derive(Debug, Clone)]
pub struct RuleRecord {
    pub rule_id: String,
    pub rule_version: String,
    pub expression: String,
    pub language: String,
    pub outcomes: Vec<String>,
    pub description: Option<String>,
    pub tags: Vec<Tag>,
    pub created_time: String,
    pub last_updated_time: String,
    pub arn: String,
}

/// One detector version: the rule references, models and execution mode it was created
/// with, plus the status it currently carries.
#[derive(Debug, Clone)]
pub struct DetectorVersionRecord {
    pub detector_version_id: String,
    pub description: Option<String>,
    /// A `DetectorVersionStatus` value — see [`DETECTOR_VERSION_STATUS_DRAFT`].
    pub status: String,
    /// `(ruleId, ruleVersion)` pairs, resolved against the detector's stored rules when
    /// the version was created.
    pub rule_refs: Vec<(String, String)>,
    pub model_versions: Vec<ModelVersion>,
    pub external_model_endpoints: Vec<String>,
    /// A `RuleExecutionMode` value — see [`RULE_EXECUTION_MODE_DEFAULT`].
    pub rule_execution_mode: String,
    pub created_time: String,
    pub last_updated_time: String,
    pub arn: String,
    pub tags: Vec<Tag>,
}

/// The status a freshly created detector version carries. One of
/// `crate::ops::types::DETECTOR_VERSION_STATUS_VALUES`; a version only leaves `DRAFT`
/// through `UpdateDetectorVersionStatus`, so there is no clock-owed transition here.
pub const DETECTOR_VERSION_STATUS_DRAFT: &str = "DRAFT";

/// The `ruleExecutionMode` a detector version gets when the request omits it, as the
/// model documents for `CreateDetectorVersion`. One of
/// `crate::ops::types::RULE_EXECUTION_MODE_VALUES`.
pub const RULE_EXECUTION_MODE_DEFAULT: &str = "FIRST_MATCHED";

/// The largest `detectorVersionId` / `ruleVersion` this service can report: both are
/// modelled as `wholeNumberVersionString`, `^([1-9][0-9]*)$` with length 1..5, so a
/// version past this one could not be rendered at all. Allocation stops here rather
/// than reusing an id or emitting one the model refuses.
pub const MAX_VERSION_ID: u32 = 99_999;

/// How many detectors, and how many distinct rules per detector, this emulator holds.
///
/// NOT an AWS quota: Fraud Detector's own account limits are adjustable and are not in
/// the model, so nothing here can claim a number. These are the emulator's own capacity
/// guards, so an in-memory store cannot grow without bound while the process lives
/// (`DeleteDetector` is the only release path in this slice, and `DeleteRule` is still a
/// 501). They sit far above any fixture a test builds, so a caller never meets them by
/// accident; a request that would cross one is refused with the modelled
/// `ValidationException` naming the limit, never served and dropped.
pub const MAX_DETECTORS: usize = 10_000;
/// See [`MAX_DETECTORS`].
pub const MAX_RULES_PER_DETECTOR: usize = 10_000;

/// The region an ARN carries when the request is unsigned, so no SigV4 credential scope
/// names one.
pub const DEFAULT_REGION: &str = "us-east-1";

/// This service's ARN namespace / endpoint prefix (`OPERATIONS.md`, "Wire protocol").
const ARN_SERVICE: &str = "frauddetector";

/// The region segment of the request's SigV4 credential scope, or [`DEFAULT_REGION`].
///
/// The modeled `fraudDetectorArn` pattern constrains that segment to
/// `[a-z0-9-]{3,20}`, so a scope whose region cannot appear in an ARN falls back to the
/// default rather than emitting a value the model refuses.
pub fn region_from_request(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .filter(|region| {
            (3..=20).contains(&region.len())
                && region
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        })
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// `arn:aws:frauddetector:<region>:<account>:detector/<detectorId>`.
pub fn detector_arn(region: &str, detector_id: &str) -> String {
    ::engine::emulator_arn(ARN_SERVICE, region, &format!("detector/{detector_id}"))
}

/// `arn:aws:frauddetector:<region>:<account>:detector-version/<detectorId>/<versionId>`.
pub fn detector_version_arn(region: &str, detector_id: &str, detector_version_id: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("detector-version/{detector_id}/{detector_version_id}"),
    )
}

/// `arn:aws:frauddetector:<region>:<account>:rule/<detectorId>/<ruleId>/<ruleVersion>`.
pub fn rule_arn(region: &str, detector_id: &str, rule_id: &str, rule_version: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("rule/{detector_id}/{rule_id}/{rule_version}"),
    )
}

/// Now, as the ISO-8601 string the model's `time` shape carries (length 11..30).
///
/// Truncated to milliseconds so the rendering stays well inside that bound while keeping
/// sub-second resolution, and formatted through `aws_smithy_types`' own RFC-3339 writer
/// rather than a hand-rolled one.
pub fn now_timestamp() -> String {
    let epoch_millis = ::std::time::SystemTime::now()
        .duration_since(::std::time::UNIX_EPOCH)
        .map_or(0_i64, |elapsed| {
            i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX)
        });
    ::aws_smithy_types::DateTime::from_millis(epoch_millis)
        .fmt(::aws_smithy_types::date_time::Format::DateTime)
        // An epoch-millisecond instant always renders; the fallback keeps the handler
        // path infallible rather than turning a clock oddity into a 500.
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_string())
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsFrauddetectorAwshawksnestservicefacadeState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsFrauddetectorAwshawksnestservicefacadeState>)
}
