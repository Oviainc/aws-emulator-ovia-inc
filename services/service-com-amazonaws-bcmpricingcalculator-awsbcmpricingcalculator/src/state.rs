// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-bcmpricingcalculator-awsbcmpricingcalculator service.
//!
//! The bill scenario is this service's root resource: a bill estimate is derived from a
//! scenario, and a usage modification only exists inside one, so every modification is
//! stored on its parent scenario rather than in a second map that could drift from it.

use std::collections::{BTreeMap, HashMap};

use ::aws_smithy_types::DateTime;

use crate::ops::types::{BillInterval, HistoricalUsageEntity, UsageAmount, UsageQuantity};

/// The region an ARN carries when the request's SigV4 credential scope names none.
/// `engine::emulator_arn`'s contract: "emulators default to us-east-1".
pub const DEFAULT_REGION: &str = "us-east-1";

/// This service's ARN namespace, as the Smithy model's `aws.api#service` declares it
/// (`arnNamespace: bcm-pricing-calculator`).
pub const ARN_NAMESPACE: &str = "bcm-pricing-calculator";

/// How long after creation a bill scenario reports it will expire.
///
/// CHOSEN, not documented: neither the Smithy model nor the API reference states a
/// retention span for a bill scenario, so this emulator picks one, reports it
/// consistently through `expiresAt` on create, get and list, and enforces it in
/// [`ComAmazonawsBcmpricingcalculatorAwsbcmpricingcalculatorState::purge_expired`].
pub const BILL_SCENARIO_RETENTION_SECS: i64 = 15 * 24 * 60 * 60;

/// The page size a `@paginated` list operation uses when the caller sends no
/// `maxResults`: the model's own maximum for that member (`range 1..25`).
pub const MAX_PAGE_SIZE: i32 = 25;

/// The greatest number of bill scenarios this emulator holds at once.
///
/// CHOSEN, not documented: AWS publishes no bill-scenario quota in the model or the
/// API reference. A cap is still required rather than optional, for two reasons: the
/// model declares `ServiceQuotaExceededException` (HTTP 402, "The request would cause
/// you to exceed your service quota") on `CreateBillScenario`, which is unreachable
/// without one; and without it a caller could grow a long-lived emulator's store
/// without bound.
pub const MAX_BILL_SCENARIOS: usize = 1000;

/// The greatest number of usage modifications ONE bill scenario holds at once. Chosen
/// for the same two reasons as [`MAX_BILL_SCENARIOS`], and answered with the same
/// modelled `ServiceQuotaExceededException`.
pub const MAX_USAGE_MODIFICATIONS_PER_BILL_SCENARIO: usize = 2000;

/// One stored usage modification of a bill scenario.
#[derive(Debug, Clone)]
pub struct UsageModification {
    /// The server-assigned `ResourceId` (a lowercase-hex v4 UUID).
    pub id: String,
    /// The `key` the batch entry that created this modification carried, so a later
    /// batch response can report an error against the same key.
    pub key: Option<String>,
    pub service_code: String,
    pub usage_type: String,
    pub operation: String,
    pub usage_account_id: String,
    pub availability_zone: Option<String>,
    /// The pricing location of this line. Only resolvable from the pricing catalog,
    /// which this emulator does not carry, so it stays unset.
    pub location: Option<String>,
    pub group: Option<String>,
    pub amounts: Vec<UsageAmount>,
    pub historical_usage: Option<HistoricalUsageEntity>,
}

impl UsageModification {
    /// The modified usage quantities, one per requested `UsageAmount`.
    ///
    /// `unit` stays unset: a usage type's unit of measure comes from the pricing
    /// catalog, which this emulator does not carry, and `UsageQuantity$unit` is
    /// optional precisely so it can be absent.
    pub fn quantities(&self) -> Option<Vec<UsageQuantity>> {
        if self.amounts.is_empty() {
            return None;
        }
        Some(
            self.amounts
                .iter()
                .map(|amount| UsageQuantity {
                    start_hour: Some(amount.start_hour),
                    unit: None,
                    amount: Some(amount.amount),
                })
                .collect(),
        )
    }

    /// The value a `ListUsageFilter` name selects on this modification, or `None` when
    /// this modification carries nothing for it.
    ///
    /// Names are the `ListUsageFilterName` enum's own values (smithy_query:
    /// `shape="ListUsageFilterName"`); the `HISTORICAL_*` half reads the entry's
    /// `historicalUsage`.
    pub fn filter_value(&self, name: &str) -> Option<&str> {
        let historical = self.historical_usage.as_ref();
        match name {
            "USAGE_ACCOUNT_ID" => Some(self.usage_account_id.as_str()),
            "SERVICE_CODE" => Some(self.service_code.as_str()),
            "USAGE_TYPE" => Some(self.usage_type.as_str()),
            "OPERATION" => Some(self.operation.as_str()),
            "LOCATION" => self.location.as_deref(),
            "USAGE_GROUP" => self.group.as_deref(),
            "HISTORICAL_USAGE_ACCOUNT_ID" => historical.map(|h| h.usage_account_id.as_str()),
            "HISTORICAL_SERVICE_CODE" => historical.map(|h| h.service_code.as_str()),
            "HISTORICAL_USAGE_TYPE" => historical.map(|h| h.usage_type.as_str()),
            "HISTORICAL_OPERATION" => historical.map(|h| h.operation.as_str()),
            "HISTORICAL_LOCATION" => historical.and_then(|h| h.location.as_deref()),
            _ => None,
        }
    }
}

/// One stored bill scenario.
#[derive(Debug, Clone)]
pub struct BillScenario {
    /// The server-assigned `ResourceId` (a lowercase-hex v4 UUID).
    pub id: String,
    /// `arn:aws:bcm-pricing-calculator:<region>:<account>:bill-scenario/<id>`, built
    /// through `engine::emulator_arn` so the account segment is the canonical one.
    ///
    /// Stored, not yet reported: the operations that address a scenario by ARN
    /// (`TagResource`, `UntagResource`, `ListTagsForResource`) are still 501 and own
    /// their own issue, so this is what they will resolve against.
    pub arn: String,
    pub name: String,
    /// A `BillScenarioStatus` value (`READY` | `LOCKED` | `STALE` | `FAILED`).
    pub status: String,
    pub created_at: DateTime,
    pub expires_at: DateTime,
    pub bill_interval_start: DateTime,
    pub bill_interval_end: DateTime,
    pub failure_message: Option<String>,
    pub group_sharing_preference: Option<String>,
    pub cost_category_group_sharing_preference_arn: Option<String>,
    pub tags: BTreeMap<String, String>,
    /// Insertion-ordered, so a paginated read of this scenario's modifications is
    /// stable across calls.
    pub usage_modifications: Vec<UsageModification>,
}

impl BillScenario {
    /// The time period this scenario covers, as the modelled structure.
    pub fn bill_interval(&self) -> BillInterval {
        BillInterval {
            start: Some(self.bill_interval_start),
            end: Some(self.bill_interval_end),
        }
    }

    /// The value a `ListBillScenariosFilter` name selects on this scenario, or `None`
    /// when this scenario carries nothing for it.
    ///
    /// Names are the `ListBillScenariosFilterName` enum's own values (smithy_query:
    /// `shape="ListBillScenariosFilterName"`).
    pub fn filter_value(&self, name: &str) -> Option<&str> {
        match name {
            "STATUS" => Some(self.status.as_str()),
            "NAME" => Some(self.name.as_str()),
            "GROUP_SHARING_PREFERENCE" => self.group_sharing_preference.as_deref(),
            "COST_CATEGORY_ARN" => self.cost_category_group_sharing_preference_arn.as_deref(),
            _ => None,
        }
    }

    /// The modification with this id, if this scenario holds it.
    pub fn usage_modification(&self, id: &str) -> Option<&UsageModification> {
        self.usage_modifications.iter().find(|m| m.id == id)
    }
}

/// What a previous request carrying an `@idempotencyToken` did.
#[derive(Debug, Clone)]
pub struct IdempotentRecord {
    /// A digest of the request that made this record, so the same token replayed with a
    /// DIFFERENT request is recognised as a conflict rather than answered with this
    /// request's result.
    pub fingerprint: String,
    /// The resource ids that request created.
    pub ids: Vec<String>,
}

/// What [`ComAmazonawsBcmpricingcalculatorAwsbcmpricingcalculatorState::replay`] found
/// for a request's `@idempotencyToken`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Replay {
    /// No record: a first request, or no token at all.
    Fresh,
    /// This exact request was already served; these are the ids it created.
    Replayed(Vec<String>),
    /// This token was already used for a DIFFERENT request; these are the ids that
    /// request created, so the conflict can name the resource it already made.
    Mismatch(Vec<String>),
}

/// Per-service state for com-amazonaws-bcmpricingcalculator-awsbcmpricingcalculator. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsBcmpricingcalculatorAwsbcmpricingcalculatorState {
    /// Every live bill scenario, in creation order so a paginated read is stable.
    pub bill_scenarios: Vec<BillScenario>,
    /// `<operation>:<clientToken>` -> what that request created, so a replayed
    /// `@idempotencyToken` answers with the first result instead of minting a second
    /// resource. The key carries the operation name because the same token may
    /// legitimately be reused for a different operation.
    pub idempotency: HashMap<String, IdempotentRecord>,
}

impl ComAmazonawsBcmpricingcalculatorAwsbcmpricingcalculatorState {
    /// The scenario with this id, or `None` when it was never created or was deleted.
    pub fn bill_scenario(&self, id: &str) -> Option<&BillScenario> {
        self.bill_scenarios.iter().find(|s| s.id == id)
    }

    /// The scenario with this id, mutably.
    pub fn bill_scenario_mut(&mut self, id: &str) -> Option<&mut BillScenario> {
        self.bill_scenarios.iter_mut().find(|s| s.id == id)
    }

    /// One fresh `ResourceId`, as [`Self::mint_resource_ids`] mints a batch of them.
    pub fn mint_resource_id(&self) -> String {
        // `mint_resource_ids` returns exactly as many ids as it is asked for, so the
        // fallback is unreachable; it is here so a handler needs no unwrap.
        self.mint_resource_ids(1)
            .pop()
            .unwrap_or_else(candidate_resource_id)
    }

    /// `count` fresh `ResourceId`s, none of which a live scenario, a live usage
    /// modification, or another id of this batch already carries.
    ///
    /// The live-id set is built ONCE per call rather than per candidate, so minting a
    /// batch costs one pass over the store instead of one per row. The rejection loop
    /// is what makes uniqueness exact: a candidate the store already holds is discarded
    /// rather than overwriting somebody's resource, so a caller never loses a row to a
    /// repeated id.
    pub fn mint_resource_ids(&self, count: usize) -> Vec<String> {
        let mut live: ::std::collections::HashSet<&str> = ::std::collections::HashSet::new();
        for scenario in &self.bill_scenarios {
            live.insert(scenario.id.as_str());
            live.extend(
                scenario
                    .usage_modifications
                    .iter()
                    .map(|modification| modification.id.as_str()),
            );
        }
        let mut minted: Vec<String> = Vec::with_capacity(count);
        while minted.len() < count {
            let candidate = candidate_resource_id();
            if !live.contains(candidate.as_str()) && !minted.contains(&candidate) {
                minted.push(candidate);
            }
        }
        minted
    }

    /// Drop the scenario with this id (and, with it, its usage modifications).
    /// `true` when one was removed.
    pub fn remove_bill_scenario(&mut self, id: &str) -> bool {
        let before = self.bill_scenarios.len();
        self.bill_scenarios.retain(|s| s.id != id);
        let removed = before != self.bill_scenarios.len();
        if removed {
            self.prune_idempotency();
        }
        removed
    }

    /// Materialise the expiry the clock has already made due: drop every scenario whose
    /// `expiresAt` has passed, and the idempotency entries that named it.
    ///
    /// The expiry is owed at `now_secs` whether or not anybody called, so even a
    /// reporting operation may apply it. Without it the retention window every reader
    /// advertises through `expiresAt` would never be enforced, and the store would grow
    /// for the life of the process.
    pub fn purge_expired(&mut self, now_secs: i64) {
        let before = self.bill_scenarios.len();
        self.bill_scenarios
            .retain(|scenario| scenario.expires_at.secs() > now_secs);
        if before != self.bill_scenarios.len() {
            self.prune_idempotency();
        }
    }

    /// Forget every resource id an idempotency record names that no longer exists, and
    /// drop a record left naming nothing.
    ///
    /// A consequence worth stating: once the resources a token created are deleted (or
    /// expired), that token no longer replays, so the same `clientToken` sent again
    /// creates a new resource rather than answering with the vanished one. The
    /// alternative — keeping records for a retention period of their own — would let a
    /// caller grow this map without bound by creating and deleting under fresh tokens,
    /// and would have a replay promise nothing can honour, since the resource it names
    /// is gone.
    ///
    /// Pruning per id rather than per record keeps the record and the replay it
    /// promises in agreement: a replay answers with exactly the resources that still
    /// exist, instead of falling through (because one id of a batch vanished) and
    /// minting a second set under the same token. And a record naming nothing could
    /// never answer a replay at all, so keeping it would only let a caller grow this
    /// map by repeatedly creating and deleting under fresh tokens.
    pub fn prune_idempotency(&mut self) {
        // Nothing to prune, and the live-id set is the expensive part: the delete and
        // expiry paths reach here on every call.
        if self.idempotency.is_empty() {
            return;
        }
        // Destructured so the live set can BORROW the scenario ids while the records
        // are mutated: they are disjoint fields, which a whole-`self` borrow would not
        // let the compiler see.
        let Self {
            bill_scenarios,
            idempotency,
        } = self;
        let live: std::collections::HashSet<&str> = bill_scenarios
            .iter()
            .flat_map(|scenario| {
                std::iter::once(scenario.id.as_str()).chain(
                    scenario
                        .usage_modifications
                        .iter()
                        .map(|modification| modification.id.as_str()),
                )
            })
            .collect();
        idempotency.retain(|_, record| {
            record.ids.retain(|id| live.contains(id.as_str()));
            !record.ids.is_empty()
        });
    }

    /// What a request's `@idempotencyToken` says about this request: a first request, a
    /// genuine retry of the same one, or the same token used for a different one.
    pub fn replay(
        &self,
        operation: &str,
        client_token: Option<&str>,
        fingerprint: &str,
    ) -> Replay {
        let Some(token) = client_token else {
            return Replay::Fresh;
        };
        match self.idempotency.get(&idempotency_key(operation, token)) {
            None => Replay::Fresh,
            Some(record) if record.fingerprint == fingerprint => {
                Replay::Replayed(record.ids.clone())
            }
            Some(record) => Replay::Mismatch(record.ids.clone()),
        }
    }

    /// Record what this request created against its `@idempotencyToken`.
    ///
    /// A request that carried no token, or created nothing, records nothing: a record
    /// naming no resource could never answer a replay and would never be pruned.
    pub fn record_idempotent(
        &mut self,
        operation: &str,
        client_token: Option<&str>,
        fingerprint: &str,
        ids: Vec<String>,
    ) {
        if ids.is_empty() {
            return;
        }
        if let Some(token) = client_token {
            self.idempotency.insert(
                idempotency_key(operation, token),
                IdempotentRecord {
                    fingerprint: fingerprint.to_string(),
                    ids,
                },
            );
        }
    }
}

fn idempotency_key(operation: &str, client_token: &str) -> String {
    format!("{operation}:{client_token}")
}

/// A stable digest of the request parts an `@idempotencyToken` is scoped to, and of
/// the selection a page of results was measured against.
///
/// It is what lets a genuine retry (the same token AND the same request) be told from a
/// token reused for a different request, which is a conflict rather than a replay. The
/// parts are length-prefixed, so `["a", "bc"]` and `["ab", "c"]` do not collide.
///
/// Digesting rather than keeping the parts matters for the second use: the digest
/// travels inside a `NextPageToken`, which the model caps at 2048 characters, while a
/// request's `filters` are unbounded.
pub fn request_fingerprint<S: AsRef<str>>(parts: &[S]) -> String {
    use sha2::Digest;

    let mut digest = sha2::Sha256::new();
    for part in parts {
        let part = part.as_ref();
        digest.update((part.len() as u64).to_le_bytes());
        digest.update(part.as_bytes());
    }
    hex::encode(digest.finalize())
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsBcmpricingcalculatorAwsbcmpricingcalculatorState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsBcmpricingcalculatorAwsbcmpricingcalculatorState>)
}

/// Wall-clock seconds since the Unix epoch.
pub fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs() as i64)
}

/// A candidate `ResourceId`: the lowercase-hex v4 UUID form this service's
/// `ResourceId` shape declares
/// (`^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$`, len 36..36), with
/// RFC 9562's version nibble and variant bits set, so it is a well-formed v4 value and
/// not merely 32 hex characters.
///
/// The 128 bits come from the standard library's `RandomState`, which the OS seeds, and
/// a process-wide sequence number so two ids minted in the same nanosecond differ.
/// Uniqueness is then made EXACT rather than probabilistic by
/// [`ComAmazonawsBcmpricingcalculatorAwsbcmpricingcalculatorState::mint_resource_id`],
/// which rejects a candidate the store already holds — the property the service needs,
/// since a repeated id would overwrite a caller's resource.
fn candidate_resource_id() -> String {
    use ::std::hash::{BuildHasher, Hasher};
    use ::std::sync::atomic::{AtomicU64, Ordering};

    static SEQUENCE: AtomicU64 = AtomicU64::new(0);

    let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let nanos = ::std::time::SystemTime::now()
        .duration_since(::std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos());
    let half = |salt: u64| -> u64 {
        let mut hasher = ::std::collections::hash_map::RandomState::new().build_hasher();
        hasher.write_u64(salt);
        hasher.write_u64(sequence);
        hasher.write_u128(nanos);
        hasher.finish()
    };
    // The version nibble (4) and the two variant bits (0b10) are the only fixed bits of
    // a v4 UUID; everything else is random.
    let high = (half(0) & 0xffff_ffff_ffff_0fff) | 0x0000_0000_0000_4000;
    let low = (half(1) & 0x3fff_ffff_ffff_ffff) | 0x8000_0000_0000_0000;
    format!(
        "{:08x}-{:04x}-{:04x}-{:04x}-{:012x}",
        high >> 32,
        (high >> 16) & 0xffff,
        high & 0xffff,
        low >> 48,
        low & 0xffff_ffff_ffff,
    )
}

/// The UTC calendar month containing `secs`, as `[start, end)` epoch seconds — the
/// billing period a bill scenario covers.
pub fn month_bounds(secs: i64) -> (i64, i64) {
    use ::aws_smithy_types::date_time::Format;

    let bounds = || -> Option<(i64, i64)> {
        // The calendar reading and the conversion back both come from
        // `aws_smithy_types::DateTime`, the crate this service already serialises its
        // timestamps with: `Format::DateTime` renders the instant as RFC 3339
        // (`2026-10-22T09:30:00Z`), whose first seven characters are the calendar year
        // and month, and `DateTime::from_str` turns the first instant of a month back
        // into epoch seconds. The only arithmetic written here is the month rollover.
        let text = DateTime::from_secs(secs).fmt(Format::DateTime).ok()?;
        let year: i32 = text.get(..4)?.parse().ok()?;
        let month: u32 = text.get(5..7)?.parse().ok()?;
        let first_instant_of = |year: i32, month: u32| -> Option<i64> {
            let text = format!("{year:04}-{month:02}-01T00:00:00Z");
            Some(DateTime::from_str(&text, Format::DateTime).ok()?.secs())
        };
        let (next_year, next_month) = if month == 12 {
            (year + 1, 1)
        } else {
            (year, month + 1)
        };
        Some((
            first_instant_of(year, month)?,
            first_instant_of(next_year, next_month)?,
        ))
    };
    // An instant no calendar can render (a year outside RFC 3339's four digits) has no
    // calendar month; report the instant itself rather than a fabricated interval.
    bounds().unwrap_or((secs, secs))
}

/// The region this request was signed for, defaulting to [`DEFAULT_REGION`].
///
/// The value comes from the caller's SigV4 credential scope, so it is only accepted
/// when it has the shape of a region (lowercase alphanumerics and hyphens, 1..32
/// characters); anything else falls back to the default rather than being interpolated
/// into an ARN the real service could never emit.
pub fn request_region(headers: &::http::HeaderMap) -> String {
    ::emulator_server::sigv4_credential_scope(headers)
        .map(|scope| scope.region)
        .filter(|region| {
            (1..=32).contains(&region.len())
                && region
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        })
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// Whether `candidate` satisfies a filter's `values` under its `matchOption`.
///
/// The `MatchOption` enum's values are `EQUALS` | `STARTS_WITH` | `CONTAINS`
/// (smithy_query: `shape="MatchOption"`); a filter that sends no `matchOption` is an
/// exact match. The values of one filter are OR-ed.
pub fn matches_filter(candidate: &str, values: &[String], match_option: Option<&str>) -> bool {
    values.iter().any(|value| match match_option {
        Some("STARTS_WITH") => candidate.starts_with(value.as_str()),
        Some("CONTAINS") => candidate.contains(value.as_str()),
        _ => candidate == value.as_str(),
    })
}

/// Whether `moment` falls inside a `FilterTimestamp` window, which is inclusive at
/// both ends and open at an end the caller left unset.
///
/// The inclusivity of each bound is CHOSEN, not documented: `FilterTimestamp`'s members
/// say only "Include results after/before this timestamp" (smithy_query:
/// `shape="FilterTimestamp"`), which does not settle whether the bound itself is
/// included. Both bounds are inclusive here, consistently for every window this service
/// applies, and that choice is pinned by a test row on each boundary instant.
pub fn within_window(
    moment: DateTime,
    after: Option<DateTime>,
    before: Option<DateTime>,
) -> bool {
    let key = (moment.secs(), moment.subsec_nanos());
    if let Some(after) = after {
        if key < (after.secs(), after.subsec_nanos()) {
            return false;
        }
    }
    if let Some(before) = before {
        if key > (before.secs(), before.subsec_nanos()) {
            return false;
        }
    }
    true
}

/// The prefix every pagination token this service emits carries, so a token minted by
/// another service (or a hand-written one) is rejected rather than silently read as an
/// offset.
const PAGE_TOKEN_PREFIX: &str = "bcmpc-offset:";

/// What a caller is told about a token this service did not issue for THIS request.
const UNISSUED_PAGE_TOKEN: &str = "nextToken is not a token this service issued for this request";

/// The opaque `nextToken` naming the item at `offset` in an insertion-ordered
/// collection, bound to `scope`.
///
/// `scope` is a [`request_fingerprint`] of the request the page belongs to (its
/// operation and the selection it asked for), so an offset is only ever applied to the
/// list it was measured against: a token from another operation, or from the same
/// operation under different `filters`, is refused rather than silently skipping or
/// repeating items.
pub fn encode_page_token(scope: &str, offset: usize) -> String {
    hex::encode(format!("{PAGE_TOKEN_PREFIX}{scope}:{offset}"))
}

/// The offset [`encode_page_token`] encoded for `scope`, or `Err` with the reason the
/// token is not one this service issued for this request.
///
/// A token is meaningful only to the process that minted it: the offset indexes an
/// in-memory collection, so it is refused anywhere else, which is the safe direction.
pub fn decode_page_token(token: &str, scope: &str) -> Result<usize, String> {
    let decoded = hex::decode(token).map_err(|_| UNISSUED_PAGE_TOKEN.to_string())?;
    let text = String::from_utf8(decoded).map_err(|_| UNISSUED_PAGE_TOKEN.to_string())?;
    let body = text
        .strip_prefix(PAGE_TOKEN_PREFIX)
        .ok_or_else(|| UNISSUED_PAGE_TOKEN.to_string())?;
    let (token_scope, offset) = body
        .rsplit_once(':')
        .ok_or_else(|| UNISSUED_PAGE_TOKEN.to_string())?;
    if token_scope != scope {
        return Err(UNISSUED_PAGE_TOKEN.to_string());
    }
    offset
        .parse::<usize>()
        .map_err(|_| UNISSUED_PAGE_TOKEN.to_string())
}

/// One page of `items`, starting at the offset `next_token` names.
///
/// `max_results` is the caller's page size, defaulting to the model's maximum, and
/// `scope` binds the token to this request (see [`encode_page_token`]). The returned
/// token is `Some` only while items remain after this page, so a caller that paginates
/// to the end sees it absent — and an offset past the end is an empty last page rather
/// than an error, because the item a live token named may have been deleted in between.
pub fn paginate<T: Clone>(
    items: &[T],
    next_token: Option<&str>,
    max_results: Option<i32>,
    scope: &str,
) -> Result<(Vec<T>, Option<String>), String> {
    let offset = match next_token {
        Some(token) if !token.is_empty() => decode_page_token(token, scope)?,
        _ => 0,
    };
    // The model declares `maxResults` as `range 1..25`, so the boundary validator has
    // already refused anything outside it; refuse rather than clamp, so a page size
    // that somehow reached here is reported instead of silently becoming 1.
    let page_size = match max_results {
        Some(requested) if requested < 1 => {
            return Err(format!("maxResults must be at least 1, got {requested}"))
        }
        Some(requested) => requested as usize,
        None => MAX_PAGE_SIZE as usize,
    };
    if offset >= items.len() {
        return Ok((Vec::new(), None));
    }
    let end = (offset + page_size).min(items.len());
    let page = items[offset..end].to_vec();
    let token = (end < items.len()).then(|| encode_page_token(scope, end));
    Ok((page, token))
}
