// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-route53-awsdnsv20130401 service.
//!
//! Route 53 is a control plane over two resources: the hosted zone and, inside
//! it, the resource record set. Both live here, in ONE store, so every operation
//! reads the records another operation wrote. The helpers in this module are the
//! shared vocabulary the handlers in `src/ops/` use for them — DNS name
//! normalization, the sort key `ListResourceRecordSets` orders by, the
//! `/hostedzone/` and `/change/` id prefixes the API wraps ids in, and the id
//! minting every create needs.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::ops::types::shared_types::{
    ChangeInfo, DelegationSet, HostedZone, HostedZoneConfig, NoSuchHostedZone, ResourceRecordSet,
    Tag, VPC,
};

/// How long after a change is submitted it reaches `INSYNC`.
///
/// `CreateHostedZone`'s own model documentation: "When you submit a
/// CreateHostedZone request, the initial status of the hosted zone is PENDING. …
/// When the NS and SOA records are available, the status of the zone changes to
/// INSYNC." Real propagation takes up to a minute; the emulator owes the same
/// transition, so the span is short but NEVER zero — a caller that reads the
/// change straight back still observes `PENDING`, exactly as it would against
/// Route 53.
pub const CHANGE_PROPAGATION: Duration = Duration::from_millis(500);

/// The `Status` of a change that has not propagated yet (`ChangeStatus` enum).
pub const STATUS_PENDING: &str = "PENDING";
/// The `Status` of a change that has propagated (`ChangeStatus` enum).
pub const STATUS_INSYNC: &str = "INSYNC";

/// The most hosted zones the account may hold.
///
/// A zone is created with a caller-chosen `CallerReference`, so without a cap a
/// caller could add zones without end, each of them allowed to reach
/// [`MAX_RECORD_SETS_PER_ZONE`]. This is Route 53's own documented account
/// quota ("Hosted zones: 500 per account"), and the model declares
/// `TooManyHostedZones` for exactly this refusal: "This operation can't be
/// completed either because the current account has reached the limit on the
/// number of hosted zones".
pub const MAX_HOSTED_ZONES: usize = 500;

/// The most changes one `ChangeBatch` may carry.
///
/// The model puts no length constraint on `Changes`, so nothing upstream of the
/// handler bounds it. This is Route 53's own documented service quota — "Maximum
/// number of changes in a single ChangeResourceRecordSets request: 1,000" — so
/// the work one call can ask of the handler is bounded by the same number the
/// real service bounds it by.
pub const MAX_CHANGES_PER_BATCH: usize = 1_000;

/// The most record sets one hosted zone may hold.
///
/// Route 53's documented per-zone quota ("Records per hosted zone: 10,000"), so
/// repeated CREATE/UPSERT cannot grow a zone without end.
pub const MAX_RECORD_SETS_PER_ZONE: usize = 10_000;

/// The most `InvalidChangeBatch` messages one refusal reports.
///
/// A refusal names the changes that were wrong, and a batch may be a thousand
/// changes long, so the message list is capped and the refusal says how many
/// more there were: the error body cannot be amplified by the batch size.
pub const MAX_REPORTED_CHANGE_PROBLEMS: usize = 32;

/// The most changes the ledger retains.
///
/// Every mutating operation appends one, and no API operation deletes one, so
/// without a cap the ledger would grow for the life of the process. Past this
/// many the oldest are evicted (see [`Route53State::retain_recent_changes`]);
/// the cap is far above any single client session's change count, so a change a
/// caller is polling is never the one evicted.
const MAX_RETAINED_CHANGES: usize = 10_000;

/// How long a change stays readable once it has reached `INSYNC`.
///
/// A change is interesting until it has propagated and the caller has seen that
/// it did; an hour past `INSYNC` is far longer than any client waits, and it
/// gives the ledger a retention answer rather than unbounded growth.
const CHANGE_RETENTION: Duration = Duration::from_secs(3_600);

/// The most tags one taggable resource may carry.
///
/// `ChangeTagsForResource$AddTags`'s own documentation: "You can add a maximum
/// of 10 tags to a health check or a hosted zone", and the modelled `TagList`
/// behind `ResourceTagSet$Tags` is `len 1..10`, so a resource holding more than
/// ten tags could not even be reported on the wire.
pub const MAX_TAGS_PER_RESOURCE: usize = 10;

/// The `TagResourceType` naming a hosted zone (`TagResourceType` enum).
pub const TAG_RESOURCE_TYPE_HOSTED_ZONE: &str = "hostedzone";
/// The `TagResourceType` naming a health check (`TagResourceType` enum).
pub const TAG_RESOURCE_TYPE_HEALTH_CHECK: &str = "healthcheck";

/// The refusal text `CreateHostedZone` answers a `CallerReference` that already
/// names a zone with (`HostedZoneAlreadyExists`, HTTP 409).
///
/// A CONSTANT, naming neither the caller reference nor the zone the reference
/// already names: the shape's own documentation states the rule without either
/// ("Amazon Route 53 returns this error when a hosted zone has already been
/// created with the specified CallerReference"), and the zone's id is
/// server-generated, so a refusal quoting it would differ on every run and tell
/// a caller nothing it can act on. The zone the reference names is in state,
/// where `ListHostedZones` reads it.
pub const HOSTED_ZONE_ALREADY_EXISTS_MESSAGE: &str =
    "A hosted zone has already been created with the specified caller reference.";

/// The refusal text `DeleteHostedZone` answers a zone that still holds records
/// of the caller's with (`HostedZoneNotEmpty`, HTTP 400).
///
/// A CONSTANT naming no zone, and it ends with what the caller has to do next —
/// the operation's own documentation states both halves ("You can delete a
/// hosted zone only if it contains only the default SOA and NS records" / "If
/// the hosted zone contains other resource record sets, you must delete them
/// before you can delete the hosted zone").
pub const HOSTED_ZONE_NOT_EMPTY_MESSAGE: &str =
    "The specified hosted zone contains resource record sets other than the default SOA and NS \
     records and so cannot be deleted. Delete all non-default records and try again.";

/// The `/hostedzone/` prefix the API wraps a hosted-zone id in.
pub const HOSTED_ZONE_PREFIX: &str = "/hostedzone/";
/// The `/change/` prefix the API wraps a change id in.
pub const CHANGE_PREFIX: &str = "/change/";

/// A change request, as `GetChange` reports it.
///
/// The id is stored BARE (`C…`); [`StoredChange::to_change_info`] adds the
/// `/change/` prefix the wire carries.
#[derive(Debug, Clone)]
pub struct StoredChange {
    /// The bare change id, which is also this record's key in [`Route53State::changes`].
    pub id: String,
    /// When the change was submitted, in milliseconds since the Unix epoch.
    pub submitted_at_millis: i64,
    /// The instant the change becomes `INSYNC`; see [`CHANGE_PROPAGATION`].
    pub insync_at: Instant,
    /// The `ChangeBatch` comment, when the submitter gave one.
    pub comment: Option<String>,
}

impl StoredChange {
    /// `PENDING` until [`StoredChange::insync_at`], `INSYNC` from then on.
    ///
    /// Derived from the clock rather than stored, so no caller has to poll the
    /// change for it to make progress.
    pub fn status(&self) -> &'static str {
        if Instant::now() < self.insync_at {
            STATUS_PENDING
        } else {
            STATUS_INSYNC
        }
    }

    /// The modelled `ChangeInfo` for this change.
    pub fn to_change_info(&self) -> ChangeInfo {
        ChangeInfo {
            id: format!("{CHANGE_PREFIX}{}", self.id),
            status: self.status().to_string(),
            submitted_at: ::aws_smithy_types::DateTime::from_millis(self.submitted_at_millis),
            comment: self.comment.clone(),
        }
    }
}

/// The identity of a resource record set within a zone.
///
/// Route 53 keys a record set by (name, type, set identifier), and
/// `ListResourceRecordSets` returns them "sorted first by DNS name with the
/// labels reversed … When multiple records have the same DNS name,
/// ListResourceRecordSets sorts results by the record type" (the operation's own
/// documentation). Deriving `Ord` over the fields in that order makes a
/// `BTreeMap` keyed by this type give both the uniqueness rule and the list
/// order for free.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct RecordKey {
    /// The record name with its labels reversed and a trailing dot, lowercased:
    /// `www.example.com` sorts as `com.example.www.`.
    pub name_sort_key: String,
    /// The `RRType` of the record set.
    pub rr_type: String,
    /// The `SetIdentifier`, which distinguishes the members of a routing group.
    pub set_identifier: Option<String>,
}

impl RecordKey {
    /// The key for a record set, as the wire presents it.
    pub fn of(name: &str, rr_type: &str, set_identifier: Option<&str>) -> Self {
        Self {
            name_sort_key: dns_sort_key(name),
            rr_type: rr_type.to_string(),
            set_identifier: set_identifier.map(str::to_string),
        }
    }

    /// The key for an already-built record set.
    pub fn of_record(record: &ResourceRecordSet) -> Self {
        Self::of(
            &record.name,
            &record.r#type,
            record.set_identifier.as_deref(),
        )
    }
}

/// A hosted zone and the record sets it holds.
#[derive(Debug, Clone)]
pub struct StoredZone {
    /// The bare zone id (`Z…`), which is also this zone's key in [`Route53State::zones`].
    pub id: String,
    /// The domain name, normalized to a trailing dot by [`normalize_dns_name`].
    pub name: String,
    /// The `CallerReference` the zone was created with.
    pub caller_reference: String,
    /// The `Comment` from `HostedZoneConfig`, when one was given.
    pub comment: Option<String>,
    /// Whether this is a private hosted zone.
    pub private_zone: bool,
    /// The four authoritative name servers Route 53 assigned to this zone.
    pub name_servers: Vec<String>,
    /// The VPCs associated with a private hosted zone.
    pub vpcs: Vec<VPC>,
    /// This zone's record sets, keyed (and therefore ordered) by [`RecordKey`].
    pub records: BTreeMap<RecordKey, ResourceRecordSet>,
    /// The zone's tags, keyed by tag key — the set `ChangeTagsForResource`
    /// edits and `ListTagsForResource` reports. Held ON the zone, so deleting
    /// the zone takes its tags with it and no second map can disagree about
    /// them. Bounded by [`MAX_TAGS_PER_RESOURCE`].
    pub tags: BTreeMap<String, String>,
}

impl StoredZone {
    /// The modelled `HostedZone`, with a LIVE `ResourceRecordSetCount`.
    ///
    /// `Config` is omitted when the zone carries neither a comment nor the
    /// private flag, per the `HostedZone$Config` documentation: "If you omitted
    /// the HostedZoneConfig and Comment elements from the request, the Config
    /// and Comment elements don't appear in the response."
    pub fn to_hosted_zone(&self) -> HostedZone {
        let config = (self.comment.is_some() || self.private_zone).then(|| HostedZoneConfig {
            comment: self.comment.clone(),
            private_zone: self.private_zone,
        });
        HostedZone {
            id: format!("{HOSTED_ZONE_PREFIX}{}", self.id),
            name: self.name.clone(),
            caller_reference: self.caller_reference.clone(),
            config,
            resource_record_set_count: Some(self.records.len() as i64),
            linked_service: None,
            features: None,
        }
    }

    /// The zone's name servers as a `DelegationSet`.
    ///
    /// `Id` and `CallerReference` are absent: they belong to a REUSABLE
    /// delegation set (`CreateReusableDelegationSet`), and this zone has none.
    pub fn to_delegation_set(&self) -> DelegationSet {
        DelegationSet {
            id: None,
            caller_reference: None,
            name_servers: self.name_servers.clone(),
        }
    }

    /// This zone's tag set as `ResourceTagSet$Tags` carries it; see
    /// [`tags_to_list`].
    pub fn to_tag_list(&self) -> Option<Vec<Tag>> {
        tags_to_list(&self.tags)
    }

    /// Whether the zone still holds a record other than the default SOA and NS
    /// records at its apex.
    ///
    /// `DeleteHostedZone`'s documentation: "You can delete a hosted zone only if
    /// it contains only the default SOA and NS records."
    pub fn holds_user_records(&self) -> bool {
        let apex = dns_sort_key(&self.name);
        self.records.keys().any(|key| {
            key.name_sort_key != apex
                || key.set_identifier.is_some()
                || !matches!(key.rr_type.as_str(), "SOA" | "NS")
        })
    }
}

/// Per-service state for com-amazonaws-route53-awsdnsv20130401. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsRoute53Awsdnsv20130401State {
    /// Every hosted zone, keyed by its bare id. A `BTreeMap` so
    /// `ListHostedZones` pages over a stable total order of zone ids. Bounded by
    /// [`MAX_HOSTED_ZONES`].
    pub zones: BTreeMap<String, StoredZone>,
    /// `CallerReference` -> bare zone id, the index `CreateHostedZone` answers
    /// `HostedZoneAlreadyExists` from.
    pub zone_by_caller_reference: HashMap<String, String>,
    /// The retained changes, keyed by bare id, as `GetChange` reads them. Bounded
    /// by [`MAX_RETAINED_CHANGES`] and [`CHANGE_RETENTION`].
    pub changes: HashMap<String, StoredChange>,
    /// The ids in [`ComAmazonawsRoute53Awsdnsv20130401State::changes`] in
    /// submission order, so the oldest can be evicted first.
    change_order: VecDeque<String>,
    /// The id generator, kept here so ids stay unique across calls.
    ids: IdMinter,
}

impl ComAmazonawsRoute53Awsdnsv20130401State {
    /// Mint a hosted-zone id: `Z` plus 14 uppercase alphanumerics, the shape
    /// Route 53 returns (`Z1D633PJN98FT9`). Retried until it is unused.
    pub fn mint_zone_id(&mut self) -> String {
        loop {
            let id = self.ids.mint('Z', ID_BODY_LEN);
            if !self.zones.contains_key(&id) {
                return id;
            }
        }
    }

    /// Record a newly submitted change and return it, so the caller can render
    /// the `ChangeInfo` it just persisted. The id is `C` plus 14 uppercase
    /// alphanumerics, the shape Route 53 returns (`C2682N5HXP0BZ4`).
    pub fn submit_change(&mut self, comment: Option<String>) -> StoredChange {
        let id = loop {
            let id = self.ids.mint('C', ID_BODY_LEN);
            if !self.changes.contains_key(&id) {
                break id;
            }
        };
        let change = StoredChange {
            id: id.clone(),
            submitted_at_millis: now_millis(),
            insync_at: Instant::now() + CHANGE_PROPAGATION,
            comment,
        };
        self.change_order.push_back(id.clone());
        self.changes.insert(id, change.clone());
        self.retain_recent_changes();
        change
    }

    /// Drop the changes the ledger no longer retains: the oldest past
    /// [`MAX_RETAINED_CHANGES`], and any that reached `INSYNC` more than
    /// [`CHANGE_RETENTION`] ago.
    ///
    /// Called from the mutating path that appends a change, never from a read,
    /// so reading a change never changes what a later caller sees.
    fn retain_recent_changes(&mut self) {
        let now = Instant::now();
        while let Some(oldest) = self.change_order.front().cloned() {
            let over_cap = self.change_order.len() > MAX_RETAINED_CHANGES;
            let expired = self.changes.get(&oldest).is_none_or(|change| {
                now.saturating_duration_since(change.insync_at) > CHANGE_RETENTION
            });
            if !over_cap && !expired {
                break;
            }
            self.change_order.pop_front();
            self.changes.remove(&oldest);
        }
    }

    /// Mint the four authoritative name servers for a new hosted zone.
    ///
    /// The Route 53 Developer Guide ("NS and SOA Records that Route 53 Creates
    /// for a Hosted Zone") describes the delegation set as four name servers,
    /// one in each of the `com`, `net`, `org` and `co.uk` top-level domains, so
    /// a resolver failure in one TLD cannot take all four out.
    pub fn mint_name_servers(&mut self) -> Vec<String> {
        const TLDS: [&str; 4] = ["com", "net", "org", "co.uk"];
        TLDS.iter()
            .map(|tld| {
                let host = self.ids.next_u64() % 2048 + 1;
                let set = self.ids.next_u64() % 64;
                format!("ns-{host}.awsdns-{set:02}.{tld}")
            })
            .collect()
    }
}

/// The number of characters after the single-letter prefix in a Route 53
/// resource id (`Z1D633PJN98FT9`, `C2682N5HXP0BZ4`).
const ID_BODY_LEN: usize = 14;

/// Milliseconds since the Unix epoch, for a `SubmittedAt` timestamp.
pub fn now_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}

/// A DNS name normalized the way Route 53 stores it: with a trailing dot.
///
/// `Name`'s own documentation: "The trailing dot is optional; Amazon Route 53
/// assumes that the domain name is fully qualified. This means that Route 53
/// treats www.example.com (without a trailing dot) and www.example.com. (with a
/// trailing dot) as identical."
pub fn normalize_dns_name(name: &str) -> String {
    let trimmed = name.trim();
    if trimmed.is_empty() || trimmed.ends_with('.') {
        trimmed.to_string()
    } else {
        format!("{trimmed}.")
    }
}

/// The order key `ListResourceRecordSets` sorts by: the name's labels reversed,
/// with the trailing dot kept, lowercased.
///
/// From the operation's documentation: "ListResourceRecordSets sorts results
/// first by DNS name with the labels reversed, for example: `com.example.www.`
/// Note the trailing dot, which can change the sort order…". DNS names are
/// case-insensitive, so the key is lowercased and `WWW.example.com` and
/// `www.example.com` name the same record set.
pub fn dns_sort_key(name: &str) -> String {
    let normalized = normalize_dns_name(name).to_lowercase();
    let body = normalized.strip_suffix('.').unwrap_or(&normalized);
    if body.is_empty() {
        return normalized;
    }
    let mut key = String::with_capacity(normalized.len());
    for label in body.split('.').rev() {
        key.push_str(label);
        key.push('.');
    }
    key
}

/// Whether `name` is inside `zone_name` — the same name, or a subdomain of it.
///
/// `ChangeResourceRecordSets` only accepts records for the zone it is called
/// on, so this is what separates a record the zone owns from one it does not.
pub fn name_is_in_zone(name: &str, zone_name: &str) -> bool {
    let name_key = dns_sort_key(name);
    let zone_key = dns_sort_key(zone_name);
    name_key == zone_key || name_key.starts_with(&zone_key)
}

/// A resource's tag set as the wire carries it, in tag-key order.
///
/// `ResourceTagSet$Tags` is a list of at least one member, so a resource with no
/// tags reports the member ABSENT rather than as an empty list — the same rule
/// `GetHostedZone` applies to `VPCs`.
pub fn tags_to_list(tags: &BTreeMap<String, String>) -> Option<Vec<Tag>> {
    (!tags.is_empty()).then(|| {
        tags.iter()
            .map(|(key, value)| Tag {
                key: Some(key.clone()),
                value: Some(value.clone()),
            })
            .collect()
    })
}

/// Apply one `ChangeTagsForResource` batch to a resource's tag set.
///
/// A tag set is a MAP from key to value, which is what makes the operation
/// "adds, edits, or deletes tags" (its own documentation): adding a key the
/// resource already carries edits that key's value, and removing a key it does
/// not carry leaves the set as it was. `RemoveTagKeys` is applied FIRST, so a
/// key named in both lists ends up with the value `AddTags` gives it rather
/// than being dropped.
///
/// The batch is applied to a COPY and committed only once it is wholly valid,
/// so a refused batch leaves the resource's tags exactly as they were. `Err`
/// carries the refusal text for the caller's `InvalidInput`.
pub fn apply_tag_changes(
    tags: &mut BTreeMap<String, String>,
    add_tags: Option<&[Tag]>,
    remove_tag_keys: Option<&[String]>,
) -> Result<(), String> {
    if add_tags.is_none() && remove_tag_keys.is_none() {
        return Err(
            "the request must specify AddTags, RemoveTagKeys, or both: it asks for no change"
                .to_string(),
        );
    }

    let mut updated = tags.clone();
    for key in remove_tag_keys.unwrap_or_default() {
        updated.remove(key);
    }
    for tag in add_tags.unwrap_or_default() {
        // `Tag$Key` is not required by the model, but a tag with no key names
        // nothing to add, edit or delete.
        let key = tag
            .key
            .clone()
            .ok_or_else(|| "every Tag in AddTags must carry a Key".to_string())?;
        updated.insert(key, tag.value.clone().unwrap_or_default());
    }

    if updated.len() > MAX_TAGS_PER_RESOURCE {
        return Err(format!(
            "a resource can carry at most {MAX_TAGS_PER_RESOURCE} tags; this request would \
             leave it with {}",
            updated.len()
        ));
    }

    *tags = updated;
    Ok(())
}

/// The modelled `NoSuchHostedZone` (HTTP 404) for a zone id that names no zone
/// in this store.
///
/// The ONE place this service words that refusal, so every zone-addressed
/// operation answers a missing zone identically. The message is
/// REQUEST-DERIVED — it names the id the request asked about, as the request
/// wrote it — which is what lets a caller tell which of a batch of ids was the
/// missing one: `No hosted zone found with ID: <the requested id>`.
pub fn no_such_hosted_zone(requested_id: &str) -> NoSuchHostedZone {
    NoSuchHostedZone {
        message: Some(format!("No hosted zone found with ID: {requested_id}")),
    }
}

/// Mint the request id ONE response names the request it answers with — the
/// `RequestId` element of this service's `<ErrorResponse>` envelope (see
/// [`crate::service::envelope`]).
///
/// 8-4-4-4-12 lowercase hex, the form an AWS request id takes. The VALUE is
/// per-response and is not a requirement: nothing can predict it and no caller
/// may depend on a particular one. What it must do is name ONE response, so it
/// is drawn from a per-process counter mixed with the instant the response was
/// answered through the maintained `sha2` digest — the counter is what makes two
/// ids minted in the same nanosecond differ, and the instant is what keeps two
/// runs of this emulator from minting the same sequence. It is an identifier for
/// correlation, never a secret: nothing authenticates with it.
///
/// A free function rather than a mint on the store: a failure the request
/// boundary refused never reaches a handler, so the envelope that answers it has
/// no state to reach.
pub fn new_request_id() -> String {
    use sha2::{Digest, Sha256};

    static MINTED: AtomicU64 = AtomicU64::new(0);

    let counter = MINTED.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since_epoch| since_epoch.as_nanos());
    let mut digest = Sha256::new();
    digest.update(counter.to_be_bytes());
    digest.update(nanos.to_be_bytes());
    let hex: String = digest.finalize()[..16]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    format!(
        "{}-{}-{}-{}-{}",
        &hex[..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

/// A hosted-zone id with the `/hostedzone/` prefix stripped, so a caller may
/// pass either the bare id or the prefixed `Id` a response handed it back.
pub fn normalize_zone_id(id: &str) -> &str {
    strip_id_prefix(id, "hostedzone")
}

/// A change id with the `/change/` prefix stripped, for the same reason
/// [`normalize_zone_id`] strips `/hostedzone/`.
pub fn normalize_change_id(id: &str) -> &str {
    strip_id_prefix(id, "change")
}

/// Strip a leading `/`, then a leading `<collection>/`, from a resource id.
fn strip_id_prefix<'a>(id: &'a str, collection: &str) -> &'a str {
    let without_slash = id.strip_prefix('/').unwrap_or(id);
    without_slash
        .strip_prefix(collection)
        .and_then(|rest| rest.strip_prefix('/'))
        .unwrap_or(without_slash)
}

/// The id generator behind every minted zone and change id.
///
/// A plain xorshift64 sequence, seeded from the wall clock on first use: ids
/// have to be unique and opaque, not cryptographically unpredictable, and
/// keeping the generator in the state struct is what makes them unique ACROSS
/// calls rather than per handler.
#[derive(Debug, Default)]
struct IdMinter {
    seed: u64,
}

impl IdMinter {
    /// The alphabet Route 53 ids are drawn from: uppercase letters and digits.
    const ALPHABET: &'static [u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";

    fn next_u64(&mut self) -> u64 {
        if self.seed == 0 {
            // Any non-zero seed will do; the clock keeps two emulator runs from
            // minting the same first id. The constant is the fallback for a
            // clock that reads before the epoch.
            self.seed = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0x2545_F491_4F6C_DD1D, |d| d.as_nanos() as u64)
                | 1;
        }
        let mut x = self.seed;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.seed = x;
        x
    }

    /// `prefix` followed by `len` characters from [`IdMinter::ALPHABET`].
    fn mint(&mut self, prefix: char, len: usize) -> String {
        let mut id = String::with_capacity(len + 1);
        id.push(prefix);
        for _ in 0..len {
            let pick = (self.next_u64() % Self::ALPHABET.len() as u64) as usize;
            id.push(char::from(Self::ALPHABET[pick]));
        }
        id
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsRoute53Awsdnsv20130401State>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsRoute53Awsdnsv20130401State>)
}
