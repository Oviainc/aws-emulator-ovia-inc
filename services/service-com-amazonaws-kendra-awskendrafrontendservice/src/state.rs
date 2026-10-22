// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-kendra-awskendrafrontendservice service.
//!
//! This holds the Amazon Kendra index-and-search loop: the indices `CreateIndex`
//! minted, the documents `BatchPutDocument` persisted into them, and the query
//! records `Query` minted a `QueryId` for. `DescribeIndex`, `ListIndices` and
//! `Query` compute every value they answer with from what is stored here.
//!
//! Two clock-owed transitions live here as well, because an index Kendra hands
//! back is not immediately usable: `CreateIndex` is documented as asynchronous
//! ("check the `Status` field returned from a call to `DescribeIndex`. The
//! `Status` field is set to `ACTIVE` when the index is ready to use"), and
//! `DeleteIndex` likewise moves an index to `DELETING` before it disappears.
//! Both are materialised by [`ComAmazonawsKendraAwskendrafrontendserviceState::materialize_due_transitions`],
//! which every handler calls first: the due instant is stored, never slept on,
//! so a reporting operation only ever writes a change the clock already owed.

use std::collections::{HashMap, VecDeque};
use std::time::{Duration, SystemTime};

use crate::ops::types::{
    DocumentAttribute, Principal, S3Path, ServerSideEncryptionConfiguration, Tag,
    UserGroupResolutionConfiguration, UserTokenConfiguration,
};

/// A process-wide mint counter, so two ids minted in the same nanosecond still differ.
static MINT_SEQUENCE: ::std::sync::atomic::AtomicU64 = ::std::sync::atomic::AtomicU64::new(0);

/// A 36-character identifier shaped like a version-4 UUID: `IndexId` is modelled
/// len 36..36 with pattern `^[a-zA-Z0-9][a-zA-Z0-9-]*$`, and `QueryId` len 1..36.
///
/// The bits come from the standard library's
/// [`std::collections::hash_map::RandomState`], whose SipHash keys the host seeds from
/// the operating system when it is constructed, mixed with the wall clock and a
/// process-wide counter so two draws in the same instant cannot coincide. The caller
/// re-mints on a collision with an id already stored, so uniqueness is guaranteed by
/// the store rather than by the draw.
pub fn mint_uuid() -> String {
    use ::std::hash::BuildHasher;

    let sequence = MINT_SEQUENCE.fetch_add(1, ::std::sync::atomic::Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(::std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as u64;
    let high = ::std::collections::hash_map::RandomState::new().hash_one((sequence, nanos));
    let low = ::std::collections::hash_map::RandomState::new().hash_one((nanos, !sequence));
    // The literal `4` is the UUID version nibble and `0x8000 |` sets the variant bits,
    // so the id is shaped exactly like the version-4 UUIDs this service's ids are.
    format!(
        "{:08x}-{:04x}-4{:03x}-{:04x}-{:012x}",
        high >> 32,
        (high >> 16) & 0xffff,
        high & 0x0fff,
        0x8000 | ((low >> 48) & 0x3fff),
        low & 0x0000_ffff_ffff_ffff
    )
}

/// `IndexStatus` values this emulator uses (`smithy_query(query="shape",
/// shape="IndexStatus")` lists CREATING, ACTIVE, DELETING, FAILED, UPDATING,
/// SYSTEM_UPDATING).
pub const STATUS_CREATING: &str = "CREATING";
/// The terminal, ready status of an index.
pub const STATUS_ACTIVE: &str = "ACTIVE";
/// An index that `DeleteIndex` accepted and that has not disappeared yet.
pub const STATUS_DELETING: &str = "DELETING";

/// The default `Edition` of an index created without one.
///
/// From the model's own doc for `CreateIndex$Edition`: "The `Edition` parameter
/// is optional. If you don't supply a value, the default is
/// `ENTERPRISE_EDITION`."
pub const DEFAULT_INDEX_EDITION: &str = "ENTERPRISE_EDITION";

/// How long after `CreateIndex` an index becomes `ACTIVE`.
///
/// A CHOSEN span: the real service takes minutes, which no caller of an emulator
/// would wait for, so this sits inside the emulator's 250–750 ms window for a
/// transition the clock owes. It is never zero, so a caller that acts straight
/// after `CreateIndex` still observes `CREATING` and the state errors that go
/// with it.
pub const INDEX_ACTIVATION_DELAY: Duration = Duration::from_millis(500);

/// How long after `DeleteIndex` an index stops existing. A CHOSEN span, for the
/// same reason as [`INDEX_ACTIVATION_DELAY`].
pub const INDEX_DELETION_DELAY: Duration = Duration::from_millis(500);

/// How many `Query` records are retained, so a long-lived emulator's query log
/// cannot grow without bound. The oldest record is evicted first.
pub const MAX_RETAINED_QUERIES: usize = 1024;

/// Documents a DEVELOPER_EDITION index holds, from the published Amazon Kendra
/// quotas ("Developer edition … 10,000 documents").
pub const MAX_DOCUMENTS_DEVELOPER_EDITION: usize = 10_000;

/// Documents an Enterprise-class index holds, from the published Amazon Kendra
/// quotas ("Enterprise edition … 100,000 documents" per storage capacity unit;
/// this emulator provisions no extra capacity units).
pub const MAX_DOCUMENTS_ENTERPRISE_EDITION: usize = 100_000;

/// Indices this emulator holds for the account it serves.
///
/// Amazon Kendra publishes a small, adjustable indices-per-account quota and
/// refuses `CreateIndex` past it with `ServiceQuotaExceededException`. This
/// emulator serves one account and keeps everything in memory, so it sets its own
/// (more generous) ceiling and refuses past it with that same modelled error,
/// rather than letting one caller grow the store without end.
pub const MAX_INDICES: usize = 100;

/// Indexed text one index holds, in bytes.
///
/// Amazon Kendra caps the extracted text per index as well as the document count
/// (3 GB on Developer Edition, more on Enterprise). This emulator holds that text
/// in memory, so it enforces an emulator-scale ceiling in the same place and with
/// the same modelled `ServiceQuotaExceededException`: the document-count quota
/// alone would let a caller store documents up to the request-body cap each.
pub const MAX_INDEXED_TEXT_BYTES: usize = 64 * 1024 * 1024;

/// The documents-per-index quota for an `IndexEdition`.
///
/// Enforced by `BatchPutDocument` with the modelled `ServiceQuotaExceededException`,
/// which also bounds the per-query work in [`Index::search`].
pub fn document_quota(edition: &str) -> usize {
    if edition == "DEVELOPER_EDITION" {
        MAX_DOCUMENTS_DEVELOPER_EDITION
    } else {
        MAX_DOCUMENTS_ENTERPRISE_EDITION
    }
}

/// A document stored in an index by `BatchPutDocument`, keyed by `Document.Id`.
#[derive(Debug, Clone)]
pub struct StoredDocument {
    /// `Document$Id` — unique per index, so a later put with the same id
    /// replaces this document.
    pub id: String,
    pub title: Option<String>,
    /// The document's text: `Document$Blob` decoded as UTF-8.
    pub body_text: String,
    pub content_type: Option<String>,
    pub attributes: Vec<DocumentAttribute>,
    pub access_control_list: Vec<Principal>,
    pub s3_path: Option<S3Path>,
    pub indexed_at: SystemTime,
    /// How often each term occurs in the title, counted ONCE at ingest so a query
    /// costs one lookup per query term rather than a scan of the document.
    pub title_term_counts: HashMap<String, u32>,
    /// How often each term occurs in the body, counted once at ingest.
    pub body_term_counts: HashMap<String, u32>,
}

impl StoredDocument {
    /// The bytes of this document's indexed text, as `IndexStatistics` counts them.
    pub fn indexed_text_bytes(&self) -> usize {
        self.body_text.len()
    }
}

/// One Kendra index, as `CreateIndex` minted it.
#[derive(Debug, Clone)]
pub struct Index {
    /// `IndexId`: 36 characters, `^[a-zA-Z0-9][a-zA-Z0-9-]*$`.
    pub id: String,
    pub name: String,
    pub edition: String,
    pub role_arn: String,
    pub description: Option<String>,
    /// An `IndexStatus` value: `CREATING`, `ACTIVE` or `DELETING` here.
    pub status: String,
    pub created_at: SystemTime,
    pub updated_at: SystemTime,
    pub user_context_policy: Option<String>,
    pub tags: Vec<Tag>,
    pub server_side_encryption_configuration: Option<ServerSideEncryptionConfiguration>,
    pub user_token_configurations: Option<Vec<UserTokenConfiguration>>,
    pub user_group_resolution_configuration: Option<UserGroupResolutionConfiguration>,
    /// The `ClientToken` of the `CreateIndex` call that minted this index, so a
    /// retry with the same token answers with this same `Id`.
    pub client_token: Option<String>,
    pub documents: HashMap<String, StoredDocument>,
    /// Indexed question-answer pairs (`CreateFaq` is not implemented yet, so no
    /// call can raise this above zero).
    pub faq_count: i32,
    /// When `CREATING` becomes `ACTIVE`; `None` once the transition is done.
    pub active_at: Option<SystemTime>,
    /// When a `DELETING` index stops existing; `None` while it is not deleting.
    pub removed_at: Option<SystemTime>,
}

impl Index {
    /// `TextDocumentStatistics.IndexedTextDocumentsCount` — the documents
    /// actually stored in this index.
    pub fn indexed_text_documents_count(&self) -> i32 {
        i32::try_from(self.documents.len()).unwrap_or(i32::MAX)
    }

    /// `TextDocumentStatistics.IndexedTextBytes` — the size of the indexed text.
    pub fn indexed_text_bytes(&self) -> i64 {
        i64::try_from(self.stored_text_bytes()).unwrap_or(i64::MAX)
    }

    /// The indexed text this index holds, in bytes — what the emulator's
    /// indexed-text ceiling is measured against.
    pub fn stored_text_bytes(&self) -> usize {
        self.documents
            .values()
            .map(crate::state::StoredDocument::indexed_text_bytes)
            .sum()
    }

    /// Documents matching `terms`, most relevant first.
    ///
    /// A document matches when at least one query term is one of its own terms
    /// (title or body, case-insensitively). The score weights a title match
    /// above a body match, and ties break on `Document.Id` so a repeated query
    /// returns a stable order.
    ///
    /// The cost is BOUNDED on both factors: one hash lookup per (document, query
    /// term) pair, with the documents in an index capped by
    /// [`document_quota`] and the query terms capped by the caller-side limit on
    /// `QueryText` that `Query` enforces before calling this.
    pub fn search(&self, terms: &[String]) -> Vec<DocumentMatch> {
        let mut matches: Vec<DocumentMatch> = self
            .documents
            .values()
            .filter_map(|doc| DocumentMatch::of(doc, terms))
            .collect();
        matches.sort_by(|a, b| {
            b.score
                .cmp(&a.score)
                .then_with(|| a.document_id.cmp(&b.document_id))
        });
        matches
    }
}

/// How well one stored document matched a query.
#[derive(Debug, Clone)]
pub struct DocumentMatch {
    pub document_id: String,
    /// `TITLE_TERM_WEIGHT` per title hit plus one per body hit.
    pub score: usize,
    /// How many DISTINCT query terms this document carries.
    pub matched_terms: usize,
}

/// A title hit counts for this many body hits when scoring a match: a query term
/// in the title is the stronger signal of relevance.
const TITLE_TERM_WEIGHT: usize = 3;

impl DocumentMatch {
    /// Score `doc` against `terms`, or `None` when it carries none of them.
    ///
    /// One hash lookup per query term, against the term counts built when the
    /// document was stored — so this never re-reads the document's text.
    fn of(doc: &StoredDocument, terms: &[String]) -> Option<Self> {
        let mut score = 0usize;
        let mut matched_terms = 0usize;
        for term in terms {
            let title_hits = *doc.title_term_counts.get(term).unwrap_or(&0) as usize;
            let body_hits = *doc.body_term_counts.get(term).unwrap_or(&0) as usize;
            if title_hits + body_hits > 0 {
                matched_terms += 1;
            }
            score += title_hits * TITLE_TERM_WEIGHT + body_hits;
        }

        (score > 0).then(|| Self {
            document_id: doc.id.clone(),
            score,
            matched_terms,
        })
    }
}

/// The terms of `text`: maximal runs of alphanumeric characters, lowercased.
///
/// This is the whole analyzer: the emulator matches terms, so a query term is
/// found when the text carries that exact word, case-insensitively.
pub fn terms_of(text: &str) -> Vec<String> {
    let mut terms = Vec::new();
    tokenize(text, |_, _, term| terms.push(term));
    terms
}

/// How often each term occurs in `text`.
pub fn term_counts(text: &str) -> HashMap<String, u32> {
    let mut counts: HashMap<String, u32> = HashMap::new();
    tokenize(text, |_, _, term| {
        *counts.entry(term).or_insert(0) += 1;
    });
    counts
}

/// One term of a text, with where it sits in that text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WordSpan {
    /// Zero-based CHARACTER offset of the first character of the term.
    pub begin: usize,
    /// Zero-based CHARACTER offset one past the last character of the term.
    pub end: usize,
    /// The term, lowercased.
    pub term: String,
}

/// Every term of `text` with its character offsets, in order.
///
/// Offsets are character offsets because `Highlight$BeginOffset` is documented
/// as "the zero-based location in the response string", and the response string
/// is handed to the client as text, not as bytes.
pub fn word_spans(text: &str) -> Vec<WordSpan> {
    let mut spans = Vec::new();
    tokenize(text, |begin, end, term| {
        spans.push(WordSpan { begin, end, term });
    });
    spans
}

/// The one tokenizer every reader above shares: hand each term, with its
/// character offsets, to `sink`.
fn tokenize(text: &str, mut sink: impl FnMut(usize, usize, String)) {
    let mut current = String::new();
    let mut begin = 0usize;
    let mut char_count = 0usize;
    for (char_index, ch) in text.chars().enumerate() {
        char_count = char_index + 1;
        if ch.is_alphanumeric() {
            if current.is_empty() {
                begin = char_index;
            }
            current.extend(ch.to_lowercase());
        } else if !current.is_empty() {
            sink(begin, char_index, std::mem::take(&mut current));
        }
    }
    if !current.is_empty() {
        sink(begin, char_count, current);
    }
}

/// A `Query` that ran: what it searched for and which documents it answered
/// with, so the `QueryId` it minted can be resolved later (`SubmitFeedback`).
#[derive(Debug, Clone)]
pub struct QueryRecord {
    pub query_id: String,
    pub index_id: String,
    pub query_text: Option<String>,
    pub result_document_ids: Vec<String>,
    pub created_at: SystemTime,
}

/// Per-service state for com-amazonaws-kendra-awskendrafrontendservice. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsKendraAwskendrafrontendserviceState {
    /// Indices by `IndexId`.
    pub indices: HashMap<String, Index>,
    /// `Name` -> `IndexId`, so a duplicate index name can be refused.
    pub index_ids_by_name: HashMap<String, String>,
    /// `CreateIndex` `ClientToken` -> `IndexId`, for an idempotent retry.
    pub index_ids_by_client_token: HashMap<String, String>,
    /// Query records by `QueryId`.
    pub queries: HashMap<String, QueryRecord>,
    /// `QueryId`s oldest first, so the query log can be bounded.
    pub query_ids_by_age: VecDeque<String>,
}

impl ComAmazonawsKendraAwskendrafrontendserviceState {
    /// Apply every state change the clock has already made due by `now`:
    /// `CREATING` indices whose activation instant has passed become `ACTIVE`,
    /// and `DELETING` indices whose removal instant has passed stop existing.
    ///
    /// Each change was owed at the same instant whether or not anybody called,
    /// which is why a reporting operation (`DescribeIndex`, `ListIndices`) may
    /// run it: it materialises the clock's work, never the caller's.
    pub fn materialize_due_transitions(&mut self, now: SystemTime) {
        for index in self.indices.values_mut() {
            if let Some(active_at) = index.active_at {
                if active_at <= now {
                    index.status = STATUS_ACTIVE.to_string();
                    // The index changed at the instant it became active, not now.
                    index.updated_at = active_at;
                    index.active_at = None;
                }
            }
        }

        let removed: Vec<String> = self
            .indices
            .values()
            .filter(|index| index.removed_at.is_some_and(|at| at <= now))
            .map(|index| index.id.clone())
            .collect();
        for index_id in removed {
            self.remove_index(&index_id);
        }
    }

    /// Drop an index and everything that pointed at it (its documents go with
    /// it, since they are owned by the index).
    pub fn remove_index(&mut self, index_id: &str) {
        if let Some(index) = self.indices.remove(index_id) {
            self.index_ids_by_name.remove(&index.name);
            if let Some(token) = index.client_token {
                self.index_ids_by_client_token.remove(&token);
            }
        }
    }

    /// The index `index_id` names, or `None` when no such index exists.
    pub fn index(&self, index_id: &str) -> Option<&Index> {
        self.indices.get(index_id)
    }

    /// The index `index_id` names, mutably.
    pub fn index_mut(&mut self, index_id: &str) -> Option<&mut Index> {
        self.indices.get_mut(index_id)
    }

    /// Record a query, evicting the oldest once [`MAX_RETAINED_QUERIES`] are held.
    pub fn record_query(&mut self, record: QueryRecord) {
        self.query_ids_by_age.push_back(record.query_id.clone());
        self.queries.insert(record.query_id.clone(), record);
        while self.query_ids_by_age.len() > MAX_RETAINED_QUERIES {
            if let Some(oldest) = self.query_ids_by_age.pop_front() {
                self.queries.remove(&oldest);
            }
        }
    }
}

/// The envelope a `NextToken` this service issues carries, so a token it did not
/// issue — or one issued by a DIFFERENT paginated operation — is recognisable rather
/// than silently resumed from. The operation name travels inside it for that reason.
const PAGE_TOKEN_PREFIX: &str = "kendra:v1:";

/// A pagination token for `operation` that resumes at the resource key `resume_at`.
///
/// A `NextToken` is opaque to the caller (the model constrains only its length), so
/// the key travels inside an envelope rather than as itself. Every paginated
/// operation in this crate shares this pair.
pub fn encode_page_token(operation: &str, resume_at: &str) -> String {
    ::aws_smithy_types::base64::encode(format!("{PAGE_TOKEN_PREFIX}{operation}:{resume_at}"))
}

/// The resource key a `NextToken` carries, or `None` when the token is not one
/// `operation` issued — which the caller answers with its modelled validation error.
pub fn decode_page_token(operation: &str, token: &str) -> Option<String> {
    let decoded = String::from_utf8(::aws_smithy_types::base64::decode(token).ok()?).ok()?;
    decoded
        .strip_prefix(&format!("{PAGE_TOKEN_PREFIX}{operation}:"))
        .map(ToString::to_string)
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsKendraAwskendrafrontendserviceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsKendraAwskendrafrontendserviceState>)
}
