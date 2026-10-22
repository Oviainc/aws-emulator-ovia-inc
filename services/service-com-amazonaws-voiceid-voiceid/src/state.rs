// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-voiceid-voiceid service.
//!
//! The stored records are the service's own: a handler builds the modelled shapes
//! (`Domain`, `DomainSummary`, `Watchlist`, `WatchlistSummary`) FROM a record, so
//! every response is computed from what a previous request persisted.

use std::collections::{BTreeSet, HashMap, VecDeque};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::ops::types::{
    Domain, DomainSummary, ServerSideEncryptionConfiguration, Watchlist, WatchlistDetails,
    WatchlistSummary,
};

/// Length of a `DomainId` / `WatchlistId`: the model pins both at
/// `^[a-zA-Z0-9]{22}$` (length 22..22).
pub const RESOURCE_ID_LEN: usize = 22;

/// The alphabet a `DomainId` / `WatchlistId` is drawn from, i.e. exactly the
/// characters the modelled pattern `^[a-zA-Z0-9]{22}$` admits.
const RESOURCE_ID_ALPHABET: &[u8; 62] =
    b"0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ";

/// The status a domain is created in. `DomainStatus` accepts
/// `ACTIVE`/`PENDING`/`SUSPENDED` (see `DOMAIN_STATUS_VALUES`); a freshly created
/// Voice ID domain is usable at once, so it is returned ready and stays ready.
pub const DOMAIN_STATUS_ACTIVE: &str = "ACTIVE";

/// `ResourceType` value naming a domain, for `ResourceNotFoundException`.
pub const RESOURCE_TYPE_DOMAIN: &str = "DOMAIN";

/// `ResourceType` value naming a watchlist, for `ResourceNotFoundException`.
pub const RESOURCE_TYPE_WATCHLIST: &str = "WATCHLIST";

/// The ARN namespace of this service (`awsService.arnNamespace` in the model).
pub const ARN_NAMESPACE: &str = "voiceid";

/// The region used when a request carries no SigV4 credential scope to read one
/// from (emulators default to `us-east-1`).
pub const DEFAULT_REGION: &str = "us-east-1";

/// Name of the watchlist every domain is created with. AWS documents that "every
/// domain is created with a default watchlist" but not the name it carries, so
/// the label the console shows is used.
pub const DEFAULT_WATCHLIST_NAME: &str = "Default";

/// `ListDomains`' `MaxResults` range maximum (model: range 1..10), also the page
/// size used when a request omits `MaxResults`.
pub const LIST_DOMAINS_MAX_RESULTS: i32 = 10;

/// `ListWatchlists`' `MaxResults` range maximum (model: range 1..100), also the
/// page size used when a request omits `MaxResults`.
pub const LIST_WATCHLISTS_MAX_RESULTS: i32 = 100;

/// Capacity guard, NOT an AWS quota: the number of domains this emulator keeps
/// in memory at once. A create past it is refused with the modelled
/// `ServiceQuotaExceededException` (HTTP 402) rather than growing the store
/// without limit, and it is set far above any realistic emulator workload so it
/// never stands in for Voice ID's own (adjustable, account-level) quota.
pub const MAX_DOMAINS: usize = 1_000;

/// Capacity guard, NOT an AWS quota: watchlists kept per domain, counting the
/// default watchlist. Refused the same way as [`MAX_DOMAINS`].
pub const MAX_WATCHLISTS_PER_DOMAIN: usize = 1_000;

/// How many client tokens stay remembered for idempotent replay. The map is
/// evicted oldest-first past this count, so a caller that sends a fresh token on
/// every create cannot grow it without limit; a token evicted this way simply
/// stops being recognised, exactly as one aged out of a replay window does.
pub const MAX_REMEMBERED_CLIENT_TOKENS: usize = 10_000;

/// Attempts a fresh-id mint makes before giving up. The id space is 62^22, so a
/// single collision is already vanishingly unlikely and exhausting these means
/// the random source is broken — answered as an internal failure rather than
/// looped on forever.
const ID_MINT_ATTEMPTS: usize = 8;

/// One stored domain.
#[derive(Debug, Clone)]
pub struct DomainRecord {
    pub domain_id: String,
    pub arn: String,
    pub name: String,
    pub description: Option<String>,
    pub domain_status: String,
    pub kms_key_id: String,
    pub created_at: SystemTime,
    pub updated_at: SystemTime,
    pub default_watchlist_id: String,
    /// Tags given at creation. Kept so the tagging operations have the one copy
    /// of a domain's tags to read when they are implemented.
    pub tags: Vec<(String, String)>,
}

impl DomainRecord {
    /// The modelled `Domain` for this record.
    pub fn to_domain(&self) -> Domain {
        Domain {
            domain_id: Some(self.domain_id.clone()),
            arn: Some(self.arn.clone()),
            name: Some(self.name.clone()),
            description: self.description.clone(),
            domain_status: Some(self.domain_status.clone()),
            server_side_encryption_configuration: Some(ServerSideEncryptionConfiguration {
                kms_key_id: self.kms_key_id.clone(),
            }),
            created_at: Some(to_date_time(self.created_at)),
            updated_at: Some(to_date_time(self.updated_at)),
            // Set only once a server-side encryption update has been requested,
            // which this slice never does.
            server_side_encryption_update_details: None,
            watchlist_details: Some(WatchlistDetails {
                default_watchlist_id: self.default_watchlist_id.clone(),
            }),
        }
    }

    /// The modelled `DomainSummary` for this record: the same stored fields
    /// `Domain` reports, which is what `ListDomains` must return.
    pub fn to_summary(&self) -> DomainSummary {
        DomainSummary {
            domain_id: Some(self.domain_id.clone()),
            arn: Some(self.arn.clone()),
            name: Some(self.name.clone()),
            description: self.description.clone(),
            domain_status: Some(self.domain_status.clone()),
            server_side_encryption_configuration: Some(ServerSideEncryptionConfiguration {
                kms_key_id: self.kms_key_id.clone(),
            }),
            created_at: Some(to_date_time(self.created_at)),
            updated_at: Some(to_date_time(self.updated_at)),
            server_side_encryption_update_details: None,
            watchlist_details: Some(WatchlistDetails {
                default_watchlist_id: self.default_watchlist_id.clone(),
            }),
        }
    }
}

/// One stored watchlist.
#[derive(Debug, Clone)]
pub struct WatchlistRecord {
    pub domain_id: String,
    pub watchlist_id: String,
    pub name: String,
    pub description: Option<String>,
    pub default_watchlist: bool,
    pub created_at: SystemTime,
    pub updated_at: SystemTime,
    /// The fraudsters associated with this watchlist. Empty until
    /// `AssociateFraudster` is implemented; it is the one place a domain's
    /// fraudster membership lives.
    pub fraudster_ids: BTreeSet<String>,
}

impl WatchlistRecord {
    /// The modelled `Watchlist` for this record.
    pub fn to_watchlist(&self) -> Watchlist {
        Watchlist {
            domain_id: Some(self.domain_id.clone()),
            watchlist_id: Some(self.watchlist_id.clone()),
            name: Some(self.name.clone()),
            description: self.description.clone(),
            default_watchlist: self.default_watchlist,
            created_at: Some(to_date_time(self.created_at)),
            updated_at: Some(to_date_time(self.updated_at)),
        }
    }

    /// The modelled `WatchlistSummary` for this record.
    pub fn to_summary(&self) -> WatchlistSummary {
        WatchlistSummary {
            domain_id: Some(self.domain_id.clone()),
            watchlist_id: Some(self.watchlist_id.clone()),
            name: Some(self.name.clone()),
            description: self.description.clone(),
            default_watchlist: self.default_watchlist,
            created_at: Some(to_date_time(self.created_at)),
            updated_at: Some(to_date_time(self.updated_at)),
        }
    }
}

/// The watchlists of one domain, in creation order.
#[derive(Debug, Default)]
pub struct WatchlistStore {
    /// Watchlist ids in creation order, so a pagination token resumes stably.
    pub order: Vec<String>,
    pub by_id: HashMap<String, WatchlistRecord>,
}

impl WatchlistStore {
    fn insert(&mut self, record: WatchlistRecord) {
        self.order.push(record.watchlist_id.clone());
        self.by_id.insert(record.watchlist_id.clone(), record);
    }
}

/// What a `ClientToken` is remembered under: an idempotency token is scoped to
/// the operation (and, for a watchlist, to the domain it was created in), so the
/// same token replayed at two different creates does not alias.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct IdempotencyKey {
    pub scope: String,
    pub token: String,
}

impl IdempotencyKey {
    /// The key a `CreateDomain` client token is remembered under.
    pub fn domain(token: &str) -> Self {
        Self {
            scope: "CreateDomain".to_string(),
            token: token.to_string(),
        }
    }

    /// The key a `CreateWatchlist` client token is remembered under, scoped to
    /// the domain the watchlist is created in.
    pub fn watchlist(domain_id: &str, token: &str) -> Self {
        Self {
            scope: Self::watchlist_scope(domain_id),
            token: token.to_string(),
        }
    }

    /// The scope every `CreateWatchlist` token of one domain is remembered under,
    /// so the domain's tokens can be found without building a key around a dummy
    /// token.
    pub fn watchlist_scope(domain_id: &str) -> String {
        format!("CreateWatchlist/{domain_id}")
    }
}

/// Why a create could not be served.
#[derive(Debug)]
pub enum CreateError {
    /// The store is at its capacity guard: the modelled
    /// `ServiceQuotaExceededException` (HTTP 402).
    QuotaExceeded(String),
    /// Id minting made [`ID_MINT_ATTEMPTS`] attempts without finding a free id:
    /// a server-side failure (HTTP 500), never a client error.
    IdMintExhausted(String),
}

/// Per-service state for com-amazonaws-voiceid-voiceid. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsVoiceidVoiceidState {
    /// Every domain that has been created and not yet deleted, by `DomainId`.
    /// At most [`MAX_DOMAINS`] entries.
    pub domains: HashMap<String, DomainRecord>,
    /// Domain ids in creation order, so `ListDomains` pages stably. Holds one id
    /// per entry of `domains`.
    pub domain_order: Vec<String>,
    /// Watchlists by `DomainId`; removed with the domain, at most
    /// [`MAX_WATCHLISTS_PER_DOMAIN`] per domain.
    pub watchlists: HashMap<String, WatchlistStore>,
    /// Client token -> the id of the resource the first call created, so a
    /// replayed `@idempotent` create returns that resource instead of minting a
    /// second one. At most [`MAX_REMEMBERED_CLIENT_TOKENS`] entries; write
    /// through [`ComAmazonawsVoiceidVoiceidState::remember_token`], which keeps
    /// that bound.
    pub idempotency: HashMap<IdempotencyKey, String>,
    /// The remembered tokens in the order they were first seen, so the oldest is
    /// the one evicted when the map is full.
    idempotency_order: VecDeque<IdempotencyKey>,
    /// Monotonic source of every `DomainId`/`WatchlistId` this service mints, so
    /// the ids of one run are unique without a random source.
    next_id_seq: u64,
}

impl ComAmazonawsVoiceidVoiceidState {
    /// Mint a `DomainId` no live domain carries, in at most [`ID_MINT_ATTEMPTS`]
    /// attempts.
    fn fresh_domain_id(&mut self) -> Option<String> {
        for _ in 0..ID_MINT_ATTEMPTS {
            self.next_id_seq += 1;
            let candidate = id_from_seq(self.next_id_seq);
            if !self.domains.contains_key(&candidate) {
                return Some(candidate);
            }
        }
        None
    }

    /// Mint a `WatchlistId` no watchlist of `domain_id` carries, in at most
    /// [`ID_MINT_ATTEMPTS`] attempts. Minting is separate from inserting, so a
    /// mint that fails leaves the store untouched.
    fn fresh_watchlist_id(&mut self, domain_id: &str) -> Option<String> {
        for _ in 0..ID_MINT_ATTEMPTS {
            self.next_id_seq += 1;
            let candidate = id_from_seq(self.next_id_seq);
            let taken = self
                .watchlists
                .get(domain_id)
                .is_some_and(|store| store.by_id.contains_key(&candidate));
            if !taken {
                return Some(candidate);
            }
        }
        None
    }

    /// Remember `created` under `key`, evicting the oldest remembered token once
    /// the map is at [`MAX_REMEMBERED_CLIENT_TOKENS`].
    pub fn remember_token(&mut self, key: IdempotencyKey, created: String) {
        if self.idempotency.insert(key.clone(), created).is_none() {
            self.idempotency_order.push_back(key);
        }
        while self.idempotency.len() > MAX_REMEMBERED_CLIENT_TOKENS {
            match self.idempotency_order.pop_front() {
                Some(oldest) => {
                    self.idempotency.remove(&oldest);
                }
                // The deque holds one entry per map key, so it cannot run dry
                // while the map is over the bound; stop rather than spin.
                None => break,
            }
        }
    }

    /// Forget every remembered token the predicate does not keep.
    fn retain_tokens(&mut self, keep: impl Fn(&IdempotencyKey, &str) -> bool) {
        self.idempotency
            .retain(|key, created| keep(key, created.as_str()));
        let live = &self.idempotency;
        self.idempotency_order.retain(|key| live.contains_key(key));
    }

    /// Create a domain and its default watchlist, and return the stored record.
    ///
    /// `now` is the single instant stamped on the domain and its default
    /// watchlist, so the two agree.
    #[allow(clippy::too_many_arguments)]
    pub fn create_domain(
        &mut self,
        name: String,
        description: Option<String>,
        kms_key_id: String,
        tags: Vec<(String, String)>,
        region: &str,
        now: SystemTime,
    ) -> Result<DomainRecord, CreateError> {
        if self.domains.len() >= MAX_DOMAINS {
            return Err(CreateError::QuotaExceeded(format!(
                "This emulator holds its limit of {MAX_DOMAINS} domains; delete a domain before creating another."
            )));
        }
        let domain_id = self.fresh_domain_id().ok_or_else(|| {
            CreateError::IdMintExhausted("could not mint a fresh DomainId".to_string())
        })?;
        // Both ids are minted BEFORE anything is inserted: a mint that fails must
        // leave the store exactly as it was, with no watchlist entry standing
        // under a domain that was never created.
        let default_watchlist_id = self.fresh_watchlist_id(&domain_id).ok_or_else(|| {
            CreateError::IdMintExhausted("could not mint a fresh WatchlistId".to_string())
        })?;

        let watchlists = self.watchlists.entry(domain_id.clone()).or_default();
        watchlists.insert(WatchlistRecord {
            domain_id: domain_id.clone(),
            watchlist_id: default_watchlist_id.clone(),
            name: DEFAULT_WATCHLIST_NAME.to_string(),
            description: None,
            default_watchlist: true,
            created_at: now,
            updated_at: now,
            fraudster_ids: BTreeSet::new(),
        });

        let record = DomainRecord {
            arn: ::engine::emulator_arn(ARN_NAMESPACE, region, &format!("domain/{domain_id}")),
            domain_id: domain_id.clone(),
            name,
            description,
            domain_status: DOMAIN_STATUS_ACTIVE.to_string(),
            kms_key_id,
            created_at: now,
            updated_at: now,
            default_watchlist_id,
            tags,
        };
        self.domain_order.push(domain_id.clone());
        self.domains.insert(domain_id, record.clone());
        Ok(record)
    }

    /// Create a non-default watchlist in `domain_id` and return the stored
    /// record. The caller has already checked the domain exists.
    pub fn create_watchlist(
        &mut self,
        domain_id: &str,
        name: String,
        description: Option<String>,
        now: SystemTime,
    ) -> Result<WatchlistRecord, CreateError> {
        let held = self
            .watchlists
            .get(domain_id)
            .map_or(0, |store| store.order.len());
        if held >= MAX_WATCHLISTS_PER_DOMAIN {
            return Err(CreateError::QuotaExceeded(format!(
                "Domain {domain_id} holds this emulator's limit of {MAX_WATCHLISTS_PER_DOMAIN} watchlists."
            )));
        }
        let watchlist_id = self.fresh_watchlist_id(domain_id).ok_or_else(|| {
            CreateError::IdMintExhausted("could not mint a fresh WatchlistId".to_string())
        })?;
        let watchlists = self.watchlists.entry(domain_id.to_string()).or_default();
        let record = WatchlistRecord {
            domain_id: domain_id.to_string(),
            watchlist_id,
            name,
            description,
            default_watchlist: false,
            created_at: now,
            updated_at: now,
            fraudster_ids: BTreeSet::new(),
        };
        watchlists.insert(record.clone());
        Ok(record)
    }

    /// Remove a domain, its watchlists and the client tokens scoped to it.
    /// Answers whether a domain was there to remove.
    pub fn delete_domain(&mut self, domain_id: &str) -> bool {
        let removed = self.domains.remove(domain_id).is_some();
        self.domain_order.retain(|id| id != domain_id);
        self.watchlists.remove(domain_id);
        let watchlist_scope = IdempotencyKey::watchlist_scope(domain_id);
        self.retain_tokens(|key, created| key.scope != watchlist_scope && created != domain_id);
        removed
    }

    /// The watchlist ids of a domain, in creation order.
    pub fn watchlist_order(&self, domain_id: &str) -> &[String] {
        match self.watchlists.get(domain_id) {
            Some(store) => store.order.as_slice(),
            None => &[],
        }
    }

    /// One stored watchlist of a domain.
    pub fn watchlist(&self, domain_id: &str, watchlist_id: &str) -> Option<&WatchlistRecord> {
        self.watchlists
            .get(domain_id)
            .and_then(|store| store.by_id.get(watchlist_id))
    }
}

/// One page of ids out of an insertion-ordered list.
#[derive(Debug)]
pub struct Page {
    /// The ids on this page, in stored order.
    pub ids: Vec<String>,
    /// The token that resumes after this page — `None` when nothing remains, so
    /// a caller stops exactly when the last page is served.
    pub next_token: Option<String>,
}

/// Take the page of `order` that `next_token` resumes at.
///
/// The token is the id of the first item of the next page, so paging is stable
/// across inserts at the end of the list. An `Err` carries the message for a
/// `ValidationException`: a token naming an item the list no longer holds cannot
/// be resumed from.
pub fn page(order: &[String], next_token: Option<&str>, page_size: usize) -> Result<Page, String> {
    let start = match next_token {
        None => 0,
        Some(token) => order
            .iter()
            .position(|id| id == token)
            .ok_or_else(|| format!("NextToken '{token}' is not a valid pagination token."))?,
    };
    let size = page_size.max(1);
    let end = order.len().min(start.saturating_add(size));
    Ok(Page {
        ids: order[start..end].to_vec(),
        next_token: order.get(end).cloned(),
    })
}

/// The page size a `MaxResults` member asks for, bounded by the model's range.
pub fn page_size(max_results: Option<i32>, modelled_max: i32) -> usize {
    let requested = max_results.unwrap_or(modelled_max);
    requested.clamp(1, modelled_max) as usize
}

/// A fresh `^[a-zA-Z0-9]{22}$` resource id for `seq`.
///
/// The id is the per-run sequence number mixed through SplitMix64 — a published
/// 64-bit mixer — and spread over the alphabet the modelled pattern admits, so
/// consecutive ids neither repeat nor share a prefix and no random source is
/// needed. Uniqueness within a run is the sequence's; the callers above still
/// check the store, so a collision cannot overwrite a live resource.
fn id_from_seq(seq: u64) -> String {
    fn mix(state: &mut u64) -> u64 {
        *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = *state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    let base = RESOURCE_ID_ALPHABET.len() as u64;
    // The most characters one mixed word can carry while `base^n` still fits in
    // a u64: ten for this 62-symbol alphabet.
    let chars_per_word = {
        let mut chars = 0usize;
        let mut capacity = 1u64;
        while capacity <= u64::MAX / base {
            capacity *= base;
            chars += 1;
        }
        chars
    };

    let mut state = seq;
    let mut id = String::with_capacity(RESOURCE_ID_LEN);
    while id.len() < RESOURCE_ID_LEN {
        let mut word = mix(&mut state);
        for _ in 0..chars_per_word {
            if id.len() == RESOURCE_ID_LEN {
                break;
            }
            let digit = (word % base) as usize;
            id.push(char::from(RESOURCE_ID_ALPHABET[digit]));
            word /= base;
        }
    }
    id
}

/// A modelled `Timestamp` for a stored instant.
pub fn to_date_time(time: SystemTime) -> ::aws_smithy_types::DateTime {
    let secs = time
        .duration_since(UNIX_EPOCH)
        .map_or(0.0, |elapsed| elapsed.as_secs_f64());
    ::aws_smithy_types::DateTime::from_secs_f64(secs)
}

/// The region a request was signed for, from its SigV4 credential scope
/// (`Credential=<akid>/<date>/<region>/<service>/aws4_request`), so an ARN this
/// service mints carries the region the caller addressed. Falls back to
/// [`DEFAULT_REGION`] for a request that carries no SigV4 scope.
///
/// Parsing is `emulator_server::sigv4_credential_scope`, the one source of
/// SigV4-scope parsing the routing and telemetry paths already read.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsVoiceidVoiceidState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsVoiceidVoiceidState>)
}
