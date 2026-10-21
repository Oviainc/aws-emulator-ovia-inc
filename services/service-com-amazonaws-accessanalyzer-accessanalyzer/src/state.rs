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

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::time::SystemTime;

use crate::ops::types::{
    AnalyzerConfiguration, AnalyzerSummary, ArchiveRuleSummary, Criterion, InternalServerException,
    StatusReason, ValidationException, ValidationExceptionField,
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
