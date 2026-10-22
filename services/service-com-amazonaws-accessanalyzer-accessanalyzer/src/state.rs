// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-accessanalyzer-accessanalyzer service.
//!
//! The analyzer is this service's primary resource: every analyzer-scoped operation
//! (`GetAnalyzer`, `ListAnalyzers`, `DeleteAnalyzer`, and the archive rules nested
//! under `/analyzer/{analyzerName}/archive-rule`) resolves the SAME [`Analyzer`]
//! record out of [`ComAmazonawsAccessanalyzerAccessanalyzerState::analyzers`], so a
//! create is observable by every later read and a delete by every later call.
//!
//! The record is the stored half; the wire half (`AnalyzerSummary`,
//! `ArchiveRuleSummary`) is PROJECTED from it by [`Analyzer::summary`] /
//! [`ArchiveRule::summary`], so no handler assembles a response member by hand.
//!
//! Access previews are nested the same way: `CreateAccessPreview` stores an
//! [`AccessPreviewRecord`] on the analyzer the request names, `GetAccessPreview`
//! projects it back with [`AccessPreviewRecord::preview_at`], and deleting the analyzer
//! takes its previews with it.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::time::{Duration, SystemTime};

use crate::ops::types::{
    AccessPreview, AccessPreviewStatusReason, AnalyzerConfiguration, AnalyzerSummary,
    ArchiveRuleSummary, Configuration, ConflictException, Criterion, InternalServerException,
    ResourceNotFoundException, ServiceQuotaExceededException, StatusReason, ValidationException,
    ValidationExceptionField,
};

/// The status a customer-managed analyzer is in once `CreateAnalyzer` returns.
///
/// One of `AnalyzerStatus`' modelled values (`ACTIVE | CREATING | DISABLED | FAILED`).
/// `CreateAnalyzer` answers for an account analyzer that is already monitoring, so the
/// resource is created ready and stays ready; `CREATING`/`FAILED` belong to the
/// organization-analyzer service-linked-role path, which this slice does not model.
pub const ANALYZER_STATUS_ACTIVE: &str = "ACTIVE";

/// The region an ARN carries when the request is unsigned, so no SigV4 credential
/// scope names one. Matches the region the emulator's own generated config defaults to.
pub const DEFAULT_REGION: &str = "us-east-1";

/// This service's ARN namespace / SigV4 signing name, as the generated
/// `OPERATIONS.md` reports it — the `service` segment of every analyzer ARN.
pub const ARN_SERVICE: &str = "access-analyzer";

/// The `resourceType` an analyzer-scoped `ConflictException` /
/// `ResourceNotFoundException` reports: the ARN's own resource-type segment.
pub const ANALYZER_RESOURCE_TYPE: &str = "analyzer";

/// The `resourceType` an archive-rule-scoped error reports.
pub const ARCHIVE_RULE_RESOURCE_TYPE: &str = "archive-rule";

/// The `resourceType` an access-preview-scoped `ResourceNotFoundException` reports.
pub const ACCESS_PREVIEW_RESOURCE_TYPE: &str = "access-preview";

/// The only `Type` an access preview can be created against: the account
/// external-access analyzer. `CreateAccessPreview$analyzerArn` documents it — "You can
/// only create an access preview for analyzers with an Account type and Active status"
/// — so an organization analyzer, or an account analyzer that reports unused or
/// internal access instead of external access, is refused.
pub const ACCESS_PREVIEW_ANALYZER_TYPE: &str = "ACCOUNT";

/// `AccessPreviewStatus` while the analysis of the proposed configuration is still
/// running, from the model's [`crate::ops::types::ACCESS_PREVIEW_STATUS_VALUES`].
pub const ACCESS_PREVIEW_STATUS_CREATING: &str = "CREATING";

/// `AccessPreviewStatus` once the analysis finished and its findings can be previewed.
pub const ACCESS_PREVIEW_STATUS_COMPLETED: &str = "COMPLETED";

/// `AccessPreviewStatus` once the analysis gave up on the proposed configuration.
pub const ACCESS_PREVIEW_STATUS_FAILED: &str = "FAILED";

/// The `AccessPreviewStatusReasonCode` a `FAILED` preview reports when the proposed
/// resource configuration is the reason, from the model's
/// [`crate::ops::types::ACCESS_PREVIEW_STATUS_REASON_CODE_VALUES`].
pub const ACCESS_PREVIEW_REASON_INVALID_CONFIGURATION: &str = "INVALID_CONFIGURATION";

/// How long a created access preview stays `CREATING` before the clock owes it its
/// terminal status.
///
/// AWS publishes no duration for the analysis, so this span is the emulator's own
/// choice: non-zero, so a caller polling straight after `CreateAccessPreview` still
/// observes `CREATING` (and the state it implies), and short enough that a caller
/// waiting for the preview is not kept waiting. Nothing sleeps for it — the due
/// instant is stored and every read compares the clock against it.
pub const ACCESS_PREVIEW_ANALYSIS_SPAN: Duration = Duration::from_millis(500);

/// How many access previews this emulator will hold under one analyzer.
///
/// Same reasoning as [`MAX_ANALYZERS`]: previews are caller-driven growth and
/// `CreateAccessPreview` models `ServiceQuotaExceededException` (HTTP 402) for meeting
/// a quota, so the ceiling is expressed with that error rather than with memory.
pub const MAX_ACCESS_PREVIEWS_PER_ANALYZER: usize = 1_000;

// ---------------------------------------------------------------------------
// Resource-scoped refusals.
//
// `ResourceNotFoundException`, `ConflictException` and
// `ServiceQuotaExceededException` all declare `resourceId` AND `resourceType` as
// REQUIRED members beside `message`
// (`smithy_query(query="shape",shape="ResourceNotFoundException,ConflictException,\
// ServiceQuotaExceededException")`), i.e. the model itself says WHICH resource a
// refusal is about travels in those two structured members. The `message` is
// therefore a FIXED text for the (error, resource kind) pair and is never composed
// from the request: a client that wants the identifier reads `resourceId`, and a
// client that classifies the refusal by message text sees the same string whatever
// it asked for. Every handler in this service builds these three errors through the
// constructors below, so no call site can reintroduce an interpolated message.
// ---------------------------------------------------------------------------

/// The fixed `message` of a `ResourceNotFoundException` for an analyzer this account
/// and Region does not hold — a name that never existed and one read back after
/// `DeleteAnalyzer` are the same refusal, so they carry the same text.
pub const ANALYZER_NOT_FOUND_MESSAGE: &str = "Analyzer not found";

/// The fixed `message` of a `ConflictException` for a `CreateAnalyzer` whose
/// `analyzerName` is already in use. The conflict is STATE, not invalid input: once
/// the stored analyzer is deleted the identical call succeeds.
pub const ANALYZER_NAME_CONFLICT_MESSAGE: &str = "Analyzer with the same name already exists";

/// The fixed `message` of the `ServiceQuotaExceededException` (HTTP 402) that answers
/// a create past this emulator's analyzer ceiling. The ceiling is a property of the
/// account and Region, not of the request, so the text names neither.
pub const ANALYZER_QUOTA_EXCEEDED_MESSAGE: &str =
    "Service quota met for analyzers in this account and Region";

/// The fixed `message` of a `ResourceNotFoundException` for an archive rule the named
/// analyzer does not hold. AWS publishes no text for this refusal, so the wording is
/// the emulator's; what the model fixes is that it is CONSTANT and that the rule name
/// travels in `resourceId`.
pub const ARCHIVE_RULE_NOT_FOUND_MESSAGE: &str = "Archive rule not found";

/// The fixed `message` of a `ConflictException` for a `CreateArchiveRule` whose
/// `ruleName` the analyzer already holds. Same provenance as
/// [`ARCHIVE_RULE_NOT_FOUND_MESSAGE`].
pub const ARCHIVE_RULE_NAME_CONFLICT_MESSAGE: &str =
    "Archive rule with the same name already exists";

/// The fixed `message` of the `ServiceQuotaExceededException` that answers a rule
/// create past this emulator's per-analyzer ceiling.
pub const ARCHIVE_RULE_QUOTA_EXCEEDED_MESSAGE: &str =
    "Service quota met for archive rules on this analyzer";

/// The 404 for an analyzer that is not stored: fixed text, identifier in the modelled
/// `resourceId`/`resourceType` members.
pub fn analyzer_not_found(analyzer_name: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: ANALYZER_NOT_FOUND_MESSAGE.to_string(),
        resource_id: analyzer_name.to_string(),
        resource_type: ANALYZER_RESOURCE_TYPE.to_string(),
    }
}

/// The 409 for a `analyzerName` already in use.
pub fn analyzer_name_conflict(analyzer_name: &str) -> ConflictException {
    ConflictException {
        message: ANALYZER_NAME_CONFLICT_MESSAGE.to_string(),
        resource_id: analyzer_name.to_string(),
        resource_type: ANALYZER_RESOURCE_TYPE.to_string(),
    }
}

/// The 402 for a create past [`MAX_ANALYZERS`].
pub fn analyzer_quota_exceeded(analyzer_name: &str) -> ServiceQuotaExceededException {
    ServiceQuotaExceededException {
        message: ANALYZER_QUOTA_EXCEEDED_MESSAGE.to_string(),
        resource_id: analyzer_name.to_string(),
        resource_type: ANALYZER_RESOURCE_TYPE.to_string(),
    }
}

/// The 404 for an archive rule the analyzer does not hold.
pub fn archive_rule_not_found(rule_name: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: ARCHIVE_RULE_NOT_FOUND_MESSAGE.to_string(),
        resource_id: rule_name.to_string(),
        resource_type: ARCHIVE_RULE_RESOURCE_TYPE.to_string(),
    }
}

/// The 409 for a `ruleName` the analyzer already holds.
pub fn archive_rule_name_conflict(rule_name: &str) -> ConflictException {
    ConflictException {
        message: ARCHIVE_RULE_NAME_CONFLICT_MESSAGE.to_string(),
        resource_id: rule_name.to_string(),
        resource_type: ARCHIVE_RULE_RESOURCE_TYPE.to_string(),
    }
}

/// The 402 for a rule create past [`MAX_ARCHIVE_RULES_PER_ANALYZER`].
pub fn archive_rule_quota_exceeded(rule_name: &str) -> ServiceQuotaExceededException {
    ServiceQuotaExceededException {
        message: ARCHIVE_RULE_QUOTA_EXCEEDED_MESSAGE.to_string(),
        resource_id: rule_name.to_string(),
        resource_type: ARCHIVE_RULE_RESOURCE_TYPE.to_string(),
    }
}

/// The fixed `message` of a `ResourceNotFoundException` for an access preview the named
/// analyzer does not hold — an id that was never minted, one minted under a DIFFERENT
/// analyzer, and one whose analyzer has been deleted are the same refusal, so they
/// carry the same text. Same provenance as [`ARCHIVE_RULE_NOT_FOUND_MESSAGE`].
pub const ACCESS_PREVIEW_NOT_FOUND_MESSAGE: &str = "Access preview not found";

/// The fixed `message` of the `ServiceQuotaExceededException` (HTTP 402) that answers a
/// preview create past this emulator's per-analyzer ceiling.
pub const ACCESS_PREVIEW_QUOTA_EXCEEDED_MESSAGE: &str =
    "Service quota met for access previews on this analyzer";

/// The 404 for an access preview the analyzer does not hold: fixed text, the
/// `accessPreviewId` in the modelled `resourceId`/`resourceType` members.
pub fn access_preview_not_found(access_preview_id: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: ACCESS_PREVIEW_NOT_FOUND_MESSAGE.to_string(),
        resource_id: access_preview_id.to_string(),
        resource_type: ACCESS_PREVIEW_RESOURCE_TYPE.to_string(),
    }
}

/// The 402 for a preview create past [`MAX_ACCESS_PREVIEWS_PER_ANALYZER`].
///
/// The create minted no id, so the refusal names the analyzer whose ceiling was met in
/// `resourceId`; the ceiling is a property of that analyzer, not of the request, so the
/// message names neither.
pub fn access_preview_quota_exceeded(analyzer_arn: &str) -> ServiceQuotaExceededException {
    ServiceQuotaExceededException {
        message: ACCESS_PREVIEW_QUOTA_EXCEEDED_MESSAGE.to_string(),
        resource_id: analyzer_arn.to_string(),
        resource_type: ACCESS_PREVIEW_RESOURCE_TYPE.to_string(),
    }
}

/// How many analyzers this emulator will hold.
///
/// The real service caps analyzers per account per Region with a service quota, and
/// `CreateAnalyzer` models `ServiceQuotaExceededException` (HTTP 402) for exceeding it.
/// This number is NOT a fidelity claim about the published quota (which is far smaller
/// and adjustable): it is the emulator's own ceiling, generous enough that no realistic
/// workflow meets it, so a caller looping on create cannot grow this process's memory
/// without bound.
pub const MAX_ANALYZERS: usize = 1_000;

/// How many archive rules this emulator will hold under one analyzer. Same reasoning,
/// and `CreateArchiveRule` models the same `ServiceQuotaExceededException`.
pub const MAX_ARCHIVE_RULES_PER_ANALYZER: usize = 1_000;

/// How many `clientToken` records are kept per idempotency store.
///
/// Tokens are caller-chosen, so the store is caller-driven growth holding no resource of
/// its own: beyond this many records the oldest is evicted, and records naming a deleted
/// analyzer are dropped with it. An evicted token is indistinguishable from one whose
/// idempotency window has passed — the next create carrying it is a new create.
pub const MAX_REMEMBERED_CLIENT_TOKENS: usize = 10_000;

/// The largest page either listing will return, and the page size used when the caller
/// names no `maxResults`.
///
/// `maxResults` carries no `@range` on `ListAnalyzers`/`ListArchiveRules`, so nothing in
/// the model bounds the response: this is the emulator's own cap on how much one request
/// can amplify into, and a caller that wants the rest follows `nextToken`.
pub const MAX_PAGE_SIZE: i32 = 100;

/// A bounded `clientToken` -> result store for an `@idempotencyToken` create.
///
/// Insert order is kept so the OLDEST record is the one evicted once
/// [`MAX_REMEMBERED_CLIENT_TOKENS`] are held, and a token already recorded keeps the
/// result it recorded (first result wins), so a retry of the first request keeps
/// resolving while a token reused for something else records nothing new.
#[derive(Debug, Clone)]
pub struct ClientTokens<V> {
    records: HashMap<String, V>,
    order: VecDeque<String>,
}

// Hand-written rather than derived: `#[derive(Default)]` would demand `V: Default`,
// which an empty store does not need.
impl<V> Default for ClientTokens<V> {
    fn default() -> Self {
        Self {
            records: HashMap::new(),
            order: VecDeque::new(),
        }
    }
}

impl<V> ClientTokens<V> {
    /// What a previous request carrying this token recorded, if the record is still held.
    pub fn get(&self, token: &str) -> Option<&V> {
        self.records.get(token)
    }

    /// Record what this token produced, keeping the first result for a token already
    /// held and evicting the oldest record once the store is full.
    pub fn remember(&mut self, token: String, value: V) {
        if self.records.contains_key(&token) {
            return;
        }
        self.order.push_back(token.clone());
        self.records.insert(token, value);
        while self.order.len() > MAX_REMEMBERED_CLIENT_TOKENS {
            if let Some(oldest) = self.order.pop_front() {
                self.records.remove(&oldest);
            }
        }
    }

    /// Drop every record `doomed` accepts — used when the resource they name is deleted,
    /// since nothing else would ever reclaim them.
    pub fn forget_where(&mut self, doomed: impl Fn(&V) -> bool) {
        let records = &mut self.records;
        records.retain(|_, value| !doomed(value));
        self.order.retain(|token| records.contains_key(token));
    }

    /// How many records are held.
    pub fn len(&self) -> usize {
        self.records.len()
    }

    /// Whether no record is held.
    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }
}

/// Per-service state for com-amazonaws-accessanalyzer-accessanalyzer. A handler reaches it through [`service_state`].
///
/// `analyzers` is a `BTreeMap` rather than a `HashMap` because the listings page over
/// it: iteration order has to be the same for the call that hands out a `nextToken`
/// and the call that resumes from it, and name order is that stable order.
#[derive(Debug, Default)]
pub struct ComAmazonawsAccessanalyzerAccessanalyzerState {
    /// Every analyzer that exists, keyed by `analyzerName`.
    pub analyzers: BTreeMap<String, Analyzer>,
    /// `CreateAnalyzer` idempotency: the `clientToken` of a completed create mapped to
    /// the analyzer name and ARN it produced, so a retry of the SAME request answers
    /// with the first result instead of a conflict. Bounded; see [`ClientTokens`].
    pub create_analyzer_tokens: ClientTokens<CreatedAnalyzerToken>,
}

impl ComAmazonawsAccessanalyzerAccessanalyzerState {
    /// The analyzer an `analyzerArn` names, or `None` when this account and Region hold
    /// no analyzer with that ARN.
    ///
    /// Analyzers are keyed by name, but the preview operations are scoped by ARN, and
    /// the ARN pins the partition, Region and account as well as the name — so the
    /// lookup compares the whole stored ARN rather than parsing a name out of it.
    pub fn analyzer_by_arn(&self, arn: &str) -> Option<&Analyzer> {
        self.analyzers
            .values()
            .find(|analyzer| analyzer.arn == arn)
    }

    /// [`Self::analyzer_by_arn`], for a caller that writes to the record it finds.
    pub fn analyzer_by_arn_mut(&mut self, arn: &str) -> Option<&mut Analyzer> {
        self.analyzers
            .values_mut()
            .find(|analyzer| analyzer.arn == arn)
    }
}

/// What a recorded `CreateAnalyzer` `clientToken` resolves to.
#[derive(Debug, Clone)]
pub struct CreatedAnalyzerToken {
    /// The `analyzerName` the token was first used with.
    pub analyzer_name: String,
    /// The ARN that create returned.
    pub arn: String,
}

/// A stored analyzer: the state `AnalyzerSummary` is rebuilt from.
#[derive(Debug, Clone)]
pub struct Analyzer {
    pub name: String,
    pub arn: String,
    pub r#type: String,
    /// One of the model's `AnalyzerStatus` values, listed in this crate as
    /// [`crate::ops::types::ANALYZER_STATUS_VALUES`]; a created analyzer carries
    /// [`ANALYZER_STATUS_ACTIVE`].
    pub status: String,
    pub status_reason: Option<StatusReason>,
    pub created_at: SystemTime,
    pub last_resource_analyzed: Option<String>,
    pub last_resource_analyzed_at: Option<SystemTime>,
    /// Exactly the `tags` map the create carried, absent when it carried none.
    pub tags: Option<HashMap<String, String>>,
    /// Exactly the `configuration` the create carried, absent when it carried none.
    pub configuration: Option<AnalyzerConfiguration>,
    /// This analyzer's archive rules, keyed by `ruleName` (ordered, for the same
    /// paging reason as `analyzers`).
    pub archive_rules: BTreeMap<String, ArchiveRule>,
    /// `CreateArchiveRule` idempotency, scoped to this analyzer the way the operation
    /// is: the `clientToken` of a completed create mapped to the `ruleName` it
    /// produced, so a retry of the SAME request answers success instead of a conflict.
    /// Bounded; see [`ClientTokens`].
    pub archive_rule_tokens: ClientTokens<String>,
    /// The access previews created against this analyzer, keyed by their
    /// `accessPreviewId`. Stored ON the analyzer record, the way `archive_rules` is,
    /// because every preview-scoped operation (`GetAccessPreview`,
    /// `ListAccessPreviews`, `ListAccessPreviewFindings`) is scoped by `analyzerArn`
    /// too, so no preview outlives the analyzer it previews for. Ordered by id for the
    /// same paging reason as `analyzers`.
    pub access_previews: BTreeMap<String, AccessPreviewRecord>,
    /// `CreateAccessPreview` idempotency, scoped to this analyzer: the `clientToken` of
    /// a completed create mapped to the `accessPreviewId` it produced, so a retry of
    /// the SAME request answers with the first id instead of previewing twice.
    /// Bounded; see [`ClientTokens`].
    pub access_preview_tokens: ClientTokens<String>,
}

impl Analyzer {
    /// Project the stored analyzer onto the modelled `AnalyzerSummary`.
    pub fn summary(&self) -> AnalyzerSummary {
        AnalyzerSummary {
            arn: self.arn.clone(),
            name: self.name.clone(),
            r#type: self.r#type.clone(),
            created_at: ::aws_smithy_types::DateTime::from(self.created_at),
            last_resource_analyzed: self.last_resource_analyzed.clone(),
            last_resource_analyzed_at: self
                .last_resource_analyzed_at
                .map(::aws_smithy_types::DateTime::from),
            tags: self.tags.clone(),
            status: self.status.clone(),
            status_reason: self.status_reason.clone(),
            configuration: self.configuration.clone(),
            // Only a service-linked analyzer carries a managing service principal, and
            // this slice creates customer-managed analyzers only.
            managed_by: None,
        }
    }
}

/// A stored access preview: the state the modelled `AccessPreview` is rebuilt from.
///
/// The analysis of a proposed configuration is not instantaneous, so the record keeps
/// the instant the analysis is DUE and the terminal status it resolves to; the status a
/// read reports is computed from the clock against that instant by
/// [`AccessPreviewRecord::status_at`], so nothing sleeps and no read has to write.
#[derive(Debug, Clone)]
pub struct AccessPreviewRecord {
    /// The `accessPreviewId` the create returned, in the model's `AccessPreviewId` form.
    pub id: String,
    /// The ARN of the analyzer the preview was created against.
    pub analyzer_arn: String,
    /// Exactly the `configurations` map the create carried.
    pub configurations: HashMap<String, Configuration>,
    pub created_at: SystemTime,
    /// When the analysis is due to finish: before it the preview reads `CREATING`,
    /// at or after it the preview reads its terminal status.
    pub completes_at: SystemTime,
    /// The status the analysis resolves to: `COMPLETED`, or `FAILED` when the proposed
    /// configuration could not be analyzed.
    pub terminal_status: String,
    /// Why the terminal status is `FAILED`; `None` for a `COMPLETED` preview, since
    /// `statusReason` explains a failure.
    pub terminal_status_reason: Option<AccessPreviewStatusReason>,
}

impl AccessPreviewRecord {
    /// The status, and the reason for it, this preview is in at `now`.
    pub fn status_at(&self, now: SystemTime) -> (&str, Option<AccessPreviewStatusReason>) {
        if now < self.completes_at {
            // `statusReason` explains a failure, and an analysis still running has not
            // failed.
            (ACCESS_PREVIEW_STATUS_CREATING, None)
        } else {
            (
                self.terminal_status.as_str(),
                self.terminal_status_reason.clone(),
            )
        }
    }

    /// Project the stored preview onto the modelled `AccessPreview`, as of `now`.
    pub fn preview_at(&self, now: SystemTime) -> AccessPreview {
        let (status, status_reason) = self.status_at(now);
        AccessPreview {
            id: self.id.clone(),
            analyzer_arn: self.analyzer_arn.clone(),
            configurations: self.configurations.clone(),
            created_at: ::aws_smithy_types::DateTime::from(self.created_at),
            status: status.to_string(),
            status_reason,
        }
    }
}

/// Why a proposed access-preview configuration cannot be analyzed, or `None` when it
/// can.
///
/// A proposed configuration is analyzable when the resource it is keyed by is named by
/// an ARN and every policy document it proposes is a JSON object (which is what a
/// resource policy, a trust policy and a key policy all are). Anything else is the
/// "invalid proposed resource configuration" the model's `AccessPreviewStatusReason`
/// documents, which the preview reports by resolving to `FAILED` with
/// [`ACCESS_PREVIEW_REASON_INVALID_CONFIGURATION`] rather than by refusing the create.
pub fn configuration_defect(resource: &str, configuration: &Configuration) -> Option<String> {
    if !is_resource_arn(resource) {
        return Some(format!(
            "{resource} is not the ARN of a resource to preview access for"
        ));
    }
    for document in policy_documents(configuration) {
        if !serde_json::from_str::<serde_json::Value>(document)
            .is_ok_and(|parsed| parsed.is_object())
        {
            return Some(format!(
                "the policy proposed for {resource} is not a JSON policy document"
            ));
        }
    }
    None
}

/// Whether `candidate` is an ARN: the six colon-separated segments of
/// `arn:partition:service:region:account:resource`, the form every `configurations`
/// key (`ConfigurationsMapKey`, documented as "a map of resource ARNs") takes.
fn is_resource_arn(candidate: &str) -> bool {
    let segments: Vec<&str> = candidate.splitn(6, ':').collect();
    segments.len() == 6 && segments[0] == "arn" && !segments[5].is_empty()
}

/// Every policy document a proposed configuration carries, in no particular order.
///
/// One arm per `Configuration` union member (from
/// `smithy_query(query="shape", shape="Configuration")`), so a member that proposes no
/// policy — an EBS snapshot's share list, an RDS snapshot's attributes, a KMS grant —
/// contributes nothing and a member that proposes several contributes all of them.
pub fn policy_documents(configuration: &Configuration) -> Vec<&str> {
    let mut documents: Vec<&str> = Vec::new();
    match configuration {
        // Shared through account ids and groups, not through a policy document.
        Configuration::ebsSnapshot(_) => {}
        Configuration::ecrRepository(config) => {
            documents.extend(config.repository_policy.as_deref())
        }
        Configuration::iamRole(config) => documents.extend(config.trust_policy.as_deref()),
        Configuration::efsFileSystem(config) => {
            documents.extend(config.file_system_policy.as_deref())
        }
        Configuration::kmsKey(config) => documents.extend(
            config
                .key_policies
                .iter()
                .flatten()
                .map(|(_, policy)| policy.as_str()),
        ),
        // Shared through snapshot attributes, not through a policy document.
        Configuration::rdsDbClusterSnapshot(_) | Configuration::rdsDbSnapshot(_) => {}
        Configuration::secretsManagerSecret(config) => {
            documents.extend(config.secret_policy.as_deref())
        }
        Configuration::s3Bucket(config) => {
            documents.extend(config.bucket_policy.as_deref());
            documents.extend(
                config
                    .access_points
                    .iter()
                    .flatten()
                    .filter_map(|(_, point)| point.access_point_policy.as_deref()),
            );
        }
        Configuration::snsTopic(config) => documents.extend(config.topic_policy.as_deref()),
        Configuration::sqsQueue(config) => documents.extend(config.queue_policy.as_deref()),
        Configuration::s3ExpressDirectoryBucket(config) => {
            documents.extend(config.bucket_policy.as_deref());
            documents.extend(
                config
                    .access_points
                    .iter()
                    .flatten()
                    .filter_map(|(_, point)| point.access_point_policy.as_deref()),
            );
        }
        Configuration::dynamodbStream(config) => documents.extend(config.stream_policy.as_deref()),
        Configuration::dynamodbTable(config) => documents.extend(config.table_policy.as_deref()),
    }
    documents
}

/// A fresh `accessPreviewId`: a version-4 UUID in the lowercase hyphenated form the
/// model's `AccessPreviewId` pattern requires
/// (`^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$`; RFC 9562 §5.4 —
/// version `4` in the high nibble of octet 6, variant `0b10` in the top bits of octet 8).
///
/// Its bits come from three independent sources, so an id is both unguessable and
/// unique within the process: the OS entropy `std` seeds a fresh `RandomState` with on
/// every call, a monotonic counter (so two ids minted in the same nanosecond on
/// different threads differ by construction), and the wall clock in nanoseconds (so ids
/// do not repeat across processes).
pub fn new_access_preview_id() -> String {
    use std::fmt::Write as _;
    use std::hash::{BuildHasher, Hasher};
    use std::sync::atomic::{AtomicU64, Ordering};

    static MINTED: AtomicU64 = AtomicU64::new(0);
    let counter = MINTED.fetch_add(1, Ordering::Relaxed);

    let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
    hasher.write_u64(counter);
    let entropy = hasher.finish();

    let nanos = SystemTime::now()
        .duration_since(::std::time::UNIX_EPOCH)
        .map(|since_epoch| since_epoch.as_nanos())
        .unwrap_or_default();

    let mut digest = <::sha2::Sha256 as ::sha2::Digest>::new();
    ::sha2::Digest::update(&mut digest, entropy.to_le_bytes());
    ::sha2::Digest::update(&mut digest, counter.to_le_bytes());
    ::sha2::Digest::update(&mut digest, nanos.to_le_bytes());
    let hashed = ::sha2::Digest::finalize(digest);

    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&hashed[..16]);
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;

    let mut rendered = String::with_capacity(36);
    for (index, byte) in bytes.iter().enumerate() {
        if matches!(index, 4 | 6 | 8 | 10) {
            rendered.push('-');
        }
        // Infallible: writing to a String never fails.
        let _ = write!(rendered, "{byte:02x}");
    }
    rendered
}

/// A stored archive rule: the state `ArchiveRuleSummary` is rebuilt from.
#[derive(Debug, Clone)]
pub struct ArchiveRule {
    pub rule_name: String,
    pub filter: HashMap<String, Criterion>,
    pub created_at: SystemTime,
    pub updated_at: SystemTime,
}

impl ArchiveRule {
    /// Project the stored rule onto the modelled `ArchiveRuleSummary`.
    pub fn summary(&self) -> ArchiveRuleSummary {
        ArchiveRuleSummary {
            rule_name: self.rule_name.clone(),
            filter: self.filter.clone(),
            created_at: ::aws_smithy_types::DateTime::from(self.created_at),
            updated_at: ::aws_smithy_types::DateTime::from(self.updated_at),
        }
    }
}

/// The ARN of an analyzer named `analyzer_name` in `region`:
/// `arn:aws:access-analyzer:<region>:<account>:analyzer/<analyzerName>`.
///
/// The account segment comes from the core's single source of truth, never a literal.
pub fn analyzer_arn(region: &str, analyzer_name: &str) -> String {
    ::engine::emulator_arn(ARN_SERVICE, region, &format!("analyzer/{analyzer_name}"))
}

/// The region a request is scoped to: the one its SigV4 credential scope names, or
/// [`DEFAULT_REGION`] when the request carries no scope (an unsigned call).
///
/// Deliberately a per-crate copy of a helper other service crates also hold: the
/// function that owns the parsing (`emulator_server::sigv4_credential_scope`) lives in
/// the vendored, read-only framework, and a shared `services/common` crate cannot be
/// introduced for it because registering a new workspace member means editing the root
/// `Cargo.toml`, which this project does not allow. Keep the copy local rather than
/// taking a dependency on another SERVICE crate, which would couple two services.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .filter(|region| !region.is_empty())
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The `ValidationExceptionReason` for a request member that failed validation, from
/// the model's own `VALIDATION_EXCEPTION_REASON_VALUES`.
pub const VALIDATION_REASON_FIELD_VALIDATION_FAILED: &str = "fieldValidationFailed";

/// One page of a listing: the items on it, and the token that resumes after them.
#[derive(Debug)]
pub struct Page<T> {
    pub items: Vec<T>,
    pub next_token: Option<String>,
}

/// Why a listing refused its paging arguments. Each handler renders this as its own
/// modelled `ValidationException`, naming the member that was wrong.
#[derive(Debug)]
pub enum PageError {
    /// `nextToken` was not a token this service issued.
    InvalidToken(String),
    /// `maxResults` was not a positive page size.
    InvalidMaxResults(i32),
}

/// Take one page out of a name-ordered listing.
///
/// `all` is every item that matches the request, paired with the key it is ordered
/// by. The page holds at most `max_results` items starting at `next_token`, and
/// carries a token only when further items remain — so a caller that follows tokens
/// to exhaustion sees every item exactly once.
pub fn page<T>(
    all: Vec<(String, T)>,
    max_results: Option<i32>,
    next_token: Option<&str>,
) -> Result<Page<T>, PageError> {
    let start = match next_token {
        None => None,
        Some(token) => Some(
            decode_page_token(token).ok_or_else(|| PageError::InvalidToken(token.to_string()))?,
        ),
    };
    let limit = match max_results {
        // No `maxResults`: the page is the emulator's own cap, never the whole store.
        None => MAX_PAGE_SIZE as usize,
        // A requested page larger than the cap is clamped to it; the caller follows
        // `nextToken` for the rest.
        Some(requested) if requested >= 1 => requested.min(MAX_PAGE_SIZE) as usize,
        Some(requested) => return Err(PageError::InvalidMaxResults(requested)),
    };

    let mut rest = all
        .into_iter()
        .skip_while(|(key, _)| start.as_deref().is_some_and(|from| key.as_str() < from));
    let items: Vec<T> = rest.by_ref().take(limit).map(|(_, item)| item).collect();
    // The token names the first item NOT on this page, so the next call resumes at it.
    let next_token = rest.next().map(|(key, _)| encode_page_token(&key));
    Ok(Page { items, next_token })
}

/// Render a paging refusal as this service's modelled `ValidationException`.
///
/// `fieldList` names the member that was wrong, which is how a client learns what to
/// fix, and the reason is the model's `fieldValidationFailed`.
pub fn page_validation_exception(error: &PageError) -> ValidationException {
    let (name, message) = match error {
        PageError::InvalidToken(token) => (
            "nextToken",
            format!("{token} is not a valid pagination token"),
        ),
        PageError::InvalidMaxResults(requested) => (
            "maxResults",
            format!("maxResults must be at least 1, but was {requested}"),
        ),
    };
    ValidationException {
        message: message.clone(),
        reason: VALIDATION_REASON_FIELD_VALIDATION_FAILED.to_string(),
        field_list: Some(vec![ValidationExceptionField {
            name: name.to_string(),
            message,
        }]),
    }
}

/// Encode a listing key as an opaque continuation token.
///
/// The key is prefixed with a version marker before hex-encoding, so the token is
/// opaque to a client (it is not the resource name it was built from) AND a value this
/// service did not issue is refused rather than resumed from: hex alone would accept
/// any even-length hex string as a cursor. It stays a pure function of the key, so the
/// token is stable and carries no server-side cursor state.
fn encode_page_token(key: &str) -> String {
    ::hex::encode(format!("{PAGE_TOKEN_PREFIX}{key}"))
}

/// The inverse of [`encode_page_token`]; `None` for anything this service did not issue.
fn decode_page_token(token: &str) -> Option<String> {
    let decoded = String::from_utf8(::hex::decode(token).ok()?).ok()?;
    decoded.strip_prefix(PAGE_TOKEN_PREFIX).map(str::to_string)
}

/// Marks a continuation token as this service's, and at which version it was issued.
const PAGE_TOKEN_PREFIX: &str = "accessanalyzer:v1:";

/// The modelled server error every handler answers with when [`service_state`] finds
/// no store (this service mounted on an engine that holds none). The payload is shared;
/// each operation wraps it in its OWN generated error variant.
pub fn store_unavailable() -> InternalServerException {
    InternalServerException {
        message: "the access-analyzer store is unavailable".to_string(),
        // Nothing about a missing store resolves with time, so no retry hint.
        retry_after_seconds: None,
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsAccessanalyzerAccessanalyzerState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsAccessanalyzerAccessanalyzerState>)
}
