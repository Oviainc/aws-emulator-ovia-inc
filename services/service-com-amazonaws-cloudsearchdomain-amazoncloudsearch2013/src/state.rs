// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-cloudsearchdomain-amazoncloudsearch2013 service.
//!
//! A CloudSearch Domain endpoint serves ONE search domain's documents, so the state
//! is a map of domain key -> [`DomainIndex`]. `UploadDocuments` writes the index;
//! `Search` and `Suggest` only read it.
//!
//! The two submodules hold the data-plane behaviour the operations share:
//!
//! * [`sdf`] — parsing a Search Data Format document batch (JSON or XML).
//! * [`query`] — the query parsers, matching, sorting, faceting, statistics and
//!   highlighting `Search`/`Suggest` compute from the index.

pub mod query;
pub mod sdf;

use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

/// The index key used when the request's host names no CloudSearch domain (a
/// loopback or IP-literal endpoint, as in-process tests and the local emulator
/// use).
///
/// Begins with `/`, which no host label can contain, so this key cannot collide
/// with a key derived from a host: a real search domain NAMED `default` keys
/// `default` and keeps its own index, separate from the loopback one.
pub const DEFAULT_DOMAIN_KEY: &str = "/default";

/// The most documents one domain's index holds.
///
/// CloudSearch bounds a domain by data size rather than by a published document
/// count, so this is the emulator's own memory ceiling: the store lives in this
/// process, and `UploadDocuments` is unauthenticated from the emulator's point of
/// view, so a batch that would push an index past this many documents is refused
/// with the modelled `DocumentServiceException` instead of growing the process
/// without bound.
pub const MAX_DOCUMENTS_PER_DOMAIN: usize = 10_000;

/// The most domain indexes this endpoint holds at once.
///
/// An index is created for each distinct domain endpoint a batch addresses, and
/// the endpoint comes from the request, so the number of indexes is bounded as
/// well. The value is CloudSearch's own account limit ("Maximum number of search
/// domains: 100", Limits, Amazon CloudSearch Developer Guide), so no domain a
/// real account can own is refused by this bound.
pub const MAX_DOMAINS: usize = 100;

/// The page size CloudSearch applies when a request leaves `size` at its modelled
/// default of 0: the Search and Suggest APIs both document "Default: 10", and the
/// modelled default cannot be told apart from an absent parameter on the wire.
pub const DEFAULT_PAGE_SIZE: usize = 10;

/// A document stored in a domain's index, exactly as an SDF `add` entry described
/// it: an id plus the field values uploaded under it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Document {
    /// The document id the batch gave (`Hit::id`, and the `_id` sort field).
    pub id: String,
    /// Field name -> its one or more values. Ordered so a response renders the
    /// same fields in the same order for the same document.
    pub fields: BTreeMap<String, Vec<String>>,
}

impl Document {
    /// Every value stored for `field`, or an empty slice when the document has none.
    pub fn values(&self, field: &str) -> &[String] {
        self.fields.get(field).map_or(&[], Vec::as_slice)
    }
}

/// One search domain's index.
#[derive(Debug, Default)]
pub struct DomainIndex {
    /// Document id -> document. Ordered by id, so `_id asc`, paging and cursor
    /// paging are stable across calls without storing a separate order.
    pub documents: BTreeMap<String, Document>,
}

impl DomainIndex {
    /// Store `document`, replacing any document already held under its id.
    pub fn add(&mut self, document: Document) {
        self.documents.insert(document.id.clone(), document);
    }

    /// Remove the document stored under `id`. `true` when one was there.
    pub fn delete(&mut self, id: &str) -> bool {
        self.documents.remove(id).is_some()
    }

    /// The documents to search, in `_id asc` order.
    pub fn snapshot(&self) -> Vec<Document> {
        self.documents.values().cloned().collect()
    }
}

/// The field `suggester` suggests from, or `None` when this domain has no such
/// suggester, which it has exactly while one of `documents` carries a field of
/// that name.
///
/// DERIVED, never stored: a suggester exists exactly as long as a document
/// carries its field, so deleting the documents that introduced a field also
/// removes its suggester and nothing accumulates per field name ever seen.
/// Judged on the SNAPSHOT the caller already took, so one `Suggest` call reads
/// the index once and holds the store's lock only for that read.
///
/// LIMITATION, stated here because it is the one place a reader can check it:
/// on AWS a suggester is defined out of band, by the configuration service's
/// `DefineSuggester` (still a scaffold in this emulator, filed as its own
/// issue), and this endpoint cannot see those definitions. Until it can, each
/// field of the indexed documents exposes ONE suggester, under the field's own
/// name — nothing speculative beyond that. Any other name does not exist, and
/// `Suggest` refuses it with the modelled `SearchException`.
pub fn suggester_field(documents: &[Document], suggester: &str) -> Option<String> {
    documents
        .iter()
        .any(|document| document.fields.contains_key(suggester))
        .then(|| suggester.to_string())
}

/// Per-service state for com-amazonaws-cloudsearchdomain-amazoncloudsearch2013. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsCloudsearchdomainAmazoncloudsearch2013State {
    /// Domain key (see [`domain_key`]) -> that domain's index.
    pub domains: BTreeMap<String, DomainIndex>,
}

/// What applying one batch changed, as `UploadDocuments` reports it.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Applied {
    /// How many `add` entries were applied.
    pub adds: i64,
    /// How many `delete` entries were applied.
    pub deletes: i64,
}

impl ComAmazonawsCloudsearchdomainAmazoncloudsearch2013State {
    /// Apply `entries` to `key`'s index as ONE unit and report what they changed.
    ///
    /// `Err` carries the message of the `DocumentServiceException` the batch
    /// earns: a batch that would push the store past [`MAX_DOCUMENTS_PER_DOMAIN`]
    /// or [`MAX_DOMAINS`] is refused, and nothing is applied when it is — the
    /// ceilings are checked against the batch's own outcome before any entry is
    /// written.
    pub fn apply(&mut self, key: &str, entries: Vec<sdf::Entry>) -> Result<Applied, String> {
        if !self.domains.contains_key(key) && self.domains.len() >= MAX_DOMAINS {
            return Err(format!(
                "This endpoint already serves {MAX_DOMAINS} search domains, which is \
                 the maximum; delete a domain's documents before uploading to a new one."
            ));
        }
        // The ids the index would hold afterwards, so the ceiling is judged on the
        // batch's outcome rather than on its length: a batch that replaces or
        // deletes documents does not grow the index. Computed BEFORE the index is
        // created, so a refused first batch leaves no empty index behind holding
        // one of the MAX_DOMAINS slots.
        let existing = self.domains.get(key);
        let mut ids: BTreeSet<&str> = existing
            .map(|index| index.documents.keys().map(String::as_str).collect())
            .unwrap_or_default();
        for entry in &entries {
            match entry {
                sdf::Entry::Add(document) => {
                    ids.insert(document.id.as_str());
                }
                sdf::Entry::Delete(id) => {
                    ids.remove(id.as_str());
                }
            }
        }
        let resulting = ids.len();
        if resulting > MAX_DOCUMENTS_PER_DOMAIN {
            return Err(format!(
                "The batch would leave this domain holding {resulting} documents; a \
                 domain holds at most {MAX_DOCUMENTS_PER_DOMAIN} documents."
            ));
        }

        let index = self.domains.entry(key.to_string()).or_default();
        let mut applied = Applied::default();
        for entry in entries {
            match entry {
                sdf::Entry::Add(document) => {
                    index.add(document);
                    applied.adds += 1;
                }
                sdf::Entry::Delete(id) => {
                    // A delete is applied whether or not the document was there:
                    // either way the document is gone afterwards.
                    index.delete(&id);
                    applied.deletes += 1;
                }
            }
        }
        Ok(applied)
    }

    /// The documents of `key`'s index, empty when nothing was ever uploaded to it.
    /// A read never creates the index.
    pub fn snapshot(&self, key: &str) -> Vec<Document> {
        self.domains
            .get(key)
            .map(DomainIndex::snapshot)
            .unwrap_or_default()
    }
}

/// Record one `UploadDocuments` batch: what the caller asked for, who asked (the
/// access key id the request presented), which domain's index changed and how
/// the call ended.
///
/// The one place this service writes an audit record, so every batch — applied
/// or refused — records the same fields. `UploadDocuments` is this service's
/// only state-changing operation, and it both inserts and REMOVES documents, so
/// the record is what ties a destructive change to the request that made it.
/// Only wire metadata and the counts the response itself reports go in: never a
/// document's field values, and never any credential material beyond the access
/// key id the request already presented in clear.
pub fn audit_batch(
    parts: &http::request::Parts,
    domain: &str,
    outcome: &str,
    adds: i64,
    deletes: i64,
) {
    ::tracing::info!(
        audit = true,
        operation = "UploadDocuments",
        outcome,
        caller_akid = %::engine::extract_akid(&parts.headers),
        domain,
        adds,
        deletes,
        "cloudsearchdomain document batch"
    );
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// `None` only when this service was mounted on an engine that is not the shared
/// [`::engine::InMemoryEngine`] — an emulator-side misconfiguration, not anything a
/// caller did, and unreachable through the router, which constructs every service
/// over that one engine. A handler answers it with its modelled 400 because this
/// service models no server fault and the scaffold channel offers only a 501
/// ("nobody wrote this yet", which would be a false claim for an implemented
/// operation); never with a fresh engine, which would hand that request a private
/// store dropped on return.
///
/// The misconfiguration is logged at error level HERE, once, so it shows up in the
/// emulator's own output: the caller only ever sees the modelled 400, which says
/// nothing about the engine the service was mounted on.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsCloudsearchdomainAmazoncloudsearch2013State>> {
    let state = (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsCloudsearchdomainAmazoncloudsearch2013State>);
    if state.is_none() {
        ::tracing::error!(
            engine = ::std::any::type_name::<Eng>(),
            "cloudsearchdomain is mounted on an engine that is not the shared \
             InMemoryEngine, so it holds no index; every request is answered with \
             its modelled 400"
        );
    }
    state
}

/// The index key a request addresses.
///
/// A CloudSearch domain is reached through two hosts — `doc-<domain>-<id>.<region>…`
/// for `UploadDocuments` and `search-<domain>-<id>.<region>…` for `Search`/`Suggest`
/// — which address the SAME index, so the key is the endpoint host's first label
/// with the `doc-`/`search-` role prefix removed. A host that names no domain (an
/// IP literal or `localhost`, as the local emulator and in-process callers use)
/// keys [`DEFAULT_DOMAIN_KEY`], so one process still serves a single index.
pub fn domain_key(parts: &http::request::Parts) -> String {
    let host = parts
        .headers
        .get(http::header::HOST)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string)
        .or_else(|| parts.uri.authority().map(|a| a.as_str().to_string()))
        .unwrap_or_default();
    let host = host.rsplit('@').next().unwrap_or_default().to_ascii_lowercase();
    let host = strip_port(&host);
    if host.is_empty() || host == "localhost" || host.parse::<std::net::IpAddr>().is_ok() {
        return DEFAULT_DOMAIN_KEY.to_string();
    }
    let label = host.split('.').next().unwrap_or_default();
    let label = label
        .strip_prefix("doc-")
        .or_else(|| label.strip_prefix("search-"))
        .unwrap_or(label);
    if label.is_empty() {
        DEFAULT_DOMAIN_KEY.to_string()
    } else {
        label.to_string()
    }
}

/// `host` without its `:port`, leaving an IPv6 literal's own colons alone.
///
/// Total for every header value a client can send: the bracketed form is taken
/// only when the value really is `[…]…`, and every other value falls through to
/// the `host:port` split, so a malformed host answers a key rather than panicking.
fn strip_port(host: &str) -> &str {
    // `[::1]:9297` — the authority form of an IPv6 literal.
    if let Some((address, _)) = host
        .strip_prefix('[')
        .and_then(|rest| rest.split_once(']'))
    {
        return address;
    }
    match host.rsplit_once(':') {
        Some((head, port)) if !port.is_empty() && port.bytes().all(|b| b.is_ascii_digit()) => head,
        _ => host,
    }
}

/// Monotonic part of [`new_rid`], so two request ids minted in the same nanosecond
/// still differ.
static RID_COUNTER: AtomicU64 = AtomicU64::new(0);

/// A fresh opaque request id for a `SearchStatus`/`SuggestStatus` `rid`.
///
/// CloudSearch documents `rid` as an encrypted, per-request resource id, so it is
/// generated per call rather than derived from the request: this one combines the
/// wall clock with a process counter, which makes it unique for every response.
pub fn new_rid() -> String {
    let counter = RID_COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_nanos());
    format!("{nanos:024x}{counter:08x}")
}

/// How long the call took, in whole milliseconds, for a `timems` member.
///
/// The measured duration, truncated to whole milliseconds and reported as
/// measured: an answer computed out of this process's memory can genuinely take
/// less than a millisecond, and CloudSearch reports `timems: 0` for a trivial
/// query, so nothing is floored onto the value.
pub fn elapsed_ms(started: Instant) -> i64 {
    i64::try_from(started.elapsed().as_millis()).unwrap_or(i64::MAX)
}
