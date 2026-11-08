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
//! takes its previews with it. The preview's FINDINGS are not stored at all:
//! `ListAccessPreviewFindings` derives them from the stored proposed configuration (the
//! policies [`proposed_policies`] enumerates, the resource type
//! [`configuration_resource_type`] names) on every read, so there is no second copy of
//! the analysis to keep in step with the configuration it came from.
//!
//! So are resource scans: `StartResourceScan` records a [`ResourceScan`] on the analyzer
//! it names, and the analyzer's `lastResourceAnalyzed` / `lastResourceAnalyzedAt` are
//! DERIVED from those records against the clock by [`Analyzer::summary_at`] — there is no
//! separately stored copy of either member for a scan to have to keep in step.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::time::{Duration, SystemTime};

/// The external access the account's own resource-based policies grant: the ONE analysis
/// every external-access finding this service reports is derived from, and the grant rule
/// the access-preview analysis shares with it.
mod external_access;
pub use external_access::*;

use crate::ops::types::{
    AccessPreview, AccessPreviewStatusReason, AnalyzerConfiguration, AnalyzerSummary,
    ArchiveRuleSummary, CloudTrailProperties, Configuration, ConflictException, Criterion,
    GeneratedPolicy, GeneratedPolicyProperties, GeneratedPolicyResult, InternalServerException,
    JobDetails, PolicyGeneration, ResourceNotFoundException, ServiceQuotaExceededException,
    StatusReason, Trail, TrailProperties, ValidationException, ValidationExceptionField,
};

/// The HTTP status a successful call that has NO content to report answers with.
///
/// Every operation of this service leaves its success code defaulted in the model
/// (`@http` carries no `code:`, i.e. `codeDefaulted: true`), so the Smithy default of
/// 200 is not a statement about this service — the status is a behavioural fact, and
/// the recorded AWS responses are what states it: a call whose output is a DECLARED
/// structure with no members (`TagResourceResponse`, `UntagResourceResponse`,
/// `CancelPolicyGenerationResponse` — the only three in this model) is answered with
/// 204 No Content, because there is no content to parse. A 200 there promises a body
/// the response does not carry, so every client path gated on 204 (conditional body
/// parsing, caches and proxies that key on it) takes the wrong branch.
///
/// This does NOT extend to an operation whose output is `smithy.api#Unit` — the model
/// gives it no response structure at all, and the recordings show AWS answering those
/// (`CreateArchiveRule`, `UpdateArchiveRule`, `DeleteArchiveRule`, `DeleteAnalyzer`, …)
/// with the defaulted 200; those operations keep it.
pub const NO_CONTENT_SUCCESS_STATUS: u16 = 204;

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

/// How long a scan started by `StartResourceScan` runs before the clock owes the
/// analyzer the result.
///
/// `StartResourceScan` "immediately starts a scan" and answers with no body, so nothing
/// in the response says the scan has FINISHED; what observes the result is a later read
/// of the analyzer (`AnalyzerSummary$lastResourceAnalyzed` /
/// `lastResourceAnalyzedAt`). AWS publishes no duration for the scan, so this span is
/// the emulator's own choice, chosen like [`ACCESS_PREVIEW_ANALYSIS_SPAN`]: non-zero, so
/// a caller reading the analyzer straight after the call still sees the scan outstanding,
/// and short enough not to keep a caller waiting. Nothing sleeps for it — the due instant
/// is stored and every read compares the clock against it.
pub const RESOURCE_SCAN_SPAN: Duration = Duration::from_millis(500);

/// How many distinct scanned resources this emulator remembers under one analyzer.
///
/// Scans are caller-driven growth keyed by `resourceArn`, and `StartResourceScan` models
/// no quota error at all (only 400/403/404/429/500), so the ceiling cannot be reported as
/// a quota: past it the LEAST recently scanned resource is forgotten, the way
/// [`ClientTokens`] evicts its oldest record. Rescanning a resource already held replaces
/// its record rather than adding one, so a caller rescanning one resource never evicts
/// anything.
pub const MAX_SCANNED_RESOURCES_PER_ANALYZER: usize = 1_000;

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

/// The fixed `message` of the `NotFoundException` that answers an ARN-addressed call
/// (the tagging surface) naming a resource this account and Region does not hold.
/// Same provenance as [`ANALYZER_NOT_FOUND_MESSAGE`]: ONE constant text for the refusal,
/// whatever ARN was asked for — this is the recorded service's own wording for the
/// refusal, so it is reproduced verbatim rather than paraphrased.
pub const RESOURCE_NOT_FOUND_MESSAGE: &str = "Resource was not found";

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

/// The fixed `message` of the `ConflictException` (HTTP 409) that answers a call asking
/// for the RESULT of an access preview whose analysis has not completed — one still
/// `CREATING`, and one that resolved to `FAILED`, which produced no findings at all.
/// Same provenance as [`ACCESS_PREVIEW_NOT_FOUND_MESSAGE`]: one constant text per
/// refusal kind, with the identifier in the modelled `resourceId`.
pub const ACCESS_PREVIEW_NOT_COMPLETED_MESSAGE: &str =
    "Access preview findings are available once the access preview is completed";

/// The 409 for an access preview whose analysis has not completed: fixed text, the
/// `accessPreviewId` in the modelled `resourceId`/`resourceType` members.
pub fn access_preview_not_completed(access_preview_id: &str) -> ConflictException {
    ConflictException {
        message: ACCESS_PREVIEW_NOT_COMPLETED_MESSAGE.to_string(),
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

/// How many tags one resource will hold.
///
/// `TagsMap` carries no `@length` in the model, so nothing in the model bounds how many
/// tags one `TagResource` call can add: this is the emulator's own ceiling on a
/// caller-driven map, set at the AWS-wide tagging limit of 50 tags per resource
/// ("Tagging AWS resources", tag restrictions). A call that would take a resource past
/// it is refused with the modelled `ValidationException`.
pub const MAX_TAGS_PER_RESOURCE: usize = 50;

/// The longest tag key this service accepts, from the same AWS-wide tag restrictions
/// (maximum key length 128 Unicode characters); the shortest is one character, since an
/// empty key names nothing.
pub const MAX_TAG_KEY_CHARS: usize = 128;

/// The longest tag value this service accepts, from the same AWS-wide tag restrictions
/// (maximum value length 256 Unicode characters). A value MAY be empty.
pub const MAX_TAG_VALUE_CHARS: usize = 256;

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
    /// Every policy generation `StartPolicyGeneration` has started, keyed by its `jobId`.
    ///
    /// Account-scoped rather than nested under an analyzer, the way the operations that
    /// reach it are: a generation is asked for an IAM principal and addressed by `jobId`
    /// alone, and no analyzer appears anywhere in its contract. A `BTreeMap` for the same
    /// reason as `analyzers`: `ListPolicyGenerations` pages over it, so its iteration
    /// order has to be the same for the call that hands out a `nextToken` and the call
    /// that resumes from it.
    pub policy_generations: BTreeMap<String, PolicyGenerationJob>,
    /// `StartPolicyGeneration` idempotency: the `clientToken` of a completed start mapped
    /// to the `jobId` it produced, so a retry of the SAME request answers with the first
    /// job instead of generating a second policy. Bounded; see [`ClientTokens`].
    pub start_policy_generation_tokens: ClientTokens<String>,
}

impl ComAmazonawsAccessanalyzerAccessanalyzerState {
    /// The analyzer an `analyzerArn` names, or `None` when this account and Region hold
    /// no analyzer with that ARN.
    ///
    /// Analyzers are keyed by name, but the preview and tagging operations are scoped by
    /// ARN, and the ARN pins the partition, Region and account as well as the name — so
    /// the lookup compares the whole stored ARN rather than parsing a name out of it,
    /// and an ARN this account and Region never minted names no resource.
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
    /// Every resource `StartResourceScan` has been asked to scan with this analyzer,
    /// keyed by `resourceArn` (ordered, for the same paging reason as `analyzers`).
    ///
    /// This is the ONLY source of `AnalyzerSummary$lastResourceAnalyzed` /
    /// `lastResourceAnalyzedAt`, which [`Analyzer::summary_at`] derives from it against
    /// the clock — there is no second, separately stored copy of those two members to
    /// drift from the scans that produced them.
    pub resource_scans: BTreeMap<String, ResourceScan>,
    /// The analyzer's tag set: exactly the `tags` map the create carried, EMPTY when it
    /// carried none, as the tagging operations have since changed it.
    ///
    /// A tagged resource always has a tag collection — an untagged one has an empty
    /// collection, not a missing one — so this is a map and never an `Option`, and
    /// [`Self::summary`] projects it as the (possibly empty) `TagsMap` the model
    /// declares rather than suppressing the member.
    pub tags: HashMap<String, String>,
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

/// Which `AnalyzerConfiguration` union member an analyzer's `type` configures.
///
/// The model's `Type` enum has three families (`smithy_query(query="shape",
/// shape="Type")`): the `_UNUSED_ACCESS` types configure `unusedAccess`, the
/// `_INTERNAL_ACCESS` types configure `internalAccess`, and plain `ACCOUNT` /
/// `ORGANIZATION` are the external access analyzers, which configure neither.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfiguredAccess {
    /// `ACCOUNT_UNUSED_ACCESS` / `ORGANIZATION_UNUSED_ACCESS`.
    UnusedAccess,
    /// `ACCOUNT_INTERNAL_ACCESS` / `ORGANIZATION_INTERNAL_ACCESS`.
    InternalAccess,
}

impl ConfiguredAccess {
    /// The union member this family's configuration is carried in, as the wire names it.
    pub fn member_name(self) -> &'static str {
        match self {
            Self::UnusedAccess => "unusedAccess",
            Self::InternalAccess => "internalAccess",
        }
    }

    /// Whether `configuration` is the union member this family configures.
    pub fn matches(self, configuration: &AnalyzerConfiguration) -> bool {
        matches!(
            (self, configuration),
            (
                Self::UnusedAccess,
                AnalyzerConfiguration::unusedAccess(_)
            ) | (
                Self::InternalAccess,
                AnalyzerConfiguration::internalAccess(_)
            )
        )
    }
}

/// Which configuration family an analyzer `type` belongs to, or `None` for an external
/// access analyzer, which has no `AnalyzerConfiguration` to set.
///
/// The arms name the `Type` values the model publishes
/// (`crate::ops::types::TYPE_VALUES`); a value this match does not name falls into the
/// external-access arm, which is the conservative answer — `UpdateAnalyzer` then refuses
/// it rather than storing a configuration under a family nothing here knows.
pub fn configured_access(analyzer_type: &str) -> Option<ConfiguredAccess> {
    match analyzer_type {
        "ACCOUNT_UNUSED_ACCESS" | "ORGANIZATION_UNUSED_ACCESS" => {
            Some(ConfiguredAccess::UnusedAccess)
        }
        "ACCOUNT_INTERNAL_ACCESS" | "ORGANIZATION_INTERNAL_ACCESS" => {
            Some(ConfiguredAccess::InternalAccess)
        }
        // `ACCOUNT` and `ORGANIZATION` are external access analyzers.
        _ => None,
    }
}

impl Analyzer {
    /// Which configuration family THIS analyzer belongs to; see [`configured_access`].
    pub fn configured_access(&self) -> Option<ConfiguredAccess> {
        configured_access(&self.r#type)
    }

    /// The scan of this analyzer's that had FINISHED by `now`, most recently — or `None`
    /// while no scan of it has finished yet.
    ///
    /// A scan started but not yet due ([`RESOURCE_SCAN_SPAN`]) has analyzed nothing, so
    /// it is not a candidate: that is what makes a read taken straight after
    /// `StartResourceScan` still report the previous result. Two scans due at the very
    /// same instant are ordered by `resourceArn`, the map's own order, so the answer is
    /// deterministic rather than dependent on iteration luck.
    pub fn last_completed_scan(&self, now: SystemTime) -> Option<&ResourceScan> {
        self.resource_scans
            .values()
            .filter(|scan| scan.completes_at <= now)
            .max_by_key(|scan| scan.completes_at)
    }

    /// Record that `resource_arn` has been scanned by this analyzer, as of `now`.
    ///
    /// Rescanning a resource the analyzer already scanned REPLACES that resource's scan
    /// (one resource has one most-recent scan), keeping the instant its first scan
    /// started; a resource it has not scanned is added, evicting the least recently
    /// scanned record once [`MAX_SCANNED_RESOURCES_PER_ANALYZER`] are held.
    pub fn record_scan(&mut self, resource_arn: &str, owner_account: String, now: SystemTime) {
        let first_started_at = self
            .resource_scans
            .get(resource_arn)
            .map_or(now, |previous| previous.first_started_at);
        if !self.resource_scans.contains_key(resource_arn)
            && self.resource_scans.len() >= MAX_SCANNED_RESOURCES_PER_ANALYZER
        {
            if let Some(evicted) = self
                .resource_scans
                .values()
                .min_by_key(|scan| scan.started_at)
                .map(|scan| scan.resource_arn.clone())
            {
                self.resource_scans.remove(&evicted);
            }
        }
        self.resource_scans.insert(
            resource_arn.to_string(),
            ResourceScan {
                resource_arn: resource_arn.to_string(),
                resource_owner_account: owner_account,
                first_started_at,
                started_at: now,
                completes_at: now + RESOURCE_SCAN_SPAN,
            },
        );
    }

    /// Project the stored analyzer onto the modelled `AnalyzerSummary`, as of `now`.
    ///
    /// `now` is taken rather than read here because the two `lastResourceAnalyzed`
    /// members are DERIVED from the analyzer's scans against the clock
    /// ([`Analyzer::last_completed_scan`]), so a listing projects every analyzer it
    /// reports at one instant instead of a slightly different one each.
    pub fn summary_at(&self, now: SystemTime) -> AnalyzerSummary {
        let last_scan = self.last_completed_scan(now);
        AnalyzerSummary {
            arn: self.arn.clone(),
            name: self.name.clone(),
            r#type: self.r#type.clone(),
            created_at: ::aws_smithy_types::DateTime::from(self.created_at),
            // "The resource that was most recently analyzed by the analyzer", and "the
            // time at which the most recently analyzed resource was analyzed" — the
            // instant the scan finished, not the instant it was asked for. Both are
            // absent until a scan of this analyzer has finished.
            last_resource_analyzed: last_scan.map(|scan| scan.resource_arn.clone()),
            last_resource_analyzed_at: last_scan
                .map(|scan| ::aws_smithy_types::DateTime::from(scan.completes_at)),
            // Present even when empty: an untagged analyzer has an empty tag set, which
            // is `{}` on the wire, not an absent member.
            tags: Some(self.tags.clone()),
            status: self.status.clone(),
            status_reason: self.status_reason.clone(),
            configuration: self.configuration.clone(),
            // Only a service-linked analyzer carries a managing service principal, and
            // this slice creates customer-managed analyzers only.
            managed_by: None,
        }
    }

    /// This resource's tag set, as `ListTagsForResource` reports it: the stored map,
    /// empty when the resource carries no tags.
    pub fn tag_set(&self) -> HashMap<String, String> {
        self.tags.clone()
    }

    /// How many tags the resource carries.
    pub fn tag_count(&self) -> usize {
        self.tags.len()
    }

    /// Add `tags` to the tag set: a key the set does not hold is added, and a key it
    /// already holds takes the new value. Every other tag is left alone, so tagging is
    /// a merge and never a replacement of the set.
    pub fn add_tags(&mut self, tags: impl IntoIterator<Item = (String, String)>) {
        self.tags.extend(tags);
    }

    /// Whether the tag set holds `key`, so a caller can tell an overwrite from an
    /// addition.
    pub fn holds_tag(&self, key: &str) -> bool {
        self.tags.contains_key(key)
    }

    /// Remove every tag whose key `doomed` accepts, keeping the rest. A key the set
    /// does not hold removes nothing, so removal is idempotent, and removing the last
    /// tag leaves the set EMPTY rather than absent — the resource still has a tag
    /// collection.
    pub fn remove_tags(&mut self, doomed: impl Fn(&str) -> bool) {
        self.tags.retain(|key, _| !doomed(key.as_str()));
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

// ---------------------------------------------------------------------------
// Policy generation jobs.
//
// `StartPolicyGeneration` mints ONE [`PolicyGenerationJob`] per accepted request and
// stores it in [`ComAmazonawsAccessanalyzerAccessanalyzerState::policy_generations`];
// `GetGeneratedPolicy`, `CancelPolicyGeneration` and `ListPolicyGenerations` all resolve
// the SAME record, so the `jobId` the start returns is the one every later call tracks
// and there is no second copy of a job's status to keep in step.
// ---------------------------------------------------------------------------

/// The `JobStatus` a generation that is still running reports, from the model's own enum
/// ([`crate::ops::types::JOB_STATUS_VALUES`]).
pub const JOB_STATUS_IN_PROGRESS: &str = "IN_PROGRESS";

/// The `JobStatus` a generation that ran to completion reports.
pub const JOB_STATUS_SUCCEEDED: &str = "SUCCEEDED";

/// The `JobStatus` a generation `CancelPolicyGeneration` stopped before it completed
/// reports.
///
/// `FAILED`, the fourth value the enum carries, is what a generation whose analysis could
/// not run at all reports; it travels with a `JobError`, and an accepted request has no
/// such failure to report, so no stored job resolves to it.
pub const JOB_STATUS_CANCELED: &str = "CANCELED";

/// How long a started policy generation runs before the clock owes it its terminal
/// status.
///
/// AWS publishes no duration for the generation, so this span is the emulator's own
/// choice, made like [`ACCESS_PREVIEW_ANALYSIS_SPAN`]: non-zero, so a caller reading or
/// cancelling straight after `StartPolicyGeneration` still observes `IN_PROGRESS` and the
/// state it implies, and short enough that a caller waiting for the policy is not kept
/// waiting. Nothing sleeps for it — the due instant is stored and every read compares the
/// clock against it.
pub const POLICY_GENERATION_SPAN: Duration = Duration::from_secs(3);

/// How long a started generation stays part of `ListPolicyGenerations`' answer.
///
/// That operation is documented as listing "all of the policy generations requested in
/// the last seven days", so a job started longer ago than this is no longer listed — the
/// listing is filtered against the clock rather than the store being pruned by a read.
pub const POLICY_GENERATION_RETENTION: Duration = Duration::from_secs(7 * 24 * 60 * 60);

/// How many policy generation jobs this emulator will hold.
///
/// Same reasoning as [`MAX_ANALYZERS`]: jobs are caller-driven growth and
/// `StartPolicyGeneration` models `ServiceQuotaExceededException` (HTTP 402) for meeting a
/// quota, so the ceiling is expressed with that error rather than with memory.
pub const MAX_POLICY_GENERATION_JOBS: usize = 1_000;

/// The `resourceType` a policy-generation-scoped refusal reports.
pub const POLICY_GENERATION_RESOURCE_TYPE: &str = "policy-generation";

/// The fixed `message` of the `ServiceQuotaExceededException` (HTTP 402) that answers a
/// start past this emulator's job ceiling. The ceiling is a property of the account and
/// Region, not of the request, so the text names neither.
pub const POLICY_GENERATION_QUOTA_EXCEEDED_MESSAGE: &str =
    "Service quota met for policy generations in this account and Region";

/// The 402 for a start past [`MAX_POLICY_GENERATION_JOBS`].
///
/// The start minted no `jobId`, so the refusal names the principal the generation was
/// asked for in the modelled `resourceId`.
pub fn policy_generation_quota_exceeded(principal_arn: &str) -> ServiceQuotaExceededException {
    ServiceQuotaExceededException {
        message: POLICY_GENERATION_QUOTA_EXCEEDED_MESSAGE.to_string(),
        resource_id: principal_arn.to_string(),
        resource_type: POLICY_GENERATION_RESOURCE_TYPE.to_string(),
    }
}

/// The 400 for a `jobId` that names no policy generation job.
///
/// `GetGeneratedPolicy` and `CancelPolicyGeneration` model no 404 at all (403, 429, 500
/// and this 400 are every error they declare), so a `jobId` this account and Region never
/// minted is refused as a field validation of `jobId` — which is also the member a client
/// has to correct.
///
/// AWS words it as `Invalid jobId: ` + the submitted id + ` does not exist`: fixed text
/// either side of the only variable part, the `jobId` the request named. Every operation
/// that resolves a job renders this ONE template, so a client that can only tell "no such
/// job" from the message (there being no distinct error code) reads the same sentence
/// whichever of them it called.
pub fn policy_generation_not_found(job_id: &str) -> ValidationException {
    field_validation_exception("jobId", format!("Invalid jobId: {job_id} does not exist"))
}

/// The 400 for a cancel of a job that is no longer running.
///
/// Cancelling stops work that is still in progress; a job that has already reached a
/// terminal status has none left to stop, so the request is refused rather than silently
/// doing nothing, and the message reports the status the job is actually in.
pub fn policy_generation_not_cancelable(job_id: &str, status: &str) -> ValidationException {
    field_validation_exception(
        "jobId",
        format!(
            "jobId must name a policy generation job in status {JOB_STATUS_IN_PROGRESS}, \
             but {job_id} is in status {status}"
        ),
    )
}

/// The IAM policy language version a generated policy document carries — the `Version`
/// element of every policy IAM Access Analyzer generates.
pub const GENERATED_POLICY_VERSION: &str = "2012-10-17";

/// The policy document a completed generation produces.
///
/// A generated policy is the set of actions the principal was OBSERVED using in the
/// CloudTrail events of the analysed time range. This emulator records no CloudTrail
/// events for any principal, so the observed set is empty whatever trails and range a job
/// names, and the document it produces is a policy with no statements: computed from the
/// activity the store holds, not assembled to look generated. The document is built with
/// `serde_json`, so it is valid JSON by construction.
fn generated_policy_document() -> String {
    ::serde_json::json!({
        "Version": GENERATED_POLICY_VERSION,
        "Statement": [],
    })
    .to_string()
}

/// One policy generation job, as `StartPolicyGeneration` records it.
///
/// A generation is not instantaneous, so the record keeps the instant it is DUE to have
/// finished and the instant a cancel stopped it; the status a read reports is computed
/// from the clock against those by [`PolicyGenerationJob::status_at`], so nothing sleeps
/// and no read has to write.
#[derive(Debug, Clone)]
pub struct PolicyGenerationJob {
    /// The `jobId` the start returned, the handle every later call names the job by.
    pub job_id: String,
    /// The IAM entity the policy is generated for, exactly as
    /// `policyGenerationDetails.principalArn` named it.
    pub principal_arn: String,
    /// Exactly the trails `cloudTrailDetails.trails` named, which
    /// `GeneratedPolicyProperties$cloudTrailProperties` reports back.
    pub trails: Vec<Trail>,
    /// The start of the analysed time range, as the request named it.
    pub analysis_start_time: ::aws_smithy_types::DateTime,
    /// The end of the analysed time range: the instant the request named, or — since
    /// `cloudTrailDetails.endTime` is documented to default to "the current time" — the
    /// instant the generation was started.
    pub analysis_end_time: ::aws_smithy_types::DateTime,
    /// When the generation was started.
    pub started_at: SystemTime,
    /// When the generation is DUE to have finished: `started_at` plus
    /// [`POLICY_GENERATION_SPAN`].
    pub completes_at: SystemTime,
    /// When `CancelPolicyGeneration` stopped the generation, while it was still running;
    /// `None` for a job nobody cancelled.
    pub canceled_at: Option<SystemTime>,
}

impl PolicyGenerationJob {
    /// The status this job is in at `now`, and the instant it reached that status —
    /// `None` while the generation is still running, which has completed at no instant.
    ///
    /// A cancel is what it reports first: a cancelled generation stopped where it was and
    /// never reaches the status the clock would otherwise owe it.
    pub fn status_at(&self, now: SystemTime) -> (&'static str, Option<SystemTime>) {
        if let Some(canceled_at) = self.canceled_at {
            return (JOB_STATUS_CANCELED, Some(canceled_at));
        }
        if now < self.completes_at {
            (JOB_STATUS_IN_PROGRESS, None)
        } else {
            (JOB_STATUS_SUCCEEDED, Some(self.completes_at))
        }
    }

    /// Whether the generation is still running at `now`, i.e. whether there is still work
    /// for a cancel to stop.
    pub fn is_in_progress(&self, now: SystemTime) -> bool {
        self.status_at(now).0 == JOB_STATUS_IN_PROGRESS
    }

    /// Project the stored job onto the modelled `JobDetails`, as of `now`.
    pub fn job_details_at(&self, now: SystemTime) -> JobDetails {
        let (status, completed_on) = self.status_at(now);
        JobDetails {
            job_id: self.job_id.clone(),
            status: status.to_string(),
            started_on: ::aws_smithy_types::DateTime::from(self.started_at),
            completed_on: completed_on.map(::aws_smithy_types::DateTime::from),
            // `jobError` is "the job error for the policy generation request" — what a
            // `FAILED` generation reports; nothing failed here, so there is none.
            job_error: None,
        }
    }

    /// Project the stored job onto the modelled `PolicyGeneration`, as a listing reports
    /// it at `now`.
    pub fn summary_at(&self, now: SystemTime) -> PolicyGeneration {
        let (status, completed_on) = self.status_at(now);
        PolicyGeneration {
            job_id: self.job_id.clone(),
            principal_arn: self.principal_arn.clone(),
            status: status.to_string(),
            started_on: ::aws_smithy_types::DateTime::from(self.started_at),
            completed_on: completed_on.map(::aws_smithy_types::DateTime::from),
        }
    }

    /// Whether this job was started within [`POLICY_GENERATION_RETENTION`] of `now`, so a
    /// listing can report the window the operation documents.
    ///
    /// A job the clock reports as started in the FUTURE (the host clock moved backwards
    /// between the start and this read) is as recent as a job can be, so it is in the
    /// window: ageing out is for a job that is genuinely old.
    pub fn is_within_retention(&self, now: SystemTime) -> bool {
        match now.duration_since(self.started_at) {
            Ok(age) => age <= POLICY_GENERATION_RETENTION,
            Err(_) => true,
        }
    }

    /// The result this job reports at `now`: the properties of the generation, and the
    /// policy it produced once it has one.
    pub fn generated_policy_result_at(&self, now: SystemTime) -> GeneratedPolicyResult {
        let (status, _) = self.status_at(now);
        GeneratedPolicyResult {
            properties: GeneratedPolicyProperties {
                // "true if the generated policy contains all possible actions for a
                // service that IAM Access Analyzer identified from the CloudTrail trail
                // that you specified": the document carries every action of every service
                // the analysis identified, so nothing was left out of it.
                is_complete: Some(true),
                principal_arn: self.principal_arn.clone(),
                cloud_trail_properties: Some(CloudTrailProperties {
                    trail_properties: self
                        .trails
                        .iter()
                        .map(|trail| TrailProperties {
                            cloud_trail_arn: trail.cloud_trail_arn.clone(),
                            regions: trail.regions.clone(),
                            all_regions: trail.all_regions,
                        })
                        .collect(),
                    start_time: self.analysis_start_time,
                    end_time: self.analysis_end_time,
                }),
            },
            // A policy exists only once the generation has run to completion: one still
            // running, and one a cancel stopped, produced none.
            generated_policies: (status == JOB_STATUS_SUCCEEDED).then(|| {
                vec![GeneratedPolicy {
                    policy: generated_policy_document(),
                }]
            }),
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
    for policy in proposed_policies(configuration) {
        if !serde_json::from_str::<serde_json::Value>(policy.document)
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
/// key (`ConfigurationsMapKey`, documented as "a map of resource ARNs") takes, and the
/// form the ARN-addressed operations (the tagging surface) require of `resourceArn`.
pub fn is_resource_arn(candidate: &str) -> bool {
    let segments: Vec<&str> = candidate.splitn(6, ':').collect();
    segments.len() == 6 && segments[0] == "arn" && !segments[5].is_empty()
}

/// One policy document a proposed access-preview configuration carries, with where in
/// the configuration it was proposed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProposedPolicy<'a> {
    /// The member path the document was proposed at, spelled the way the wire spells the
    /// members (`s3Bucket.bucketPolicy`, `kmsKey.keyPolicies.default`,
    /// `s3Bucket.accessPoints.<access point ARN>.accessPointPolicy`). It identifies the
    /// document within the configuration, so two documents of one configuration are
    /// never confused for each other.
    pub member: String,
    /// The document itself, exactly as the create carried it.
    pub document: &'a str,
    /// The ARN of the Amazon S3 access point that proposed the document, when an access
    /// point did; `None` for a policy the resource itself carries.
    pub access_point_arn: Option<&'a str>,
}

/// Every policy document a proposed configuration carries, ordered by [`ProposedPolicy::member`]
/// so a reader of one stored configuration always walks them in the same order.
///
/// One arm per `Configuration` union member (from
/// `smithy_query(query="shape", shape="Configuration")`), so a member that proposes no
/// policy — an EBS snapshot's share list, an RDS snapshot's attributes, a KMS grant —
/// contributes nothing and a member that proposes several contributes all of them.
pub fn proposed_policies(configuration: &Configuration) -> Vec<ProposedPolicy<'_>> {
    /// The document one named member carries, when it carries one.
    fn at<'a>(member: &str, document: Option<&'a String>) -> Option<ProposedPolicy<'a>> {
        document.map(|document| ProposedPolicy {
            member: member.to_string(),
            document: document.as_str(),
            access_point_arn: None,
        })
    }
    let mut policies: Vec<ProposedPolicy<'_>> = Vec::new();
    match configuration {
        // Shared through account ids and groups, not through a policy document.
        Configuration::ebsSnapshot(_) => {}
        Configuration::ecrRepository(config) => policies.extend(at(
            "ecrRepository.repositoryPolicy",
            config.repository_policy.as_ref(),
        )),
        Configuration::iamRole(config) => {
            policies.extend(at("iamRole.trustPolicy", config.trust_policy.as_ref()))
        }
        Configuration::efsFileSystem(config) => policies.extend(at(
            "efsFileSystem.fileSystemPolicy",
            config.file_system_policy.as_ref(),
        )),
        Configuration::kmsKey(config) => policies.extend(
            config
                .key_policies
                .iter()
                .flatten()
                .map(|(name, policy)| ProposedPolicy {
                    member: format!("kmsKey.keyPolicies.{name}"),
                    document: policy.as_str(),
                    access_point_arn: None,
                }),
        ),
        // Shared through snapshot attributes, not through a policy document.
        Configuration::rdsDbClusterSnapshot(_) | Configuration::rdsDbSnapshot(_) => {}
        Configuration::secretsManagerSecret(config) => policies.extend(at(
            "secretsManagerSecret.secretPolicy",
            config.secret_policy.as_ref(),
        )),
        Configuration::s3Bucket(config) => {
            policies.extend(at("s3Bucket.bucketPolicy", config.bucket_policy.as_ref()));
            policies.extend(access_point_policies(
                "s3Bucket",
                config.access_points.iter().flatten().map(|(arn, point)| {
                    (arn.as_str(), point.access_point_policy.as_deref())
                }),
            ));
        }
        Configuration::snsTopic(config) => {
            policies.extend(at("snsTopic.topicPolicy", config.topic_policy.as_ref()))
        }
        Configuration::sqsQueue(config) => {
            policies.extend(at("sqsQueue.queuePolicy", config.queue_policy.as_ref()))
        }
        Configuration::s3ExpressDirectoryBucket(config) => {
            policies.extend(at(
                "s3ExpressDirectoryBucket.bucketPolicy",
                config.bucket_policy.as_ref(),
            ));
            policies.extend(access_point_policies(
                "s3ExpressDirectoryBucket",
                config.access_points.iter().flatten().map(|(arn, point)| {
                    (arn.as_str(), point.access_point_policy.as_deref())
                }),
            ));
        }
        Configuration::dynamodbStream(config) => policies.extend(at(
            "dynamodbStream.streamPolicy",
            config.stream_policy.as_ref(),
        )),
        Configuration::dynamodbTable(config) => {
            policies.extend(at("dynamodbTable.tablePolicy", config.table_policy.as_ref()))
        }
    }
    policies.sort_by(|left, right| left.member.cmp(&right.member));
    policies
}

/// The access-point policies of a bucket configuration, keyed by the access point that
/// proposed each one — the one form of proposed policy that is not the resource's own.
///
/// Takes the ARN-and-policy pairs rather than the map itself, because a general-purpose
/// bucket and a directory bucket key their access points by two different (structurally
/// identical) configuration shapes.
fn access_point_policies<'a>(
    member: &str,
    access_points: impl IntoIterator<Item = (&'a str, Option<&'a str>)>,
) -> Vec<ProposedPolicy<'a>> {
    access_points
        .into_iter()
        .filter_map(|(arn, document)| {
            document.map(|document| ProposedPolicy {
                member: format!("{member}.accessPoints.{arn}.accessPointPolicy"),
                document,
                access_point_arn: Some(arn),
            })
        })
        .collect()
}

/// The `ResourceType` a proposed configuration is for.
///
/// One arm per `Configuration` union member, each paired with the entry of the model's
/// `ResourceType` enum ([`crate::ops::types::RESOURCE_TYPE_VALUES`]) that the member's
/// own documentation names: `s3Bucket` is "for an Amazon S3 bucket", so the finding it
/// contributes reports `AWS::S3::Bucket`. The two enum entries no `Configuration` member
/// can propose (`AWS::Lambda::Function`, `AWS::Lambda::LayerVersion`, `AWS::IAM::User`)
/// are therefore unreachable from an access preview.
pub fn configuration_resource_type(configuration: &Configuration) -> &'static str {
    match configuration {
        Configuration::ebsSnapshot(_) => "AWS::EC2::Snapshot",
        Configuration::ecrRepository(_) => "AWS::ECR::Repository",
        Configuration::iamRole(_) => "AWS::IAM::Role",
        Configuration::efsFileSystem(_) => "AWS::EFS::FileSystem",
        Configuration::kmsKey(_) => "AWS::KMS::Key",
        Configuration::rdsDbClusterSnapshot(_) => "AWS::RDS::DBClusterSnapshot",
        Configuration::rdsDbSnapshot(_) => "AWS::RDS::DBSnapshot",
        Configuration::secretsManagerSecret(_) => "AWS::SecretsManager::Secret",
        Configuration::s3Bucket(_) => "AWS::S3::Bucket",
        Configuration::snsTopic(_) => "AWS::SNS::Topic",
        Configuration::sqsQueue(_) => "AWS::SQS::Queue",
        Configuration::s3ExpressDirectoryBucket(_) => "AWS::S3Express::DirectoryBucket",
        Configuration::dynamodbStream(_) => "AWS::DynamoDB::Stream",
        Configuration::dynamodbTable(_) => "AWS::DynamoDB::Table",
    }
}

/// A fresh `accessPreviewId`: a [`new_uuid_v4`] in the lowercase hyphenated form the
/// model's `AccessPreviewId` pattern requires
/// (`^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$`).
pub fn new_access_preview_id() -> String {
    new_uuid_v4()
}

/// A fresh `jobId` for a policy generation.
///
/// `JobId` carries no `@pattern` in the model, and the `jobId`s AWS returns from
/// `StartPolicyGeneration` are UUIDs, so a job is identified the same unguessable,
/// process-unique way an access preview is ([`new_uuid_v4`]).
pub fn new_policy_generation_job_id() -> String {
    new_uuid_v4()
}

/// A freshly minted version-4 UUID in the lowercase hyphenated 8-4-4-4-12 form (RFC 9562
/// §5.4 — version `4` in the high nibble of octet 6, variant `0b10` in the top bits of
/// octet 8), which is the form every identifier this service mints takes.
///
/// Its bits come from three independent sources, so an id is both unguessable and unique
/// within the process: the OS entropy `std` seeds a fresh `RandomState` with on every
/// call, a monotonic counter (so two ids minted in the same nanosecond on different
/// threads differ by construction), and the wall clock in nanoseconds (so ids do not
/// repeat across processes).
pub fn new_uuid_v4() -> String {
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

/// A UUID DERIVED from what it identifies, rather than minted: the same `parts` always
/// render the same id, and two different part lists practically never collide (the id is
/// the first 128 bits of the SHA-256 of the parts, length-prefixed so no two different
/// lists hash the same bytes).
///
/// Used for an identifier the service reports for something it does NOT store a record
/// of — an access preview finding, which is recomputed from the stored proposed
/// configuration on every read. Deriving it is what makes the id STABLE across reads
/// without a stored table of ids, which a minted id could not be.
///
/// Rendered as a version-8 UUID (RFC 9562 §5.8: the version for an id whose bits come
/// from an application-chosen source, which is exactly this), in the same lowercase
/// hyphenated 8-4-4-4-12 form as [`new_access_preview_id`].
pub fn derived_uuid(parts: &[&str]) -> String {
    use std::fmt::Write as _;

    let mut digest = <::sha2::Sha256 as ::sha2::Digest>::new();
    for part in parts {
        // The length prefix is what keeps `["ab", "c"]` and `["a", "bc"]` apart.
        ::sha2::Digest::update(&mut digest, (part.len() as u64).to_le_bytes());
        ::sha2::Digest::update(&mut digest, part.as_bytes());
    }
    let hashed = ::sha2::Digest::finalize(digest);

    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&hashed[..16]);
    // Version 8 in the high nibble of octet 6, variant `0b10` in the top bits of octet 8.
    bytes[6] = (bytes[6] & 0x0f) | 0x80;
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

/// One resource's most recent scan under one analyzer, as `StartResourceScan` records it.
///
/// A scan is not instantaneous, so the record keeps the instant it is DUE to have
/// finished; what a read reports is computed from the clock against that instant by
/// [`Analyzer::last_completed_scan`], so nothing sleeps and no read has to write.
#[derive(Debug, Clone)]
pub struct ResourceScan {
    /// The `resourceArn` the scan was asked for, exactly as the request named it.
    pub resource_arn: String,
    /// The account that owns the resource; see [`resource_owner_account`].
    pub resource_owner_account: String,
    /// When the FIRST scan of this resource under this analyzer was started, kept across
    /// a rescan so the resource's own creation instant does not move when it is rescanned.
    pub first_started_at: SystemTime,
    /// When the most recent scan of this resource was started.
    pub started_at: SystemTime,
    /// When the most recent scan is DUE to have finished: `started_at` plus
    /// [`RESOURCE_SCAN_SPAN`].
    pub completes_at: SystemTime,
}

/// How many decimal digits an Amazon Web Services account ID carries.
///
/// `StartResourceScan$resourceOwnerAccount` is documented as "The Amazon Web Services
/// account ID that owns the resource" and the model leaves it an unconstrained `String`,
/// so what bounds it is the documented TYPE: an account ID is a fixed-width 12-digit
/// decimal number, which is also the form the account segment of every ARN takes.
pub const ACCOUNT_ID_DIGITS: usize = 12;

/// Refuse a `member` documented as an account ID whose value is not one.
///
/// The message is composed from the request because it is a FIELD validation: it names
/// the member and the constraint it violated, which is what a client has to fix.
pub fn validate_account_id(member: &str, value: &str) -> Result<(), ValidationException> {
    if value.len() == ACCOUNT_ID_DIGITS && value.bytes().all(|byte| byte.is_ascii_digit()) {
        return Ok(());
    }
    Err(field_validation_exception(
        member,
        format!(
            "{member} must be a {ACCOUNT_ID_DIGITS}-digit Amazon Web Services account ID, but was {value:?}"
        ),
    ))
}

/// The account that owns `resource_arn`, as a scan of it records the owner.
///
/// `resourceOwnerAccount` is OPTIONAL and documented as "for most Amazon Web Services
/// resources, the owning account is the account in which the resource was created" — the
/// account an ARN names in its own account segment. So the owner is the account the
/// caller named when it named one, else the ARN's account segment, and for the ARN forms
/// that carry no account segment (an S3 bucket ARN, for one) this emulator's account,
/// which is the account every resource it serves belongs to.
pub fn resource_owner_account(resource_arn: &str, named: Option<&str>) -> String {
    if let Some(named) = named.filter(|account| !account.is_empty()) {
        return named.to_string();
    }
    resource_arn
        .split(':')
        .nth(4)
        .filter(|account| !account.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| ::engine::emulator_account_id().to_string())
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

/// Every entry of the model's `ValidationExceptionReason` enum, as the MODEL declares it:
/// the lowerCamel discriminator (`value`) paired with the SCREAMING_SNAKE `name` a
/// response spells that discriminator with.
///
/// This service's model declares the enum in Smithy-1.0 `@enum` form, as pairs:
///
/// ```text
/// {"value":"unknownOperation","name":"UNKNOWN_OPERATION"},
/// {"value":"cannotParse","name":"CANNOT_PARSE"},
/// {"value":"fieldValidationFailed","name":"FIELD_VALIDATION_FAILED"},
/// {"value":"other","name":"OTHER"},
/// {"value":"notSupported","name":"NOT_SUPPORTED"}
/// ```
///
/// and it is the `name` that AWS puts on the wire: the recorded 400s of this service
/// carry `"reason":"FIELD_VALIDATION_FAILED"`, never the lowerCamel `value` that a
/// `smithy_query(query="shape",shape="ValidationExceptionReason")` value list shows (that
/// list, and the generated `VALIDATION_EXCEPTION_REASON_VALUES`, are the `value` half of
/// these pairs). This table is the ONE place the pairing is written down; the consts below
/// are the `name` half, so a refusal names its wire value directly and nothing transforms
/// a discriminator at runtime.
pub const VALIDATION_REASONS: &[(&str, &str)] = &[
    ("unknownOperation", "UNKNOWN_OPERATION"),
    ("cannotParse", "CANNOT_PARSE"),
    ("fieldValidationFailed", "FIELD_VALIDATION_FAILED"),
    ("other", "OTHER"),
    ("notSupported", "NOT_SUPPORTED"),
];

/// The wire value of the `fieldValidationFailed` discriminator — the `name`
/// [`VALIDATION_REASONS`] pairs with it. This is what a refusal of a request member
/// carries, in every operation of this service.
pub const VALIDATION_REASON_FIELD_VALIDATION_FAILED: &str = "FIELD_VALIDATION_FAILED";

/// The wire value of the `cannotParse` discriminator, from the same table: what a refusal
/// of a document that could not be parsed at all carries. Shared, so every policy check
/// that refuses an unparseable document reports the same one.
pub const VALIDATION_REASON_CANNOT_PARSE: &str = "CANNOT_PARSE";

/// The wire value [`VALIDATION_REASONS`] pairs with `discriminator`, or `None` when the
/// model declares no such discriminator.
///
/// A handler names a const instead; this is the lookup the pairing exists for, and what
/// this crate's tests read the table through.
pub fn validation_reason_wire(discriminator: &str) -> Option<&'static str> {
    VALIDATION_REASONS
        .iter()
        .find(|(modelled, _)| *modelled == discriminator)
        .map(|(_, wire)| *wire)
}

/// The `ValidationExceptionReason` for a well-formed request asking for something this
/// resource does not support, from the same `VALIDATION_EXCEPTION_REASON_VALUES` list.
pub const VALIDATION_REASON_NOT_SUPPORTED: &str = "notSupported";

/// A `ValidationException` for a request that omits a member the operation cannot run
/// without, although the model leaves that member optional.
///
/// AWS words this refusal as `Missing` followed by the member's own wire name, so the
/// only part of it that varies is which member was omitted — nothing of the request's
/// values reaches the text.
pub fn missing_member_exception(member: &str) -> ValidationException {
    field_validation_exception(member, format!("Missing {member}"))
}

/// A `ValidationException` about ONE request member: the reason the model lists for a
/// member that failed validation, and a `fieldList` naming it.
///
/// Unlike the resource-scoped refusals above, a validation message says what is wrong
/// with the request, so it is composed from it; what this helper fixes is that every
/// such refusal in this service carries the same reason and the same single-entry
/// `fieldList` shape, built in one place rather than per call site.
pub fn field_validation_exception(member: &str, message: impl Into<String>) -> ValidationException {
    let message = message.into();
    ValidationException {
        message: message.clone(),
        reason: VALIDATION_REASON_FIELD_VALIDATION_FAILED.to_string(),
        field_list: Some(vec![ValidationExceptionField {
            name: member.to_string(),
            message,
        }]),
    }
}

/// The fixed `message` of the `ValidationException` that answers a well-formed request
/// asking for something the resource it names does not support.
///
/// AWS's own constant for this refusal, as the recorded responses carry it: it names
/// neither the operation, nor the resource, nor its type, so it is the same string
/// whatever was asked for and in whatever account or Region.
pub const UNSUPPORTED_OPERATION_MESSAGE: &str = "Unsupported operation";

/// A `ValidationException` for a well-formed request the action does not support against
/// the resource it names — every member parses and satisfies its constraints, but the
/// resource is not one this action acts on.
///
/// Three things make it different from [`field_validation_exception`] and all three come
/// from what AWS answers: the message is the FIXED
/// [`UNSUPPORTED_OPERATION_MESSAGE`] rather than a sentence composed from the request;
/// there is no `fieldList`, because no single member is at fault and so there is nothing
/// for a client to correct; and the reason is still the FIELD-VALIDATION discriminator,
/// because that is what the recorded refusal carries (HTTP 400,
/// `reason = FIELD_VALIDATION_FAILED`) — the modelled `notSupported` discriminator is NOT
/// what this service puts on a refusal that is about the resource a request names.
pub fn unsupported_action_exception() -> ValidationException {
    ValidationException {
        message: UNSUPPORTED_OPERATION_MESSAGE.to_string(),
        reason: VALIDATION_REASON_FIELD_VALIDATION_FAILED.to_string(),
        field_list: None,
    }
}

/// How a policy check's `ReasonSummary$description` names the statement it blames.
///
/// The three policy checks (`CheckAccessNotGranted`, `CheckNoNewAccess`,
/// `CheckNoPublicAccess`) all report a finding as one SHORT canonical sentence followed by
/// a reference to the offending statement of the submitted document — never a narrative
/// about what that statement contains. The reference names the statement's `Sid` when it
/// carries one and its zero-based position in the document when it does not, which are the
/// two forms AWS puts on the wire: the `sid` form in this model's own
/// `smithy.api#examples` for `CheckAccessNotGranted` and `CheckNoPublicAccess` (verify with
/// `grep 'in the statement with sid: ' smithy-model/com-amazonaws-accessanalyzer-accessanalyzer.json`),
/// and the `index` form in the recorded responses for documents whose statements carry no
/// `Sid`.
///
/// Returned without the trailing full stop, so each check appends it to its own sentence:
/// `format!("… statement {reference}.")`.
pub fn statement_reference(statement_id: Option<&str>, statement_index: i32) -> String {
    match statement_id.filter(|sid| !sid.is_empty()) {
        Some(sid) => format!("with sid: {sid}"),
        // A statement with no `Sid` has only its position to be named by, which is the
        // same number the reason reports in `statementIndex`.
        None => format!("with index: {statement_index}"),
    }
}

/// The smallest `unusedAccessAge` an `UnusedAccessConfiguration` may carry.
///
/// The model gives the member no `@range`, but its documentation does bound it — "You
/// can choose a value between 1 and 365 days."
/// (`smithy_query(query="doc",shape="UnusedAccessConfiguration",member="unusedAccessAge")`),
/// so the boundary validator passes a value the service itself refuses. Hence the check
/// in [`validate_configuration`].
pub const UNUSED_ACCESS_AGE_MIN_DAYS: i32 = 1;

/// The largest `unusedAccessAge` an `UnusedAccessConfiguration` may carry; see
/// [`UNUSED_ACCESS_AGE_MIN_DAYS`] for the documented sentence both bounds come from.
pub const UNUSED_ACCESS_AGE_MAX_DAYS: i32 = 365;

/// Refuse an `AnalyzerConfiguration` whose values fall outside what the model documents
/// but does not constrain, as this service's modelled `ValidationException`.
///
/// Shared by every operation that ACCEPTS a configuration (`CreateAnalyzer`,
/// `UpdateAnalyzer`), so one member is bounded in one place rather than per operation.
pub fn validate_configuration(
    configuration: &AnalyzerConfiguration,
) -> Result<(), ValidationException> {
    let AnalyzerConfiguration::unusedAccess(unused_access) = configuration else {
        // `internalAccess` carries no documented numeric bound of its own; its
        // `analysisRule` members are strings the model already constrains.
        return Ok(());
    };
    let Some(days) = unused_access.unused_access_age else {
        return Ok(());
    };
    if (UNUSED_ACCESS_AGE_MIN_DAYS..=UNUSED_ACCESS_AGE_MAX_DAYS).contains(&days) {
        return Ok(());
    }
    let message = format!(
        "unusedAccessAge must be between {UNUSED_ACCESS_AGE_MIN_DAYS} and \
         {UNUSED_ACCESS_AGE_MAX_DAYS} days, but was {days}"
    );
    // The member path a client has to fix, named the way the wire names the members.
    Err(field_validation_exception(
        "configuration.unusedAccess.unusedAccessAge",
        message,
    ))
}

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
    field_validation_exception(name, message)
}

/// Refuse a `resourceArn` member that is not an ARN ([`is_resource_arn`]).
pub fn resource_arn_validation_exception(resource_arn: &str) -> ValidationException {
    field_validation_exception(
        "resourceArn",
        format!("resourceArn must be an ARN, but was {resource_arn}"),
    )
}

/// Report that no resource in this account and Region has the ARN the request named —
/// the refusal every ARN-addressed call on the `/tags/{resourceArn}` routes answers with.
///
/// The CODE is `NotFoundException` (HTTP 404), NOT the `ResourceNotFoundException` the
/// analyzer-, archive-rule- and access-preview-scoped 404s above carry: the tagging path
/// of this service answers its own code, which the model declares for no operation, so it
/// travels through [`crate::scaffolded::Scaffolded::NotFound`] rather than through an
/// operation's modelled error enum. The analyzer lookup behind `GetAnalyzer`,
/// `DeleteAnalyzer` and every name-addressed call keeps [`analyzer_not_found`].
///
/// Takes no ARN: the synthetic shape declares no `resourceId`/`resourceType` to report
/// one in, and the text is fixed, so nothing of the request reaches the refusal. The
/// generic parameter is the calling operation's modelled error type, so all three tagging
/// handlers refuse through this one constructor whichever error enum their operation
/// declares.
pub fn tag_target_not_found<E>() -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::not_found(RESOURCE_NOT_FOUND_MESSAGE)
}

/// Refuse a tag key outside this service's tag restrictions, naming the `member` that
/// carried it: a key is 1 to [`MAX_TAG_KEY_CHARS`] characters.
///
/// Applied to the keys of a tag map AND to the keys named for removal: a key outside
/// the restrictions is not a key this service could ever have stored, so naming one is
/// a malformed request rather than the removal of a tag the resource does not hold.
pub fn validate_tag_key(member: &str, key: &str) -> Result<(), ValidationException> {
    let chars = key.chars().count();
    if chars == 0 {
        return Err(field_validation_exception(
            member,
            format!("{member} must not name an empty tag key"),
        ));
    }
    if chars > MAX_TAG_KEY_CHARS {
        return Err(field_validation_exception(
            member,
            format!(
                "{member} must name a tag key of at most {MAX_TAG_KEY_CHARS} characters, but one was {chars} characters"
            ),
        ));
    }
    Ok(())
}

/// Refuse a tag value outside this service's tag restrictions: at most
/// [`MAX_TAG_VALUE_CHARS`] characters. An empty value is allowed.
pub fn validate_tag_value(member: &str, key: &str, value: &str) -> Result<(), ValidationException> {
    let chars = value.chars().count();
    if chars > MAX_TAG_VALUE_CHARS {
        return Err(field_validation_exception(
            member,
            format!(
                "{member} must carry a value of at most {MAX_TAG_VALUE_CHARS} characters, but the value of tag {key} was {chars} characters"
            ),
        ));
    }
    Ok(())
}

/// Refuse a tagging call that would leave a resource holding more than
/// [`MAX_TAGS_PER_RESOURCE`] tags.
pub fn validate_tag_count(member: &str, resulting: usize) -> Result<(), ValidationException> {
    if resulting > MAX_TAGS_PER_RESOURCE {
        return Err(field_validation_exception(
            member,
            format!(
                "{member} must leave a resource holding at most {MAX_TAGS_PER_RESOURCE} tags, but this call would leave it holding {resulting}"
            ),
        ));
    }
    Ok(())
}

/// How THIS service words a constraint violation it has its own sentence for, or `None`
/// for one it leaves to the validating walk.
///
/// A value outside its member's modelled `@pattern` is refused with Access Analyzer's own
/// short sentence: the fixed `Invalid `, the member's wire name, `: `, and the value the
/// request carried. The pattern itself never reaches the text — it is an internal detail
/// of the model, and the sentence exists to name the parameter a client has to correct.
///
/// Every other kind of violation (a bound, a length, an enumerated set) keeps the clause
/// the walk states, which is also what the request boundary renders for the same
/// violation, so nothing is reworded here without a recorded sentence to word it from.
fn service_violation_message(
    violation: &::aws_smithy_schema_server::constraints::Violation,
) -> Option<String> {
    match (&violation.kind, violation.value.as_deref()) {
        (
            ::aws_smithy_schema_server::constraints::ViolationKind::Pattern(_),
            Some(rejected),
        ) => Some(format!("Invalid {}: {rejected}", violation.path)),
        _ => None,
    }
}

/// One violation as this service states it: its own sentence where it has one, else the
/// walk's clause.
fn violation_message(violation: &::aws_smithy_schema_server::constraints::Violation) -> String {
    service_violation_message(violation).unwrap_or_else(|| violation.message())
}

/// This service's modelled `ValidationException` for the violations a modelled-constraint
/// walk reported.
///
/// For a member whose constraint traits this service enforces ITSELF (so that the refusal
/// carries the modelled `reason`, which the framework's pre-handler envelope cannot), the
/// walk is [`::aws_smithy_schema_server::constraints::validate`] — the same code the
/// boundary runs. `fieldList` names every member that failed, each with the same sentence
/// this service would state for it on its own.
///
/// `message` is the service's own sentence for the first violation it words itself
/// ([`service_violation_message`]); only when it words none of them does the refusal fall
/// back to the walk's multi-violation envelope.
pub fn constraint_validation_exception(
    violations: &[::aws_smithy_schema_server::constraints::Violation],
) -> ValidationException {
    let message = violations
        .iter()
        .find_map(service_violation_message)
        .or_else(|| ::aws_smithy_schema_server::envelope(violations))
        .unwrap_or_else(|| "invalid request".to_string());
    ValidationException {
        message,
        reason: VALIDATION_REASON_FIELD_VALIDATION_FAILED.to_string(),
        field_list: Some(
            violations
                .iter()
                .map(|violation| ValidationExceptionField {
                    name: violation.path.clone(),
                    message: violation_message(violation),
                })
                .collect(),
        ),
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

/// Emit the structured access record for one call to this service.
///
/// Every operation that creates, replaces or refuses something calls this on the branch
/// it returns from, so a reader of the emulator's log can attribute each call — above
/// all the state-changing ones, whose effect on the store is otherwise invisible after
/// the fact. The record carries the operation, the identity the request PRESENTED (the
/// SigV4 access key id, which is an identifier and not a secret), the request target it
/// named, and the outcome; the subscriber supplies the timestamp.
///
/// There is no network peer in the record because a Smithy handler is not given one:
/// `OperationContext::parts()` carries the request's method, URI, headers and
/// extensions, and nothing in this framework puts a peer address in them. The presented
/// credential is therefore what identifies the caller.
///
/// Every interpolated value is request-derived, so each is bounded and stripped of the
/// control characters that would otherwise let one request forge a second log line:
/// [`audit_field`] does both.
///
/// Deliberately metadata only: no policy document, configuration, tag or client token is
/// logged, so a caller's own data never reaches the log.
pub fn audit(
    operation: &str,
    ctx: &smithy_server_runtime::OperationContext<'_>,
    outcome: AuditOutcome,
) {
    let parts = ctx.parts();
    ::tracing::info!(
        target: "emulator::audit",
        service = ctx.identity().signing_name(),
        operation,
        access_key_id = %audit_field(&::engine::extract_akid(&parts.headers)),
        request_target = %audit_field(&format!("{} {}", parts.method, parts.uri.path())),
        outcome = outcome.as_str(),
        "access-analyzer api call"
    );
}

/// Longest request-derived value one audit field carries.
///
/// A SigV4 access key id and a request path are both caller-controlled, so an unbounded
/// echo would let one request emit an arbitrarily long log line.
const MAX_AUDIT_FIELD: usize = 128;

/// A request-derived value as it may appear in an audit record: control characters
/// (above all CR and LF, which would forge a second record) replaced, and the whole
/// thing bounded by [`MAX_AUDIT_FIELD`] on a character boundary.
fn audit_field(value: &str) -> String {
    value
        .chars()
        .map(|c| if c.is_control() { '.' } else { c })
        .take(MAX_AUDIT_FIELD)
        .collect()
}

/// What a call answered, as the one closed set [`audit`] records.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuditOutcome {
    /// An analyzer was created and stored.
    AnalyzerCreated,
    /// A stored analyzer's configuration was replaced.
    AnalyzerUpdated,
    /// Tags were merged into a resource's tag set.
    TagsAdded,
    /// Tags were removed from a resource's tag set.
    TagsRemoved,
    /// An `@idempotent` call changed nothing and replayed what it already held.
    NoChange,
    /// A policy check ran and answered a result.
    CheckEvaluated,
    /// A scan of one resource's policies was started against an analyzer.
    ResourceScanStarted,
    /// A policy generation job was started and recorded.
    PolicyGenerationStarted,
    /// A running policy generation job was cancelled.
    PolicyGenerationCanceled,
    /// The request was refused as invalid (400).
    ValidationRefused,
    /// The caller may not perform the operation (403).
    AccessDenied,
    /// The named resource does not exist (404).
    NotFound,
    /// The request conflicts with a resource that already exists (409).
    Conflict,
    /// A quota — the service's or this emulator's own ceiling — was met (402).
    QuotaExceeded,
    /// The service's own store was unreachable (500).
    StoreUnavailable,
    /// The work could not be run to completion (500).
    EvaluationFailed,
}

impl AuditOutcome {
    /// The stable token this outcome is logged as.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::AnalyzerCreated => "analyzer-created",
            Self::AnalyzerUpdated => "analyzer-updated",
            Self::TagsAdded => "tags-added",
            Self::TagsRemoved => "tags-removed",
            Self::NoChange => "no-change",
            Self::CheckEvaluated => "check-evaluated",
            Self::ResourceScanStarted => "resource-scan-started",
            Self::PolicyGenerationStarted => "policy-generation-started",
            Self::PolicyGenerationCanceled => "policy-generation-canceled",
            Self::ValidationRefused => "validation-refused",
            Self::AccessDenied => "access-denied",
            Self::NotFound => "not-found",
            Self::Conflict => "conflict",
            Self::QuotaExceeded => "quota-exceeded",
            Self::StoreUnavailable => "store-unavailable",
            Self::EvaluationFailed => "evaluation-failed",
        }
    }
}

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
