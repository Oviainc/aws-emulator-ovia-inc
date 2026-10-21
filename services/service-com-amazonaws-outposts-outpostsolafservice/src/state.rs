// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-outposts-outpostsolafservice service, plus
//! the rules more than one operation in the site + Outpost lifecycle applies: id
//! and ARN minting, resolving the `id OR ARN` identifier forms the modelled
//! `@httpLabel` patterns accept, and cutting an ordered list into pages with the
//! modelled `Token` the next page resumes from.

use std::collections::{BTreeMap, HashMap};

use sha2::{Digest, Sha256};

use crate::ops::types::{Address, Outpost, Site};

/// The region every minted ARN falls back to when the request carries no SigV4
/// credential scope to read one from (an unsigned, anonymous call).
pub const DEFAULT_REGION: &str = "us-east-1";

/// The ARN namespace of this service, as the model's `awsService` trait declares
/// it (`arnNamespace: outposts`) — the third segment of every ARN it mints.
pub const ARN_NAMESPACE: &str = "outposts";

/// The `LifeCycleStatus` a freshly created Outpost carries.
///
/// An Outpost is the logical container AWS creates synchronously; capacity arrives
/// later through an order, which is a separate resource with its own status. So
/// `CreateOutpost` answers with a ready Outpost and there is no clock-owed
/// transition to a later status. The model constrains `LifeCycleStatus` without
/// declaring a value set (len 1..20, pattern `^[ A-Za-z]+$`); `ACTIVE` is the
/// status the AWS API reference shows a created Outpost in, and it satisfies both
/// constraints.
pub const LIFE_CYCLE_STATUS_ACTIVE: &str = "ACTIVE";

/// A persisted site: the modelled [`Site`] this service answers with, plus the two
/// whole [`Address`] values `Site` itself does not carry (it exposes only the
/// operating address's country, state and city) and the creation order every list
/// pages in.
#[derive(Debug, Clone)]
pub struct SiteRecord {
    /// Position in this store's creation order — the sort key every list and every
    /// page token is expressed in, so a token stays correct across calls.
    pub sequence: u64,
    /// The site as `CreateSite` / `GetSite` / `ListSites` return it.
    pub site: Site,
    /// The full operating address the caller supplied, which `Site` projects down
    /// to `OperatingAddress{CountryCode,StateOrRegion,City}`.
    pub operating_address: Option<Address>,
    /// The full shipping address the caller supplied. `Site` carries no shipping
    /// member at all, so this is the only place it is kept.
    pub shipping_address: Option<Address>,
}

/// A persisted Outpost: the modelled [`Outpost`] plus its creation order.
#[derive(Debug, Clone)]
pub struct OutpostRecord {
    /// See [`SiteRecord::sequence`].
    pub sequence: u64,
    /// The Outpost as `CreateOutpost` / `GetOutpost` / `ListOutposts` return it.
    pub outpost: Outpost,
}

/// Per-service state for com-amazonaws-outposts-outpostsolafservice. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsOutpostsOutpostsolafserviceState {
    /// Every persisted site, keyed by its bare `SiteId` (`os-…`), which is how
    /// `GetSite`, `CreateOutpost` and `DeleteSite` resolve one.
    pub sites: BTreeMap<String, SiteRecord>,
    /// Every persisted Outpost, keyed by its bare `OutpostId` (`op-…`).
    pub outposts: BTreeMap<String, OutpostRecord>,
    /// How many resources this store has created. Doubles as the monotonically
    /// increasing creation order every record records and every page token names,
    /// and as part of each minted id's seed so no two ids collide.
    pub created: u64,
}

impl ComAmazonawsOutpostsOutpostsolafserviceState {
    /// Mint a fresh resource id of the form `<prefix>-<17 lowercase hex chars>`,
    /// and return it with the creation sequence to record on the new resource.
    ///
    /// Both `OutpostId` (`^op-[a-f0-9]{17}$`) and `SiteId` (`^os-[a-f0-9]{17}$`)
    /// are a two-letter prefix and exactly 17 lowercase hex characters, so one
    /// helper serves both. `created` advances on every call and is hashed into the
    /// seed with the wall clock and the process id, so repeated calls never return
    /// the same id.
    pub fn mint_id(&mut self, prefix: &str) -> (String, u64) {
        self.created = self.created.wrapping_add(1);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_nanos());
        let seed = format!("{prefix}|{}|{}|{nanos}", self.created, std::process::id());
        let digest = Sha256::digest(seed.as_bytes());
        let mut hex = hex::encode(&digest[..9]);
        // 9 bytes encode to 18 characters; the patterns want exactly 17.
        hex.truncate(17);
        (format!("{prefix}-{hex}"), self.created)
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsOutpostsOutpostsolafserviceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsOutpostsOutpostsolafserviceState>)
}

/// The region to mint this request's ARNs in: the region of its SigV4 credential
/// scope, or [`DEFAULT_REGION`] when the request is unsigned.
///
/// A scope region is honoured only when it can appear in an Outposts ARN at all —
/// the modelled `SiteArn` / `OutpostArn` patterns pin that segment to `[a-z\d-]+`
/// — so a hostile or nonsense scope can never make this service mint an ARN its
/// own model rejects.
pub fn request_region(parts: &http::request::Parts) -> String {
    emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .filter(|region| {
            !region.is_empty()
                && region
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        })
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// A `SiteArn`: `arn:aws:outposts:<region>:<account>:site/os-…`.
pub fn site_arn(region: &str, site_id: &str) -> String {
    ::engine::emulator_arn(ARN_NAMESPACE, region, &format!("site/{site_id}"))
}

/// An `OutpostArn`: `arn:aws:outposts:<region>:<account>:outpost/op-…`.
pub fn outpost_arn(region: &str, outpost_id: &str) -> String {
    ::engine::emulator_arn(ARN_NAMESPACE, region, &format!("outpost/{outpost_id}"))
}

/// The bare resource id an Outposts identifier names, whether it arrived as the id
/// or as the full ARN.
///
/// Every identifier member in this API is modelled
/// `^(arn:…:<kind>/)?(<prefix>-[a-f0-9]{17})$` — the ARN prefix is optional — so one
/// resolver serves `SiteId`, `OutpostId` and the ARN forms alike: take the segment
/// after the last `/` when the value is an ARN, then require the bare id to be the
/// `<prefix>-` plus exactly 17 lowercase hex characters the pattern allows.
/// `None` means the value is not an identifier this service could have minted.
pub fn resolve_id(identifier: &str, prefix: &str) -> Option<String> {
    let bare = match identifier.starts_with("arn:") {
        true => identifier.rsplit('/').next()?,
        false => identifier,
    };
    let suffix = bare.strip_prefix(prefix)?.strip_prefix('-')?;
    let well_formed = suffix.len() == 17
        && suffix
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
    well_formed.then(|| bare.to_string())
}

/// The `os-…` id a `SiteId` names. See [`resolve_id`].
pub fn resolve_site_id(identifier: &str) -> Option<String> {
    resolve_id(identifier, "os")
}

/// The `op-…` id an `OutpostId` names. See [`resolve_id`].
pub fn resolve_outpost_id(identifier: &str) -> Option<String> {
    resolve_id(identifier, "op")
}

/// The `Site` projection of an operating address: the three members `Site`
/// declares for it, each read off the address the caller supplied.
pub fn operating_address_projection(
    address: Option<&Address>,
) -> (Option<String>, Option<String>, Option<String>) {
    match address {
        Some(address) => (
            Some(address.country_code.clone()),
            Some(address.state_or_region.clone()),
            Some(address.city.clone()),
        ),
        None => (None, None, None),
    }
}

/// A tag map as a response member: `None` for a resource with no tags.
///
/// The modelled `TagMap` carries `length(min: 1)`, so an empty map is not a value
/// this API's own model admits — an untagged resource omits the member instead.
pub fn tags_member(tags: &HashMap<String, String>) -> Option<HashMap<String, String>> {
    (!tags.is_empty()).then(|| tags.clone())
}

/// A string member the caller may have sent as the empty string, which carries no
/// value: every optional string in this API is modelled `length(min: 1)` except
/// `OutpostDescription`, so an empty one is omitted rather than stored.
pub fn non_empty(value: Option<String>) -> Option<String> {
    value.filter(|value| !value.is_empty())
}

/// The stamp a `ListOutposts` page token carries, so a token issued for one list
/// cannot be replayed against another.
pub const OUTPOST_PAGE: &str = "outposts";

/// The stamp a `ListSites` page token carries. See [`OUTPOST_PAGE`].
pub const SITE_PAGE: &str = "sites";

/// The page size a list uses when the caller named no `MaxResults`, which is also
/// the ceiling the model declares for it (`MaxResults1000`, `range(1..1000)`).
pub const MAX_RESULTS: i32 = 1000;

/// The ceiling on how many sites, and how many Outposts, this store holds.
///
/// A generated emulator keeps its resources in memory for the life of the process
/// and a create is the only thing that adds one, so the store needs a maximum entry
/// count: neither a TTL nor an eviction policy is available to it, because `GetSite`
/// must still answer what `CreateSite` created. Past the cap the two creating
/// operations answer the `ServiceQuotaExceededException` (HTTP 402) this service
/// models on exactly them and on no read. The number is the emulator's own bound,
/// not a published AWS default — AWS publishes both quotas as adjustable, so no
/// single value is the contract — and is set high enough that no ordinary workflow
/// meets it.
pub const MAX_RESOURCES: usize = 5_000;

/// Encode a page boundary as an opaque `Token`.
///
/// The token carries the creation sequence of the LAST item the page returned, so
/// the next page is everything created strictly after it — a resume point that
/// stays correct when items are added or removed between pages, which a positional
/// offset would not. It is STAMPED with the list it belongs to, so a token replayed
/// against another list is refused rather than honoured as that list's boundary.
/// The rendering is exactly the modelled `Token` pattern `^(\d+)##(\S+)$`.
pub fn encode_page_token(kind: &str, last_sequence: u64) -> String {
    format!("{last_sequence}##{kind}")
}

/// The page boundary a `kind` token names, or `None` when it is not a token this
/// service issued for that list — which the caller is told as a
/// `ValidationException`, never as a silently truncated page.
pub fn decode_page_token(kind: &str, token: &str) -> Option<u64> {
    let (sequence, stamp) = token.split_once("##")?;
    if stamp != kind {
        return None;
    }
    sequence.parse().ok()
}

/// The page size a list serves, given the `MaxResults` the caller sent.
///
/// `MaxResults1000` is modelled `range(1..1000)`, so the framework has already
/// refused anything outside it; an absent value means the whole list up to that
/// same ceiling.
pub fn page_size(max_results: Option<i32>) -> usize {
    usize::try_from(max_results.unwrap_or(MAX_RESULTS))
        .unwrap_or(MAX_RESULTS as usize)
        .clamp(1, MAX_RESULTS as usize)
}

/// Cut one page out of an already-filtered sequence of records, and issue the token
/// that resumes after it.
///
/// Shared by both list operations so they cannot drift apart: the records are
/// ordered by creation sequence, one more than the page would fill is what proves a
/// further page exists, and the token names the last sequence this page returned.
pub fn page<T>(
    kind: &str,
    records: impl Iterator<Item = (u64, T)>,
    after: Option<u64>,
    size: usize,
) -> (Vec<T>, Option<String>) {
    let mut ordered: Vec<(u64, T)> = records
        .filter(|(sequence, _)| after.is_none_or(|boundary| *sequence > boundary))
        .collect();
    ordered.sort_by_key(|(sequence, _)| *sequence);
    let next_token = match ordered.len() > size {
        true => {
            ordered.truncate(size);
            ordered
                .last()
                .map(|(sequence, _)| encode_page_token(kind, *sequence))
        }
        false => None,
    };
    (
        ordered.into_iter().map(|(_, item)| item).collect(),
        next_token,
    )
}
