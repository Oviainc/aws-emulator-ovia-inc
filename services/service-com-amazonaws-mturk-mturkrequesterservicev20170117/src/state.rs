// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-mturk-mturkrequesterservicev20170117 service.
//!
//! The resource this service is built around is the **HIT** (Human Intelligence
//! Task). A HIT is identified by its `HITId` (`EntityId`: `@length(1, 64)`,
//! `@pattern ^[A-Z0-9]+$`), so the store is keyed by that id and holds the modelled
//! [`HIT`] structure itself — that way `GetHIT` and `ListHITs` read back exactly the
//! structure `CreateHIT` returned rather than a second rendering of it.
//!
//! Beside the HITs the store holds:
//!
//! * the **HIT types** `CreateHIT` interns. `CreateHIT` is documented as creating
//!   "a new HIT ... with a new HITTypeID", where the HITTypeID "can be used to
//!   create additional HITs in the future without needing to specify common
//!   parameters such as the title, description and reward amount each time" — so the
//!   id is a function of exactly those common parameters and two HITs described
//!   identically share one HITTypeId. [`derive_hit_type_id`] is that function.
//! * the **UniqueRequestToken** index, which is what makes `CreateHIT` idempotent:
//!   "a unique identifier for this request which allows you to retry the call on
//!   error without creating duplicate HITs".
//! * the requester's **account balance**, which `GetAccountBalance` reports and
//!   which later bonus/payment work debits.
//!
//! The helpers below (id minting, the clock-owed expiry transition, the opaque
//! pagination token) are shared by several operation handlers, so they live here
//! rather than being copied into each `src/ops` file — a new file under `src/ops/`
//! would be picked up by `build.rs` as an operation.

use crate::ops::types::{QualificationRequirement, HIT};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::time::{Duration, SystemTime};

/// Per-service state for com-amazonaws-mturk-mturkrequesterservicev20170117. A handler reaches it through [`service_state`].
#[derive(Debug)]
pub struct ComAmazonawsMturkMturkrequesterservicev20170117State {
    /// Every HIT this emulator holds, keyed by `HITId`. `DeleteHIT` removes an entry,
    /// which is why a deleted HIT is reported by neither `GetHIT` nor `ListHITs`
    /// ("Disposed HITs are not returned in results for the ListHITs operation").
    ///
    /// Bounded by [`MAX_HITS_TOTAL`].
    pub hits: BTreeMap<String, StoredHit>,
    /// The HIT types interned by `CreateHIT`, keyed by `HITTypeId`, so that HITs
    /// described by the same common parameters report the same `HITTypeId`.
    ///
    /// Bounded by the HITs that reference it: [`Self::intern_hit_type`] refuses past
    /// [`MAX_HIT_TYPES_TOTAL`], and [`Self::release_hit_type`] drops a type once the
    /// last HIT of that type is disposed of, so a create/dispose loop cannot grow it.
    pub hit_types: BTreeMap<String, HitType>,
    /// `UniqueRequestToken` -> the HIT the call carrying it created.
    ///
    /// Caller-supplied keys holding no resource of their own, so the map is bounded
    /// three ways: the documented 24-hour token lifetime
    /// ([`UNIQUE_REQUEST_TOKEN_TTL`]), eviction of the oldest record past
    /// [`MAX_REMEMBERED_REQUEST_TOKENS`], and release of a disposed HIT's records.
    /// Private, so every write goes through the methods that keep those bounds.
    unique_request_tokens: HashMap<String, RememberedToken>,
    /// The keys of [`Self::unique_request_tokens`] in the order they were recorded, so
    /// the oldest record is found without scanning the map. The TTL is a constant, so
    /// this is expiry order too.
    unique_request_token_order: VecDeque<String>,
    /// The requester account `GetAccountBalance` reports.
    pub account: Account,
    /// Monotonic counter behind [`entity_id`]: the value used for a HIT is both its
    /// id and its creation rank, so `ListHITs` can order and paginate without
    /// depending on wall-clock resolution.
    pub next_id: u64,
}

impl Default for ComAmazonawsMturkMturkrequesterservicev20170117State {
    fn default() -> Self {
        Self {
            hits: BTreeMap::new(),
            hit_types: BTreeMap::new(),
            unique_request_tokens: HashMap::new(),
            unique_request_token_order: VecDeque::new(),
            account: Account::seeded(),
            next_id: 1,
        }
    }
}

impl ComAmazonawsMturkMturkrequesterservicev20170117State {
    /// The HIT a previous `CreateHIT` carrying this `UniqueRequestToken` created, if
    /// one is still remembered.
    ///
    /// Expired records are dropped first: "the unique token expires after 24 hours.
    /// Subsequent calls using the same UniqueRequestToken made after the 24 hour limit
    /// could create duplicate HITs" (`CreateHIT$UniqueRequestToken`). That is owed by
    /// the CLOCK, so it is materialised on the next call rather than slept for.
    pub fn replayed_hit_id(&mut self, token: Option<&str>, now: SystemTime) -> Option<String> {
        self.forget_expired_request_tokens(now);
        let token = token?;
        self.unique_request_tokens
            .get(token)
            .map(|remembered| remembered.hit_id.clone())
    }

    /// Record which HIT a `UniqueRequestToken` created, evicting the oldest record once
    /// [`MAX_REMEMBERED_REQUEST_TOKENS`] are held.
    ///
    /// An evicted record is indistinguishable from one whose 24 hours have passed: the
    /// next `CreateHIT` carrying that token creates another HIT, which is what the
    /// documentation says happens past the window.
    pub fn remember_request_token(&mut self, token: Option<&str>, hit_id: &str, now: SystemTime) {
        let Some(token) = token else {
            return;
        };
        let remembered = RememberedToken {
            hit_id: hit_id.to_string(),
            expires_at: now.checked_add(UNIQUE_REQUEST_TOKEN_TTL).unwrap_or(now),
        };
        if self
            .unique_request_tokens
            .insert(token.to_string(), remembered)
            .is_none()
        {
            self.unique_request_token_order.push_back(token.to_string());
        }
        while self.unique_request_token_order.len() > MAX_REMEMBERED_REQUEST_TOKENS {
            if let Some(oldest) = self.unique_request_token_order.pop_front() {
                self.unique_request_tokens.remove(&oldest);
            }
        }
    }

    /// Drop the token records naming `hit_id`.
    ///
    /// Called when a HIT is disposed of: the record can no longer replay anything, and
    /// nothing else would ever reclaim it.
    pub fn forget_request_tokens_for(&mut self, hit_id: &str) {
        let Self {
            unique_request_tokens,
            unique_request_token_order,
            ..
        } = self;
        unique_request_tokens.retain(|_, remembered| remembered.hit_id != hit_id);
        unique_request_token_order.retain(|token| unique_request_tokens.contains_key(token));
    }

    /// Drop every token record whose documented 24 hours have passed.
    ///
    /// The TTL is a constant, so `unique_request_token_order` is in expiry order and
    /// the walk stops at the first record still inside its window.
    fn forget_expired_request_tokens(&mut self, now: SystemTime) {
        while let Some(oldest) = self.unique_request_token_order.front() {
            let expired = self
                .unique_request_tokens
                .get(oldest)
                .is_none_or(|remembered| remembered.expires_at <= now);
            if !expired {
                return;
            }
            if let Some(oldest) = self.unique_request_token_order.pop_front() {
                self.unique_request_tokens.remove(&oldest);
            }
        }
    }

    /// Intern `hit_type` under its id, returning that id.
    ///
    /// A type already held is reused, which is what makes two identically described
    /// HITs report one `HITTypeId`. `Err` carries the ceiling that was reached, which
    /// the caller reports as a `RequestError`.
    pub fn intern_hit_type(&mut self, hit_type: HitType) -> Result<String, usize> {
        let hit_type_id = hit_type.hit_type_id.clone();
        if !self.hit_types.contains_key(&hit_type_id) && self.hit_types.len() >= MAX_HIT_TYPES_TOTAL
        {
            return Err(MAX_HIT_TYPES_TOTAL);
        }
        self.hit_types.entry(hit_type_id.clone()).or_insert(hit_type);
        Ok(hit_type_id)
    }

    /// Drop the interned type `hit_type_id` if no stored HIT is of that type any more.
    ///
    /// Called after a HIT is disposed of, so the number of interned types stays bounded
    /// by the number of live HITs rather than by how many distinct descriptions a
    /// caller has ever sent.
    pub fn release_hit_type(&mut self, hit_type_id: &str) {
        let still_used = self
            .hits
            .values()
            .any(|stored| stored.hit.hit_type_id.as_deref() == Some(hit_type_id));
        if !still_used {
            self.hit_types.remove(hit_type_id);
        }
    }
}

/// What a `UniqueRequestToken` is remembered as: the HIT it created, and when the
/// documented 24-hour window on it closes.
#[derive(Debug, Clone)]
struct RememberedToken {
    hit_id: String,
    expires_at: SystemTime,
}

/// One persisted HIT: the modelled structure every reader hands back, plus the
/// creation rank the counter assigned it.
///
/// The rank is kept beside the HIT rather than derived from `CreationTime` because
/// two HITs created within the same clock tick would otherwise compare equal, and
/// `ListHITs`' page boundaries have to be a total order.
#[derive(Debug, Clone)]
pub struct StoredHit {
    /// The HIT as `CreateHIT` returned it and `GetHIT` / `ListHITs` read it back.
    pub hit: HIT,
    /// Creation rank: strictly increasing, never reused, and what a `NextToken`
    /// resumes after.
    pub sequence: u64,
}

/// The "common parameters" that define a HIT type, as `CreateHITType` takes them.
///
/// Two `CreateHIT` calls supplying equal values here land on one `HITTypeId`; see
/// [`derive_hit_type_id`].
#[derive(Debug, Clone)]
pub struct HitType {
    pub hit_type_id: String,
    pub title: String,
    pub description: String,
    pub keywords: Option<String>,
    pub reward: String,
    pub assignment_duration_in_seconds: i64,
    pub auto_approval_delay_in_seconds: Option<i64>,
    pub qualification_requirements: Vec<QualificationRequirement>,
}

/// The requester's account, as `GetAccountBalance` reports it.
///
/// Both members are `CurrencyAmount` (`@pattern ^[0-9]+(\.)?[0-9]{0,2}$`), so they
/// are held as the strings that go on the wire.
#[derive(Debug, Clone)]
pub struct Account {
    pub available_balance: String,
    pub on_hold_balance: String,
}

impl Account {
    /// The balance a fresh emulator starts with. The MTurk *sandbox* credits every
    /// requester account with a fixed play balance instead of real money, and this
    /// emulator is a sandbox: the figure is this emulator's choice, not an AWS
    /// constant, and it exists so the operations that spend money (`SendBonus`,
    /// `CreateAdditionalAssignmentsForHIT`) have something to debit.
    fn seeded() -> Self {
        Self {
            available_balance: "10000.00".to_string(),
            on_hold_balance: "0.00".to_string(),
        }
    }
}

/// `HITStatus` values, from the model's own `HITStatus` enum
/// (`Assignable | Unassignable | Reviewable | Reviewing | Disposed`). Only the three
/// a HIT with no assignments can be in are named here; the rest arrive with the
/// assignment operations.
pub const HIT_STATUS_ASSIGNABLE: &str = "Assignable";
/// See [`HIT_STATUS_ASSIGNABLE`].
pub const HIT_STATUS_UNASSIGNABLE: &str = "Unassignable";
/// See [`HIT_STATUS_ASSIGNABLE`].
pub const HIT_STATUS_REVIEWABLE: &str = "Reviewable";

/// The `HITReviewStatus` a HIT starts in, from the model's `HITReviewStatus` enum
/// (`NotReviewed | MarkedForReview | ReviewedAppropriate | ReviewedInappropriate`).
pub const HIT_REVIEW_STATUS_NOT_REVIEWED: &str = "NotReviewed";

/// `MaxAssignments`' documented default: "Default: 1" on `CreateHIT$MaxAssignments`
/// in the MTurk API reference. The model carries no `@default`, so an absent member
/// is resolved here.
pub const DEFAULT_MAX_ASSIGNMENTS: i32 = 1;

/// `ListHITs`' page size when the caller names none. The MTurk API reference
/// documents "Default: 10" for `MaxResults` on the List operations; the modelled
/// `@range(1, 100)` bounds what a caller may ask for instead.
pub const DEFAULT_MAX_RESULTS: usize = 10;

/// This emulator's ceiling on the number of HITs it will hold at once. Not an AWS
/// quota: the whole store lives in memory and `CreateHIT` takes no account scoping,
/// so without a bound a client could grow this process without limit.
pub const MAX_HITS_TOTAL: usize = 100_000;

/// Hard ceiling on the number of interned HIT types.
///
/// A `HITTypeId` is derived from request data, so how many DISTINCT types exist is a
/// caller's choice; a type is released when the last HIT of that type is disposed of
/// (`ComAmazonawsMturkMturkrequesterservicev20170117State::release_hit_type`), which
/// keeps the count at or below the number of live HITs. This is the ceiling that
/// holds even when a future `CreateHITType` interns a type with no HIT behind it yet.
pub const MAX_HIT_TYPES_TOTAL: usize = MAX_HITS_TOTAL;

/// How many `UniqueRequestToken` -> HIT records are kept for idempotent replay.
///
/// Tokens are caller-supplied and hold no resource of their own, so the map is
/// caller-driven growth: records naming a disposed HIT are dropped with it, records
/// past [`UNIQUE_REQUEST_TOKEN_TTL`] are dropped on the next call, and beyond this
/// many live records the oldest is evicted.
pub const MAX_REMEMBERED_REQUEST_TOKENS: usize = 10_000;

/// How long a `UniqueRequestToken` keeps replaying, straight from the documentation
/// on `CreateHIT$UniqueRequestToken`: "The unique token expires after 24 hours.
/// Subsequent calls using the same UniqueRequestToken made after the 24 hour limit
/// could create duplicate HITs."
pub const UNIQUE_REQUEST_TOKEN_TTL: Duration = Duration::from_secs(24 * 60 * 60);

/// Render `rank` as an `EntityId`: `@pattern ^[A-Z0-9]+$`, `@length(1, 64)`.
///
/// Fixed-width uppercase hex of the store's monotonic counter — unique within the
/// emulator, stable across reads, and never reused once minted.
#[must_use]
pub fn entity_id(rank: u64) -> String {
    format!("{rank:016X}")
}

/// The `HITTypeId` for a HIT type: a SHA-256 over the canonical encoding of the
/// common parameters, rendered as uppercase hex and clipped to 32 characters so it
/// satisfies `EntityId`'s `^[A-Z0-9]+$` and `@length(1, 64)`.
///
/// A function of the parameters alone, so "identical field sets reuse the same
/// HITTypeId" holds for `CreateHIT` and (later) `CreateHITType` without either
/// having to search the store. Every field is length-prefixed, so no two distinct
/// field sets can encode to the same bytes.
#[must_use]
pub fn derive_hit_type_id(hit_type: &HitType) -> String {
    fn field(hasher: &mut Sha256, value: &str) {
        hasher.update(value.len().to_be_bytes());
        hasher.update(value.as_bytes());
    }

    let mut hasher = Sha256::new();
    field(&mut hasher, &hit_type.title);
    field(&mut hasher, &hit_type.description);
    field(&mut hasher, hit_type.keywords.as_deref().unwrap_or(""));
    field(&mut hasher, &hit_type.reward);
    field(
        &mut hasher,
        &hit_type.assignment_duration_in_seconds.to_string(),
    );
    field(
        &mut hasher,
        &hit_type
            .auto_approval_delay_in_seconds
            .map(|seconds| seconds.to_string())
            .unwrap_or_default(),
    );
    field(
        &mut hasher,
        &hit_type.qualification_requirements.len().to_string(),
    );
    for requirement in &hit_type.qualification_requirements {
        field(&mut hasher, &requirement.qualification_type_id);
        field(&mut hasher, &requirement.comparator);
        field(
            &mut hasher,
            &requirement
                .integer_values
                .iter()
                .flatten()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(","),
        );
        field(
            &mut hasher,
            &requirement
                .locale_values
                .iter()
                .flatten()
                .map(|locale| {
                    format!(
                        "{}-{}",
                        locale.country,
                        locale.subdivision.as_deref().unwrap_or("")
                    )
                })
                .collect::<Vec<_>>()
                .join(","),
        );
        field(
            &mut hasher,
            &requirement.required_to_preview.unwrap_or(false).to_string(),
        );
        field(
            &mut hasher,
            requirement.actions_guarded.as_deref().unwrap_or(""),
        );
    }
    let mut digest = hex::encode_upper(hasher.finalize());
    // 32 of the 64 hex characters: still far inside `EntityId`'s `@length(1, 64)`, and
    // 128 bits of the digest is more than enough to keep two distinct descriptions apart.
    digest.truncate(32);
    digest
}

/// A timestamp as the comparable pair `(seconds, nanoseconds)`.
///
/// `aws_smithy_types::DateTime` carries the two halves separately, so this is how
/// two instants are ordered.
fn instant(timestamp: &::aws_smithy_types::DateTime) -> (i64, u32) {
    (timestamp.secs(), timestamp.subsec_nanos())
}

/// How many assignments of `hit` are still available for Workers to accept:
/// `MaxAssignments` less those pending and those already completed.
fn assignments_available(hit: &HIT) -> i32 {
    hit.max_assignments
        .unwrap_or(DEFAULT_MAX_ASSIGNMENTS)
        .saturating_sub(hit.number_of_assignments_pending.unwrap_or(0))
        .saturating_sub(hit.number_of_assignments_completed.unwrap_or(0))
        .max(0)
}

/// Bring `hit`'s status and available-assignment count in line with the clock.
///
/// `Expiration` is "the date and time the HIT expires", and a HIT whose lifetime has
/// elapsed "no longer appears in HIT searches": it becomes `Reviewable` and offers no
/// assignments, whether or not anybody called an operation at the moment it lapsed.
/// This is the transition the CLOCK owes, so a reporting operation materialises it on
/// read rather than inventing it; the reverse direction is what
/// `UpdateExpirationForHIT` performs when it moves `ExpireAt` back into the future.
///
/// Returns whether anything changed, so a caller can tell a materialised transition
/// from a no-op.
pub fn settle_hit_against_clock(hit: &mut HIT, now: SystemTime) -> bool {
    let now = instant(&::aws_smithy_types::DateTime::from(now));
    let expired = hit
        .expiration
        .as_ref()
        .is_some_and(|expiration| instant(expiration) <= now);
    let available = if expired { 0 } else { assignments_available(hit) };
    let status = if expired {
        HIT_STATUS_REVIEWABLE
    } else if available > 0 {
        HIT_STATUS_ASSIGNABLE
    } else {
        HIT_STATUS_UNASSIGNABLE
    };

    let changed = hit.number_of_assignments_available != Some(available)
        || hit.hit_status.as_deref() != Some(status);
    hit.number_of_assignments_available = Some(available);
    hit.hit_status = Some(status.to_string());
    changed
}

/// `CreationTime + LifetimeInSeconds` — "an amount of time, in seconds, after which
/// the HIT is no longer available for users to accept".
///
/// `None` when the sum is not representable, which the caller refuses as an invalid
/// `LifetimeInSeconds`.
#[must_use]
pub fn expiration_from_lifetime(
    created: SystemTime,
    lifetime_in_seconds: i64,
) -> Option<SystemTime> {
    let seconds = u64::try_from(lifetime_in_seconds).ok()?;
    created.checked_add(Duration::from_secs(seconds))
}

/// Marker the opaque `NextToken` carries, so a token minted by another service (or
/// invented by a caller) does not decode here.
const NEXT_TOKEN_PREFIX: &str = "mturk:hits:after-sequence:";

/// Encode "resume after the HIT with this creation rank" into a `NextToken`.
///
/// The rank, not an offset: ranks are never reused, so a token stays meaningful
/// after HITs on either side of the page boundary are created or disposed of. The
/// rendering keeps the result inside `PaginationToken`'s `@length(1, 255)`.
#[must_use]
pub fn encode_next_token(after_sequence: u64) -> String {
    hex::encode(format!("{NEXT_TOKEN_PREFIX}{after_sequence}"))
}

/// Decode a `NextToken` minted by [`encode_next_token`]; `None` for anything else,
/// which the caller refuses as an invalid token.
#[must_use]
pub fn decode_next_token(token: &str) -> Option<u64> {
    let decoded = String::from_utf8(hex::decode(token).ok()?).ok()?;
    decoded.strip_prefix(NEXT_TOKEN_PREFIX)?.parse().ok()
}

/// One page of `HITId`s, newest first, plus the token for the page after it.
///
/// [`entity_id`] renders a HIT's creation rank as FIXED-WIDTH uppercase hex, so the
/// store's key order IS creation order and a page is a bounded range query: a
/// `ListHITs` request costs work proportional to the page it returns, not to the
/// whole store. "Newest first" is therefore by creation rank — the order
/// `CreationTime` gives, but total (see [`StoredHit::sequence`]).
///
/// `max_results` is the modelled `@paginated` page size, already range-checked at the
/// boundary; absent means [`DEFAULT_MAX_RESULTS`]. `next_token` is the token a
/// previous page returned, and `None` is returned when it does not decode — which the
/// caller reports as a `RequestError`.
///
/// A token that resumes past every remaining HIT (because the rest were disposed of
/// meanwhile) yields an empty last page rather than an error: nothing remains, which
/// is exactly what the caller is being told.
pub fn page_ids(
    store: &ComAmazonawsMturkMturkrequesterservicev20170117State,
    max_results: Option<i32>,
    next_token: Option<&str>,
) -> Option<(Vec<String>, Option<String>)> {
    // At least one item per page: a zero-sized page would hand back a token pointing
    // at the same boundary, which is a caller loop that never terminates. The modelled
    // `@range(min: 1)` already makes this unreachable from the wire.
    let page_size = max_results
        .map_or(DEFAULT_MAX_RESULTS, |max| {
            usize::try_from(max).unwrap_or(DEFAULT_MAX_RESULTS)
        })
        .max(1);
    // One id past the page, which is how "is a NextToken owed?" is answered without
    // walking what remains.
    let wanted = page_size.saturating_add(1);
    let mut ids: Vec<String> = match next_token {
        Some(token) => {
            let boundary = entity_id(decode_next_token(token)?);
            store
                .hits
                .range::<String, _>(..boundary)
                .rev()
                .take(wanted)
                .map(|(id, _)| id.clone())
                .collect()
        }
        None => store
            .hits
            .iter()
            .rev()
            .take(wanted)
            .map(|(id, _)| id.clone())
            .collect(),
    };
    let more = ids.len() == wanted;
    ids.truncate(page_size);
    let next = if more {
        ids.last()
            .and_then(|id| store.hits.get(id))
            .map(|stored| encode_next_token(stored.sequence))
    } else {
        None
    };
    Some((ids, next))
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsMturkMturkrequesterservicev20170117State>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsMturkMturkrequesterservicev20170117State>)
}
