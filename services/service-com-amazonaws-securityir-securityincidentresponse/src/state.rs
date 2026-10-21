// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-securityir-securityincidentresponse service.
//!
//! The service's primary resource is the `case`. A case is stored once, keyed by its
//! `caseId`, and every operation — `CreateCase`, `GetCase`, `ListCases`,
//! `CreateCaseComment`, `ListComments`, `CloseCase` — reads and writes THIS struct, so a
//! write made by one request is what the next request reads back.

use std::collections::{BTreeMap, HashMap, VecDeque};

use crate::ops::types::{ImpactedAwsRegion, ThreatActorIp, Watcher};

/// The `CaseStatus` a freshly created case carries. One of
/// [`crate::ops::types::CASE_STATUS_VALUES`].
pub const CASE_STATUS_SUBMITTED: &str = "Submitted";

/// The `CaseStatus` a closed case carries. One of
/// [`crate::ops::types::CASE_STATUS_VALUES`].
pub const CASE_STATUS_CLOSED: &str = "Closed";

/// The `PendingAction` of a case that is not waiting on customer input. One of
/// [`crate::ops::types::PENDING_ACTION_VALUES`].
pub const PENDING_ACTION_NONE: &str = "None";

/// The page size a request that omits `maxResults` gets: the `@default` both
/// `ListCasesRequest$maxResults` and `ListCommentsRequest$maxResults` declare.
pub const DEFAULT_MAX_RESULTS: i32 = 25;

/// The largest page either paginated operation will build, whatever `maxResults` asks
/// for. This ceiling is the bound: it is applied in the handler ([`page_keys`] and each
/// operation's own `page_size`), so no number a request carries can size a response or
/// the allocation that builds it, whatever any other layer does or does not refuse.
/// (The emulator's boundary validator does in fact refuse `maxResults` outside 1..=25
/// before a handler runs — pinned by the integration suite's
/// `a_max_results_outside_its_modelled_range_is_refused` — but this ceiling does not
/// depend on that.)
pub const MAX_PAGE_SIZE: i32 = 25;

/// The most cases this store holds. Bounds the memory one emulator process can be made
/// to hold through repeated `CreateCase` calls: at `CaseDescription`'s modelled maximum
/// of 8,000 characters plus a 300-character title, a full store is a few megabytes.
/// `CreateCase` refuses past it rather than growing without limit.
pub const MAX_STORED_CASES: usize = 1_000;

/// The most comments one case holds. Bounds per-case memory the same way (`CommentBody`
/// is modelled up to 12,000 characters) and stays well inside the `^\d{6}$` id space, so
/// [`Case::allocate_comment_id`] can never be asked for a seventh digit.
pub const MAX_COMMENTS_PER_CASE: usize = 1_000;

/// The most idempotency replays remembered at once. Each distinct caller-supplied
/// `clientToken` adds one entry, so the map is bounded and the oldest entries are
/// evicted first ([`ComAmazonawsSecurityirSecurityincidentresponseState::record_idempotency`]).
pub const MAX_IDEMPOTENCY_ENTRIES: usize = 10_000;

/// A stored comment on a case.
#[derive(Debug, Clone)]
pub struct CaseComment {
    /// Six digits, matching `CommentId`'s `^\d{6}$`.
    pub comment_id: String,
    pub body: String,
    pub creator: String,
    pub last_updated_by: String,
    pub created_date: ::aws_smithy_types::DateTime,
    pub last_updated_date: ::aws_smithy_types::DateTime,
}

/// A stored case: every member `CreateCase` accepted, plus the server-assigned
/// identity, status and timestamps the read operations project.
#[derive(Debug, Clone)]
pub struct Case {
    /// Ten digits, matching `CaseId`'s `^\d{10,32}` and `CaseArn`'s `case/[0-9]{10}`.
    pub case_id: String,
    pub case_arn: String,
    pub title: String,
    pub description: String,
    /// A `CaseStatus` value.
    pub case_status: String,
    /// An `EngagementType` value.
    pub engagement_type: String,
    /// A `ResolverType` value.
    pub resolver_type: String,
    pub reported_incident_start_date: ::aws_smithy_types::DateTime,
    pub actual_incident_start_date: Option<::aws_smithy_types::DateTime>,
    pub impacted_accounts: Vec<String>,
    pub impacted_aws_regions: Vec<ImpactedAwsRegion>,
    pub impacted_services: Vec<String>,
    pub threat_actor_ip_addresses: Vec<ThreatActorIp>,
    pub watchers: Vec<Watcher>,
    /// A `PendingAction` value.
    pub pending_action: Option<String>,
    /// A `ClosureCode` value. Stays unset until something sets it: neither
    /// `CloseCase` nor `CreateCase` takes one.
    pub closure_code: Option<String>,
    pub created_date: ::aws_smithy_types::DateTime,
    pub last_updated_date: ::aws_smithy_types::DateTime,
    pub closed_date: Option<::aws_smithy_types::DateTime>,
    pub tags: BTreeMap<String, String>,
    /// Comments keyed by `commentId`, so `ListComments` pages deterministically.
    pub comments: BTreeMap<String, CaseComment>,
    /// Monotonic source of this case's `commentId`s.
    pub next_comment_id: u32,
}

impl Case {
    /// Whether this case can hold another comment, i.e. is below
    /// [`MAX_COMMENTS_PER_CASE`].
    pub fn accepts_another_comment(&self) -> bool {
        self.comments.len() < MAX_COMMENTS_PER_CASE
    }

    /// Allocate the next `commentId` for this case: six digits, matching
    /// `CommentId`'s `^\d{6}$`.
    ///
    /// `None` once the case is at [`MAX_COMMENTS_PER_CASE`], which is what keeps the
    /// counter inside the six-digit space the pattern allows: the caller refuses the
    /// request instead of minting a wider id or overwriting an existing comment.
    pub fn allocate_comment_id(&mut self) -> Option<String> {
        if !self.accepts_another_comment() {
            return None;
        }
        self.next_comment_id = self.next_comment_id.saturating_add(1);
        Some(format!("{:06}", self.next_comment_id))
    }
}

/// Per-service state for com-amazonaws-securityir-securityincidentresponse. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsSecurityirSecurityincidentresponseState {
    /// Every stored case, keyed by `caseId`. A `BTreeMap` so `ListCases` enumerates in
    /// a stable order and its `nextToken` resumes exactly where a page ended.
    pub cases: BTreeMap<String, Case>,
    /// Monotonic source of `caseId`s, rendered zero-padded to ten digits.
    pub next_case_id: u64,
    /// `clientToken` -> the id minted for it, so replaying an `@idempotencyToken`
    /// returns the first id instead of minting a second resource. Keys are namespaced
    /// by the minting operation (see [`idempotency_key`]) and the map is bounded by
    /// [`MAX_IDEMPOTENCY_ENTRIES`].
    pub idempotency: HashMap<String, String>,
    /// The order [`idempotency`](Self::idempotency)'s keys were recorded in, so the
    /// oldest is the one evicted when the map is full.
    pub idempotency_order: VecDeque<String>,
}

impl ComAmazonawsSecurityirSecurityincidentresponseState {
    /// Allocate the next `caseId`: ten digits, matching `CaseId`'s `^\d{10,32}` and the
    /// `case/[0-9]{10}` tail of `CaseArn`.
    pub fn allocate_case_id(&mut self) -> String {
        self.next_case_id = self.next_case_id.saturating_add(1);
        format!("{:010}", self.next_case_id)
    }

    /// Whether this store can hold another case, i.e. is below [`MAX_STORED_CASES`].
    pub fn accepts_another_case(&self) -> bool {
        self.cases.len() < MAX_STORED_CASES
    }

    /// Remember that `client_token`'s key minted `id`, evicting the oldest remembered
    /// token once [`MAX_IDEMPOTENCY_ENTRIES`] are held — so a caller cannot grow this
    /// map without limit by sending a fresh token every request.
    pub fn record_idempotency(&mut self, key: String, id: &str) {
        if self.idempotency.insert(key.clone(), id.to_string()).is_none() {
            self.idempotency_order.push_back(key);
        }
        while self.idempotency_order.len() > MAX_IDEMPOTENCY_ENTRIES {
            if let Some(oldest) = self.idempotency_order.pop_front() {
                self.idempotency.remove(&oldest);
            }
        }
    }
}

/// The idempotency-map key for `client_token` as used by `operation` on `scope`.
///
/// Namespaced by operation (and, for a per-case mint, by case) because two different
/// operations may legitimately be called with the same caller-supplied token.
pub fn idempotency_key(operation: &str, scope: &str, client_token: &str) -> String {
    format!("{operation}\u{1f}{scope}\u{1f}{client_token}")
}

/// The current wall-clock instant, as the modelled `Timestamp` type.
pub fn now() -> ::aws_smithy_types::DateTime {
    ::aws_smithy_types::DateTime::from(::std::time::SystemTime::now())
}

/// How many items a page may carry: the request's `maxResults`, or
/// [`DEFAULT_MAX_RESULTS`] when the request did not carry one — never more than
/// [`MAX_PAGE_SIZE`], whatever the request asks for. Shared by both paginated
/// operations, so the ceiling lives in exactly one place.
pub fn page_size(max_results: i32) -> usize {
    if max_results <= 0 {
        DEFAULT_MAX_RESULTS as usize
    } else {
        max_results.min(MAX_PAGE_SIZE) as usize
    }
}

/// One page of keys from a key-ordered map, plus the token that resumes after it.
///
/// Honours the `@paginated` contract shared by `ListCases` and `ListComments`: at most
/// `max_results` keys, starting at `start_at` when a token was supplied, and a
/// `next_token` that is the first key of the FOLLOWING page — absent on the last page.
///
/// `max_results` is capped at [`MAX_PAGE_SIZE`] here as well as by each caller, so no
/// number a request carries can size the allocation this makes.
pub fn page_keys<V>(
    map: &BTreeMap<String, V>,
    start_at: Option<&str>,
    max_results: usize,
) -> (Vec<String>, Option<String>) {
    let max_results = max_results.min(MAX_PAGE_SIZE as usize);
    let mut keys: Vec<String> = match start_at {
        Some(start) => map
            .range(start.to_owned()..)
            .map(|(key, _)| key.clone())
            .take(max_results + 1)
            .collect(),
        None => map.keys().take(max_results + 1).cloned().collect(),
    };
    let next_token = (keys.len() > max_results).then(|| keys[max_results].clone());
    keys.truncate(max_results);
    (keys, next_token)
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsSecurityirSecurityincidentresponseState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsSecurityirSecurityincidentresponseState>)
}
