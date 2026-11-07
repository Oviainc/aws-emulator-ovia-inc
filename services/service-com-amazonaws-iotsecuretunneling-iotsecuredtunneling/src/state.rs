// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-iotsecuretunneling-iotsecuredtunneling service.
//!
//! The service has exactly one resource — the tunnel — so this module holds the
//! tunnel store, the tag store keyed by tunnel ARN, and the small amount of
//! minting/projection logic more than one operation needs (ids, access tokens,
//! the `ListTunnels` page token, and the clock-owed close of a tunnel whose
//! `maxLifetimeTimeoutMinutes` has elapsed).
//!
//! A tunnel is a REGIONAL resource, so the store is partitioned by the Region of
//! the endpoint each record was created against ([`request_region`]) and every
//! lookup names that Region: see
//! [`ComAmazonawsIotsecuretunnelingIotsecuredtunnelingState::tunnel`] and the
//! accessors beside it.
//!
//! The refusal messages here name the modelled error that reports them and never
//! its HTTP status: the status every modelled error of this service arrives with
//! is computed in one place, [`crate::scaffolded::wire_status`].

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::ops::types::{DestinationConfig, Tag};

/// `TimeoutConfig$maxLifetimeTimeoutMinutes`: "The maximum amount of time (in
/// minutes) a tunnel can remain open. If not specified,
/// maxLifetimeTimeoutMinutes defaults to 720 minutes." (Smithy model doc for
/// `com.amazonaws.iotsecuretunneling#TimeoutConfig$maxLifetimeTimeoutMinutes`.)
pub const DEFAULT_MAX_LIFETIME_TIMEOUT_MINUTES: i32 = 720;

/// The page size `ListTunnels` applies when the caller sends no `maxResults`.
/// The model bounds `maxResults` at `range(min: 1, max: 100)`, so a full page is
/// 100 summaries and anything beyond it is reported through `nextToken`.
pub const MAX_LIST_PAGE_SIZE: i32 = 100;

/// The tunnels one account may hold OPEN at the same time in one Region.
///
/// The quota the modelled `LimitExceededException` ("Thrown when a
/// tunnel limit is exceeded") reports is a quota on CONCURRENCY: a tunnel
/// occupies one of these slots only while its status is `TunnelStatus::Open`,
/// so a tunnel that has been closed — by `CloseTunnel` or by its
/// `maxLifetimeTimeoutMinutes` running out — releases its slot immediately,
/// even though `CloseTunnel$delete` ("When set to true, IoT Secure Tunneling
/// deletes the tunnel data immediately") means its record is still describable.
/// A slot is a resource of the Region that holds the tunnel, since the quota and
/// the tunnel alike belong to one Region's endpoint.
///
/// The model publishes no number, so the number itself is this emulator's
/// bound, kept to the order of the quota AWS publishes for concurrently open
/// tunnels rather than to the size of the store (see [`MAX_RETAINED_TUNNELS`]).
pub const MAX_CONCURRENT_TUNNELS: usize = 50;

/// The refusal message for an `OpenTunnel` past [`MAX_CONCURRENT_TUNNELS`],
/// beside the constant it reports exactly as [`tag_limit_message`] sits beside
/// its own: the text names the quota and the number of tunnels the account
/// already holds open, and what releases a slot.
///
/// `open` is read from the store, so the text describes the account's own state
/// and nothing else.
pub fn tunnel_limit_message(open: usize) -> String {
    format!(
        "Tunnel limit of {MAX_CONCURRENT_TUNNELS} concurrently open tunnels exceeded; this \
         account holds {open} open tunnels in this Region, so close one before opening another"
    )
}

/// The number of tunnel RECORDS this service keeps, closed history included.
///
/// This is not the quota AWS enforces on a caller — that is
/// [`MAX_CONCURRENT_TUNNELS`], and a record past its close consumes none of it —
/// but the bound that keeps the store from growing without bound in a
/// long-lived emulator: each record's size is bounded by the modelled `@length`
/// and `@pattern` constraints on its members, and the record COUNT is bounded
/// here, across every Region, since the Region a record belongs to comes from
/// the request. Records a caller deletes leave through `CloseTunnel` with
/// `delete` set, the one call AWS documents as deleting a tunnel's data; beyond
/// that, the longest-closed records are evicted to make room for a new tunnel,
/// and an `OpenTunnel` the store has no room for at all is refused (see
/// [`ComAmazonawsIotsecuretunnelingIotsecuredtunnelingState::make_room_for_tunnel`]).
/// An OPEN tunnel is never evicted, and the number itself is this emulator's
/// bound, since the model publishes none.
pub const MAX_RETAINED_TUNNELS: usize = 500;

/// The refusal message for an `OpenTunnel` the store has no room for: it holds
/// [`MAX_RETAINED_TUNNELS`] records and none of them is closed, so nothing can
/// be evicted to make room.
///
/// Reported with the same modelled `LimitExceededException` as
/// [`tunnel_limit_message`], the one error this service models for an exhausted
/// limit; the text names the limit and what releases a record.
pub fn tunnel_record_limit_message() -> String {
    format!(
        "Tunnel limit of {MAX_RETAINED_TUNNELS} tunnel records exceeded; close a tunnel before \
         opening another"
    )
}

/// The number of tags one tunnel may carry.
///
/// AWS's tagging convention across services is 50 tags per resource, and IoT
/// publishes that same per-resource limit. The modelled `TagList`
/// (`length(min: 1, max: 200)`) bounds ONE request's list but not the set a
/// resource accumulates over repeated `TagResource` calls, so the accumulated
/// key count is bounded here: a request that would take a tunnel past it is
/// refused — with the modelled `LimitExceededException`, the one
/// error this service models for an exhausted limit — and stores nothing. This
/// is also what keeps the tag store from growing without bound in a long-lived
/// emulator — each key and value is bounded by the modelled `@length`
/// constraints on `Tag`, the keys per tunnel by this constant, and the tunnels
/// by [`MAX_RETAINED_TUNNELS`].
pub const MAX_TAGS_PER_TUNNEL: usize = 50;

/// The refusal message for a request that would take one tunnel past
/// [`MAX_TAGS_PER_TUNNEL`] tags.
///
/// Kept beside the constant so every tagging entry point refuses in the same
/// words, and reached only through
/// [`ComAmazonawsIotsecuretunnelingIotsecuredtunnelingState::tag_limit_refusal`],
/// the one place the rule's comparison lives. `count` is the number of distinct keys the tunnel would end up
/// carrying, so the text names the limit and how far past it the request goes
/// and nothing else; both entry points carry the tags on the `tags` member, so
/// that is the member the text names.
pub fn tag_limit_message(count: usize) -> String {
    format!(
        "Tag limit of {MAX_TAGS_PER_TUNNEL} exceeded; tags would take this tunnel to {count} tags"
    )
}

/// The number of service names one tunnel's `destinationConfig` may carry.
///
/// A tunnel is multiplexed over at most three simultaneous data streams — one
/// per service name the destination configuration lists — so a longer
/// `services` list asks for streams the tunnel cannot carry and is refused
/// rather than stored. The modelled `ServiceList` publishes only
/// `length(min: 1)`, so the boundary validator passes any length through and the
/// rule lives here, applied by every writer of the field.
pub const MAX_SERVICES_PER_TUNNEL: usize = 3;

/// The stored form of a request's `destinationConfig`, or the message refusing a
/// `services` list longer than [`MAX_SERVICES_PER_TUNNEL`].
///
/// The single place that rule is applied, so every writer of the field —
/// `OpenTunnel` and `RotateTunnelAccessToken` — refuses the same lists in the
/// same words, and a refused request is refused before it mints or stores
/// anything. `member` is the request member the configuration arrived on, so the
/// message names what the caller has to fix; the count comes from the request
/// and nothing else.
pub fn stored_destination_config(
    member: &str,
    config: &DestinationConfig,
) -> Result<StoredDestinationConfig, String> {
    let count = config.services.len();
    if count > MAX_SERVICES_PER_TUNNEL {
        return Err(format!(
            "{member}.services names {count} services; a tunnel is multiplexed over at most \
             {MAX_SERVICES_PER_TUNNEL} simultaneous data streams"
        ));
    }
    Ok(StoredDestinationConfig {
        thing_name: config.thing_name.clone(),
        services: config.services.clone(),
    })
}

/// The tag-key namespace AWS keeps for the tags it applies itself.
///
/// AWS's tagging rules reserve this prefix for AWS-generated tags and state that
/// a caller can neither create nor edit nor delete a tag key carrying it, so a
/// request that names such a key is refused rather than stored. The modelled
/// `TagKey` pattern `^([\p{L}\p{Z}\p{N}_.:/=+\-@]*)$` admits `:`, so the
/// boundary validator passes the key through and the rule lives here.
pub const RESERVED_TAG_KEY_PREFIX: &str = "aws:";

/// Whether `key` lies in the namespace [`RESERVED_TAG_KEY_PREFIX`] reserves.
///
/// Compared without regard to letter case: the reservation protects the
/// namespace itself, so a key differing from it only in case is in it too and is
/// refused the same way.
pub fn is_reserved_tag_key(key: &str) -> bool {
    key.get(..RESERVED_TAG_KEY_PREFIX.len())
        .is_some_and(|head| head.eq_ignore_ascii_case(RESERVED_TAG_KEY_PREFIX))
}

/// The first key of `keys` that [`is_reserved_tag_key`], if any — what a handler
/// checks before it writes, so the whole request is refused and nothing in it is
/// applied.
pub fn first_reserved_tag_key<'a>(keys: impl IntoIterator<Item = &'a str>) -> Option<&'a str> {
    keys.into_iter().find(|key| is_reserved_tag_key(key))
}

/// The refusal message for a request whose `member` named the reserved `key`.
///
/// Built from the request — the member and the offending key — so the caller can
/// fix the request from the text alone. Kept here so every tagging entry point
/// refuses in the same words.
pub fn reserved_tag_key_message(member: &str, key: &str) -> String {
    format!(
        "{member} names the tag key '{key}': the '{RESERVED_TAG_KEY_PREFIX}' prefix is reserved \
         for tags Amazon Web Services applies itself and cannot be used by a caller."
    )
}

/// The region an ARN carries when the request is unsigned and so names none.
/// `emulator_arn`'s own documented default.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The ARN service namespace of a tunnel: AWS IoT Secure Tunneling resources are
/// addressed in the `iot` namespace (`arn:aws:iot:<region>:<account>:tunnel/<id>`),
/// which is also the namespace its IAM actions live in. The model's
/// `aws.api#service$arnNamespace` metadata says `iotsecuredtunneling`, but that is
/// the internal service name, not the namespace AWS publishes for a tunnel ARN.
pub const TUNNEL_ARN_SERVICE: &str = "iot";

/// The prefix that marks a `ListTunnels` page token as one this service minted.
/// The whole token stays inside `NextToken`'s modelled pattern
/// `^[a-zA-Z0-9_=-]{1,4096}$`.
const NEXT_TOKEN_PREFIX: &str = "tnl-";

/// `TunnelStatus`, whose modelled values are `OPEN` and `CLOSED`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TunnelStatus {
    Open,
    Closed,
}

impl TunnelStatus {
    /// The value as it goes on the wire.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Open => "OPEN",
            Self::Closed => "CLOSED",
        }
    }
}

/// `ConnectionStatus`, whose modelled values are `CONNECTED` and `DISCONNECTED`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionStatus {
    Connected,
    Disconnected,
}

impl ConnectionStatus {
    /// The value as it goes on the wire.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Connected => "CONNECTED",
            Self::Disconnected => "DISCONNECTED",
        }
    }
}

/// The connection state of one end of a tunnel.
#[derive(Debug, Clone)]
pub struct StoredConnectionState {
    pub status: ConnectionStatus,
    pub last_updated_at: SystemTime,
}

impl StoredConnectionState {
    /// A end that no local proxy has connected to yet.
    pub fn disconnected(at: SystemTime) -> Self {
        Self {
            status: ConnectionStatus::Disconnected,
            last_updated_at: at,
        }
    }
}

/// `DestinationConfig` as persisted.
#[derive(Debug, Clone, Default)]
pub struct StoredDestinationConfig {
    pub thing_name: Option<String>,
    pub services: Vec<String>,
}

/// One tunnel, the service's only resource.
#[derive(Debug, Clone)]
pub struct StoredTunnel {
    pub tunnel_id: String,
    pub tunnel_arn: String,
    pub status: TunnelStatus,
    pub description: Option<String>,
    pub destination_config: Option<StoredDestinationConfig>,
    pub max_lifetime_timeout_minutes: i32,
    pub source_connection_state: StoredConnectionState,
    pub destination_connection_state: StoredConnectionState,
    pub source_access_token: String,
    pub destination_access_token: String,
    pub created_at: SystemTime,
    pub last_updated_at: SystemTime,
    /// The instant this tunnel reached `CLOSED`, and `None` while it is still
    /// open.
    ///
    /// The record of ONE lifecycle event, written once by [`StoredTunnel::close`]
    /// — the single writer of the transition, so this carries a value exactly
    /// when `status` is `CLOSED` — and never rewritten afterwards, unlike
    /// `last_updated_at`, which every call that changes anything the tunnel
    /// reports (its tag set included) stamps with the instant of ITS change.
    /// Anything that has to order closed records against each other reads this.
    pub closed_at: Option<SystemTime>,
    /// The instant the tunnel's `maxLifetimeTimeoutMinutes` runs out, after which
    /// the clock owes it a transition to `CLOSED`.
    pub expires_at: SystemTime,
    /// Creation order, so `ListTunnels` can report tunnels "by creation time in
    /// descending order" (the operation's own documentation) without depending on
    /// clock resolution for two tunnels opened in the same instant.
    pub creation_seq: u64,
}

impl StoredTunnel {
    /// Apply the close transition at `at`: the tunnel becomes `CLOSED` and
    /// neither end is connected to it any more ("we close the WebSocket
    /// connections between the client and proxy server", `CloseTunnel`'s own
    /// documentation).
    ///
    /// The single writer of the transition, so whoever closes a tunnel — the
    /// explicit call or the clock-owed expiry sweep — writes the same fields and
    /// stamps them with the instant the close happened. IDEMPOTENT: a tunnel
    /// that is already `CLOSED` is left exactly as it is, stamps included,
    /// because a close that changes nothing about the resource has nothing to
    /// report; `true` means this call was the one that closed it.
    ///
    /// It also records [`Self::closed_at`], the instant of this transition, which
    /// no later write to the record changes.
    pub fn close(&mut self, at: SystemTime) -> bool {
        if self.status != TunnelStatus::Open {
            return false;
        }
        self.status = TunnelStatus::Closed;
        self.last_updated_at = at;
        self.closed_at = Some(at);
        self.source_connection_state = StoredConnectionState::disconnected(at);
        self.destination_connection_state = StoredConnectionState::disconnected(at);
        true
    }
}

/// Per-service state for com-amazonaws-iotsecuretunneling-iotsecuredtunneling. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsIotsecuretunnelingIotsecuredtunnelingState {
    /// Every tunnel, keyed by the Region of the endpoint it was created against
    /// and then by its `tunnelId`.
    ///
    /// The Region is part of a tunnel's address because a tunnel is a regional
    /// resource: it is held by one Region's endpoint and is neither readable nor
    /// mutable through another's. Reach a record through [`Self::tunnel`],
    /// [`Self::tunnel_mut`], [`Self::tunnels_in_region`], [`Self::insert_tunnel`]
    /// and [`Self::remove_tunnel`], which all take the Region of the request
    /// being served, rather than by building a key here.
    pub tunnels: BTreeMap<(String, String), StoredTunnel>,
    /// The authoritative tag store, keyed by `tunnelArn` then by tag key. Shared
    /// by `OpenTunnel`, `TagResource`, `UntagResource`, `ListTagsForResource` and
    /// `DescribeTunnel`'s `tunnel.tags`. A tunnel ARN names its own Region, so
    /// this key is already Region-specific.
    pub tags: BTreeMap<String, BTreeMap<String, String>>,
    /// `(region, tunnelArn)` -> `tunnelId`, so an ARN-addressed operation
    /// resolves an ARN only to a tunnel of the Region it is serving.
    pub arn_index: BTreeMap<(String, String), String>,
    /// The soonest instant at which some OPEN tunnel's lifetime runs out, so the
    /// expiry sweep is a single comparison on the overwhelmingly common call
    /// where nothing is due yet, instead of a scan of the whole store under the
    /// shared lock. `None` means no tunnel is open.
    earliest_open_expiry: Option<SystemTime>,
    /// The next creation sequence number handed to a tunnel.
    next_creation_seq: u64,
}

impl ComAmazonawsIotsecuretunnelingIotsecuredtunnelingState {
    /// The tunnel `tunnel_id` names in `region`, or `None` when that Region's
    /// endpoint holds no such tunnel — including when another Region does.
    pub fn tunnel(&self, region: &str, tunnel_id: &str) -> Option<&StoredTunnel> {
        self.tunnels.get(&tunnel_key(region, tunnel_id))
    }

    /// [`Self::tunnel`], for the operations that mutate the record they resolve.
    pub fn tunnel_mut(&mut self, region: &str, tunnel_id: &str) -> Option<&mut StoredTunnel> {
        self.tunnels.get_mut(&tunnel_key(region, tunnel_id))
    }

    /// Stamp the tunnel `tunnel_id` names in `region` as last updated at `at`.
    ///
    /// `Tunnel$lastUpdatedAt` is "The last time the tunnel was updated", so
    /// every call that changes something the tunnel reports goes through here
    /// with the instant of its change — whichever member of the record it wrote,
    /// and including the tag set, which `DescribeTunnel` reports inside the same
    /// `Tunnel` shape as this stamp. A call that changed nothing has nothing to
    /// report and so does not call this, exactly as [`StoredTunnel::close`]
    /// leaves an already-closed record's stamps alone.
    pub fn stamp_tunnel_updated(&mut self, region: &str, tunnel_id: &str, at: SystemTime) {
        if let Some(tunnel) = self.tunnel_mut(region, tunnel_id) {
            tunnel.last_updated_at = at;
        }
    }

    /// Every tunnel `region`'s endpoint holds, in `tunnelId` order.
    ///
    /// The keys are ordered by Region first, so one Region's records are a
    /// contiguous run and a listing walks that run alone rather than the whole
    /// store.
    pub fn tunnels_in_region<'state, 'region: 'state>(
        &'state self,
        region: &'region str,
    ) -> impl Iterator<Item = &'state StoredTunnel> + 'state {
        self.tunnels
            .range((
                ::std::ops::Bound::Included(tunnel_key(region, "")),
                ::std::ops::Bound::Unbounded,
            ))
            .take_while(move |((stored_region, _), _)| stored_region == region)
            .map(|(_, tunnel)| tunnel)
    }

    /// Store `tunnel` as a resource of `region`, and index its ARN there, so the
    /// record and the ARN that addresses it enter the store together.
    pub fn insert_tunnel(&mut self, region: &str, tunnel: StoredTunnel) {
        self.arn_index.insert(
            tunnel_key(region, &tunnel.tunnel_arn),
            tunnel.tunnel_id.clone(),
        );
        self.tunnels
            .insert(tunnel_key(region, &tunnel.tunnel_id), tunnel);
    }

    /// Drop the tunnel `tunnel_id` names in `region`, with its ARN index entry
    /// and its tags — everything the record owns leaves the store together.
    pub fn remove_tunnel(&mut self, region: &str, tunnel_id: &str) -> Option<StoredTunnel> {
        let tunnel = self.tunnels.remove(&tunnel_key(region, tunnel_id))?;
        self.arn_index
            .remove(&tunnel_key(region, &tunnel.tunnel_arn));
        self.tags.remove(&tunnel.tunnel_arn);
        Some(tunnel)
    }

    /// Claim the next creation sequence number.
    pub fn next_creation_seq(&mut self) -> u64 {
        let seq = self.next_creation_seq;
        self.next_creation_seq = seq.saturating_add(1);
        seq
    }

    /// How many tunnels `region`'s endpoint currently holds OPEN — the count the
    /// concurrency quota is enforced against. A record whose status is `CLOSED`
    /// is not counted, however it got there, so closing a tunnel frees a slot
    /// while leaving the record readable. See [`MAX_CONCURRENT_TUNNELS`].
    ///
    /// Call [`Self::materialize_expired_tunnels`] first, so a tunnel whose
    /// lifetime has already run out is not counted as open.
    pub fn open_tunnel_count(&self, region: &str) -> usize {
        self.tunnels_in_region(region)
            .filter(|tunnel| tunnel.status == TunnelStatus::Open)
            .count()
    }

    /// Make room in the store for one more record, and report whether there is
    /// room: `true` when the store holds fewer than [`MAX_RETAINED_TUNNELS`]
    /// records once the longest-closed ones have been dropped, `false` when it
    /// is still full and so cannot take another record.
    ///
    /// Only a CLOSED record is ever evicted — an open tunnel is live state a
    /// caller is still using — and the one closed longest ago goes first, so
    /// what a caller loses is the oldest history. The eviction walks every
    /// Region, because the bound is on this store and not on one Region's share
    /// of it: the Region a record belongs to comes from the request, so a bound
    /// held per Region would be no bound on the store at all.
    ///
    /// The order is read from [`StoredTunnel::closed_at`], the instant of the
    /// close transition itself: a record carries one exactly while it is
    /// `CLOSED`, so selecting on it is selecting a closed record, and no later
    /// write to the record moves it in this order. `last_updated_at` reports the
    /// last change of ANY kind to a tunnel, so it says nothing about when a
    /// record was closed and cannot order closes.
    ///
    /// Ties — two records closed in the same instant — are broken by creation
    /// order, so the eviction is total and deterministic.
    ///
    /// Called by the one operation that grows the store, which refuses the
    /// request when this answers `false` rather than inserting anyway.
    pub fn make_room_for_tunnel(&mut self) -> bool {
        while self.tunnels.len() >= MAX_RETAINED_TUNNELS {
            let Some((region, tunnel_id)) = self
                .tunnels
                .iter()
                .filter_map(|(key, tunnel)| {
                    tunnel
                        .closed_at
                        .map(|closed_at| (key, closed_at, tunnel.creation_seq))
                })
                .min_by_key(|(_, closed_at, creation_seq)| (*closed_at, *creation_seq))
                .map(|((region, tunnel_id), _, _)| (region.clone(), tunnel_id.clone()))
            else {
                // Every record is open, so there is nothing retention may
                // reclaim and the store has no room for another tunnel.
                return false;
            };
            self.remove_tunnel(&region, &tunnel_id);
        }
        true
    }

    /// Record a newly opened tunnel's expiry, so the sweep knows when to run.
    pub fn note_open_expiry(&mut self, expires_at: SystemTime) {
        self.earliest_open_expiry = Some(match self.earliest_open_expiry {
            Some(earliest) if earliest <= expires_at => earliest,
            _ => expires_at,
        });
    }

    /// The id of the tunnel `arn` names, if `region`'s endpoint holds one.
    ///
    /// An ARN names the Region of the resource it addresses, so an ARN minted in
    /// another Region resolves to nothing here, exactly as the tunnel it names is
    /// unreachable through this endpoint.
    pub fn tunnel_id_for_arn(&self, region: &str, arn: &str) -> Option<String> {
        self.arn_index.get(&tunnel_key(region, arn)).cloned()
    }

    /// The message refusing a request that would take the resource `arn` names
    /// past [`MAX_TAGS_PER_TUNNEL`] tags, or `None` when `incoming` keeps it
    /// within the limit.
    ///
    /// The single place that rule is applied — the merged count, the comparison
    /// against the constant and the wording all live here — so every tagging
    /// entry point, `OpenTunnel`'s `tags` and `TagResource`'s `tags`, refuses at
    /// the same count in the same words, and neither can drift from the other.
    /// Like [`stored_destination_config`], it hands back the MESSAGE so each
    /// caller wraps it in its own operation's `LimitExceededException` variant.
    /// Stores nothing: a caller asks this before it writes, so a refused request
    /// applies none of its tags.
    pub fn tag_limit_refusal(&self, arn: &str, incoming: &[Tag]) -> Option<String> {
        let merged = self.merged_tag_count(arn, incoming);
        (merged > MAX_TAGS_PER_TUNNEL).then(|| tag_limit_message(merged))
    }

    /// The number of distinct tag keys `arn` would carry after merging
    /// `incoming` into what is already stored. Stores nothing, and is private to
    /// this module: a handler asks [`Self::tag_limit_refusal`], which is where
    /// the count is compared against [`MAX_TAGS_PER_TUNNEL`].
    fn merged_tag_count(&self, arn: &str, incoming: &[Tag]) -> usize {
        let mut keys: ::std::collections::BTreeSet<&str> = self
            .tags
            .get(arn)
            .map(|stored| stored.keys().map(String::as_str).collect())
            .unwrap_or_default();
        keys.extend(incoming.iter().map(|tag| tag.key.as_str()));
        keys.len()
    }

    /// Merge `tags` into the tag store of the resource `arn` names, and report
    /// whether the stored set CHANGED.
    ///
    /// A tag whose key is already stored with the same value leaves the set
    /// exactly as it is, so `false` means the resource is unchanged and its
    /// `lastUpdatedAt` has nothing to record (see
    /// [`Self::stamp_tunnel_updated`]).
    ///
    /// The answer is about the set this call LEAVES behind against the set the
    /// resource carried BEFORE it, never against a value another element of this
    /// same call wrote: a key the list names more than once is stored with its
    /// last occurrence, so the request is collapsed per key first and each key is
    /// then written — and compared — exactly once. A request whose net effect is
    /// the identity therefore answers `false` however many times it names a key.
    pub fn put_tags(&mut self, arn: &str, tags: &[Tag]) -> bool {
        if tags.is_empty() {
            return false;
        }
        // Last occurrence wins, exactly as a sequence of writes to one key
        // leaves the last value written.
        let mut requested: BTreeMap<&str, &str> = BTreeMap::new();
        for tag in tags {
            requested.insert(tag.key.as_str(), tag.value.as_str());
        }
        let stored = self.tags.entry(arn.to_string()).or_default();
        let mut changed = false;
        for (key, value) in requested {
            let previous = stored.insert(key.to_string(), value.to_string());
            changed |= previous.as_deref() != Some(value);
        }
        changed
    }

    /// Remove `keys` from the tag store of the resource `arn` names, and report
    /// whether the stored set CHANGED.
    ///
    /// A key that is not stored is left alone rather than refused, so `false`
    /// means the resource carried none of the named keys and is unchanged.
    pub fn remove_tags<'key>(
        &mut self,
        arn: &str,
        keys: impl IntoIterator<Item = &'key str>,
    ) -> bool {
        let Some(stored) = self.tags.get_mut(arn) else {
            return false;
        };
        let mut changed = false;
        for key in keys {
            changed |= stored.remove(key).is_some();
        }
        changed
    }

    /// The tags stored for `arn`, as the modelled list — possibly EMPTY.
    ///
    /// This is the projection a reported resource carries its tag set in: the
    /// member reports the stored set whatever its size, so a resource with no
    /// tags reports an empty list and a reader can iterate it without having to
    /// tell "no tags" from "tags not reported". `TagList`'s
    /// `length(min: 1, max: 200)` bounds a list a CALLER sends, not the set a
    /// resource reports back.
    pub fn projected_tag_list(&self, arn: &str) -> Vec<Tag> {
        self.tags
            .get(arn)
            .into_iter()
            .flatten()
            .map(|(key, value)| Tag {
                key: key.clone(),
                value: value.clone(),
            })
            .collect()
    }

    /// The tags stored for `arn` as an OPTIONAL list, `None` when there are
    /// none — for a member whose absence is what reports an untagged resource.
    pub fn tag_list(&self, arn: &str) -> Option<Vec<Tag>> {
        let tags = self.projected_tag_list(arn);
        (!tags.is_empty()).then_some(tags)
    }

    /// Close every tunnel whose `maxLifetimeTimeoutMinutes` has run out.
    ///
    /// This is the one write a reporting operation may make: the transition was
    /// already owed at `now` by the clock — "The maximum amount of time (in
    /// minutes) a tunnel can remain open" — whether or not anybody called. It is
    /// idempotent, and it stamps `lastUpdatedAt` with the instant the transition
    /// came due rather than with the time of the call that noticed it.
    ///
    /// The transition itself is [`StoredTunnel::close`], shared with the explicit
    /// `CloseTunnel`, so both writers close a tunnel in exactly one way and
    /// neither touches a record that is already `CLOSED`.
    ///
    /// The store is scanned only when something is actually due: the soonest
    /// expiry of any open tunnel is tracked on the state, so the usual call pays
    /// one comparison rather than a walk of every record while holding the lock.
    pub fn materialize_expired_tunnels(&mut self, now: SystemTime) {
        match self.earliest_open_expiry {
            // Nothing is open, or the soonest expiry is still in the future.
            None => return,
            Some(earliest) if now < earliest => return,
            Some(_) => {}
        }

        let mut earliest: Option<SystemTime> = None;
        for tunnel in self.tunnels.values_mut() {
            if tunnel.status != TunnelStatus::Open {
                continue;
            }
            if now >= tunnel.expires_at {
                // The stamp is the instant the transition came DUE, not the
                // instant of the call that noticed it.
                tunnel.close(tunnel.expires_at);
            } else if earliest.is_none_or(|soonest| tunnel.expires_at < soonest) {
                earliest = Some(tunnel.expires_at);
            }
        }
        self.earliest_open_expiry = earliest;
    }
}

/// Take the store lock, read the clock INSIDE the critical section and settle
/// the close the clock already owes, handing back the guard and that instant.
///
/// Every operation of this service reasons about the store with ONE instant, and
/// that instant is the one at which it actually holds the store. Acquiring this
/// shared lock blocks for an unbounded time — every operation takes the same one,
/// and the listing operations hold it while they project records — so an instant
/// sampled before the acquisition is not the instant at which the store is
/// observed: a lifetime that ran out while the caller waited for the lock would
/// still look live to it, [`ComAmazonawsIotsecuretunnelingIotsecuredtunnelingState::materialize_expired_tunnels`]
/// would be handed an instant earlier than that expiry and would settle nothing,
/// and a stamp written from it could move `lastUpdatedAt` backwards past the
/// update of a call that held the lock first.
///
/// So the clock is read here, once the guard is held, and the sweep runs on that
/// instant — the single place this service samples it for the store, which is
/// what keeps the eight handlers agreeing on it.
pub fn lock_and_settle(
    state: &::engine::ServiceState<ComAmazonawsIotsecuretunnelingIotsecuredtunnelingState>,
) -> (
    ::std::sync::MutexGuard<'_, ComAmazonawsIotsecuretunnelingIotsecuredtunnelingState>,
    SystemTime,
) {
    let mut store = state.lock();
    let now = SystemTime::now();
    store.materialize_expired_tunnels(now);
    (store, now)
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsIotsecuretunnelingIotsecuredtunnelingState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsIotsecuretunnelingIotsecuredtunnelingState>)
}

/// How a regional record is addressed in this store: the Region of the endpoint
/// the request came in on, then the record's own identifier (a `tunnelId`, or a
/// `tunnelArn` in the ARN index).
///
/// The single place the composite key is built, so the store, the ARN index and
/// every accessor agree on it.
fn tunnel_key(region: &str, identifier: &str) -> (String, String) {
    (region.to_string(), identifier.to_string())
}

/// The region this request was signed for, or [`DEFAULT_REGION`] when it carries
/// no SigV4 credential scope to read one from.
///
/// This is the Region whose endpoint the caller is talking to, and so the Region
/// whose resources the request can address at all: every operation of this
/// service resolves a tunnel within it.
///
/// Only a Region LABEL is taken from the scope — the same grammar
/// [`tunnel_id_from_arn`] reads an ARN's Region field under — so the Region a
/// record is filed and minted into its ARN under is always one the ARN-addressed
/// operations can read back out of that ARN. Arbitrary text names no Region any
/// endpoint has, so it falls back to [`DEFAULT_REGION`] exactly as an unsigned
/// request does.
///
/// Kept here rather than hoisted: it reads this service's own
/// `emulator_server` boundary for the one Region every operation of this service
/// partitions its records by, so a service-local helper is where it belongs.
pub fn request_region(ctx: &smithy_server_runtime::OperationContext<'_>) -> String {
    emulator_server::sigv4_credential_scope(&ctx.parts().headers)
        .filter(|scope| ::service_common::arn::region_label(&scope.region).is_some())
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The type of resource a tunnel ARN names: `tunnel/<tunnelId>`.
const TUNNEL_ARN_RESOURCE_TYPE: &str = "tunnel";

/// `TunnelId`'s modelled `length` ceiling, from its pattern
/// `^[a-zA-Z0-9_\-+=:]{1,128}$`.
const MAX_TUNNEL_ID_LEN: usize = 128;

/// Whether `candidate` is a value `TunnelId` admits, i.e. within its modelled
/// pattern `^[a-zA-Z0-9_\-+=:]{1,128}$`.
///
/// The pattern's class is ASCII-only, so a byte walk decides it and the length
/// bound counts the same units the pattern's `{1,128}` does.
fn is_modelled_tunnel_id(candidate: &str) -> bool {
    (1..=MAX_TUNNEL_ID_LEN).contains(&candidate.len())
        && candidate.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'+' | b'=' | b':')
        })
}

/// The ARN of the tunnel `tunnel_id` in `region`.
pub fn tunnel_arn(region: &str, tunnel_id: &str) -> String {
    ::engine::emulator_arn(
        TUNNEL_ARN_SERVICE,
        region,
        &format!("{TUNNEL_ARN_RESOURCE_TYPE}/{tunnel_id}"),
    )
}

/// The `tunnelId` a resource ARN names, or `None` when `arn` is not of the form
/// `arn:<partition>:iot:<region>:<account>:tunnel/<tunnelId>`.
///
/// EVERY field of that form is checked, because an id is only lifted out of a
/// string that really addresses a tunnel: a string failing any of them names no
/// tunnel and so carries no id, however much of the shape of an ARN it has.
///
/// The ARN itself is read through `service_common::arn` — the workspace's ONE
/// caller-supplied-ARN parse — which decides the `arn` marker, the partition and
/// the [`TUNNEL_ARN_SERVICE`] namespace a tunnel is addressed in, and reads the
/// resource in both of the `<type>/<id>` and `<type>:<id>` forms AWS writes one
/// in. On top of that parse come the two constraints a TUNNEL carries: it is a
/// regional, account-owned resource, so neither of those fields may be empty and
/// the Region has to be a Region LABEL rather than arbitrary text; and its id is
/// one `TunnelId` admits ([`is_modelled_tunnel_id`]).
pub fn tunnel_id_from_arn(arn: &str) -> Option<&str> {
    let parsed = ::service_common::arn::parse_of(arn, TUNNEL_ARN_SERVICE)?;
    if ::service_common::arn::region_label(parsed.region).is_none() || parsed.account.is_empty() {
        return None;
    }
    let tunnel_id = ::service_common::arn::resource_id_of_type(arn, TUNNEL_ARN_RESOURCE_TYPE)?;
    is_modelled_tunnel_id(tunnel_id).then_some(tunnel_id)
}

/// The message every `ResourceNotFoundException` of this service carries.
///
/// AWS identifies the missing resource by the TUNNEL's id, in one sentence form
/// shared by every operation that raises the error — the read, the mutating and
/// the ARN-addressed tag paths alike — so the message is a function of the
/// request and of nothing else. Keep this the single place it is built: a
/// per-operation wording is exactly the divergence this helper exists to
/// prevent.
pub fn tunnel_not_found_message(tunnel_id: &str) -> String {
    format!("Tunnel {tunnel_id} does not exist.")
}

/// [`tunnel_not_found_message`] for a request that addressed the tunnel by ARN:
/// the message still names the tunnel by its id, lifted out of `arn`.
///
/// An ARN that names no tunnel resource carries no id to lift, so there the
/// supplied string itself is the only identifier the request offered and the
/// message names that.
pub fn tunnel_not_found_message_for_arn(arn: &str) -> String {
    tunnel_not_found_message(tunnel_id_from_arn(arn).unwrap_or(arn))
}

/// Unique, opaque bytes for a minted id or access token.
///
/// SHA-256 over three independent inputs: a process-wide monotonic counter (so
/// two mints in the same instant differ), the wall clock in nanoseconds (so two
/// processes differ), and the OS-seeded hash keys of a fresh
/// [`std::collections::hash_map::RandomState`] (so the bytes are not derivable
/// from the clock). Nothing here comes from the request, so a token is never a
/// function of client input.
///
/// This is adequate for an emulator's opaque values but it is not an OS CSPRNG:
/// if `getrandom`/`rand` is ever declared in the workspace manifest, mint from
/// it and delete this mixing.
fn minted_bytes() -> [u8; 32] {
    use sha2::{Digest, Sha256};
    use std::hash::{BuildHasher, Hasher};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|since| since.as_nanos())
        .unwrap_or_default();

    let mut keyed = std::collections::hash_map::RandomState::new().build_hasher();
    keyed.write_u64(seq);
    keyed.write_u128(nanos);

    let mut digest = Sha256::new();
    digest.update(seq.to_be_bytes());
    digest.update(nanos.to_be_bytes());
    digest.update(keyed.finish().to_be_bytes());
    digest.finalize().into()
}

/// Mint a fresh `tunnelId`.
///
/// A random 128-bit value rendered in the UUID-v4 text form AWS uses for a
/// tunnel id; it satisfies `TunnelId`'s modelled pattern
/// `^[a-zA-Z0-9_\-+=:]{1,128}$`.
pub fn mint_tunnel_id() -> String {
    let mut id = [0u8; 16];
    id.copy_from_slice(&minted_bytes()[..16]);
    // The bytes are this crate's ([`minted_bytes`]); the UUID layout over them is the
    // workspace's ONE rendering of 16 opaque bytes as a version-4 UUID, which computes
    // it with the maintained `uuid` crate. So neither the version and variant bits nor
    // the hyphenated text form is spelled out here to drift from that one.
    ::service_common::uuid_from_random_bytes(id)
}

/// Mint a fresh client access token: 256 opaque bits, hex-encoded.
///
/// `ClientAccessToken` is an unconstrained, `@sensitive` string, so the only
/// requirements are that it is opaque, that two mints differ, and that it is not
/// derived from the request.
pub fn mint_access_token() -> String {
    hex::encode(minted_bytes())
}

/// The listing a `ListTunnels` page token names a position in.
///
/// A creation sequence is a position in ONE listing and in no other, because
/// what a listing reports is decided by the Region whose endpoint serves it —
/// the only Region whose tunnels that endpoint holds at all — and by the
/// `thingName` it is filtered by. Both go into every token the listing mints
/// ([`encode_next_token`]) and are checked when a token comes back
/// ([`decode_next_token`]), so a cursor replayed against a different listing is
/// recognised as naming no position in it instead of being applied to it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TunnelListing<'a> {
    /// The Region of the endpoint serving the listing, as [`request_region`]
    /// reads it from the request.
    pub region: &'a str,
    /// The `thingName` the listing is filtered by, or `None` when it reports
    /// every tunnel of the Region.
    pub thing_name: Option<&'a str>,
}

impl TunnelListing<'_> {
    /// The 64 bits that identify this listing, carried by every token it mints.
    ///
    /// The leading 64 bits of SHA-256 over the listing's scope: a token stays
    /// short and opaque, while two different listings do not share a
    /// fingerprint in any store this emulator can hold. The Region is
    /// terminated by a NUL byte, which neither a Region label nor a `ThingName`
    /// (modelled `^[a-zA-Z0-9:_-]+$`) can contain, so no two distinct scopes
    /// hash the same bytes; the marker byte after it distinguishes "filtered by
    /// this name" from "not filtered at all".
    fn fingerprint(&self) -> u64 {
        use sha2::{Digest, Sha256};

        let mut digest = Sha256::new();
        digest.update(self.region.as_bytes());
        digest.update([0]);
        match self.thing_name {
            Some(thing_name) => {
                digest.update([1]);
                digest.update(thing_name.as_bytes());
            }
            None => digest.update([0]),
        }
        let digest = digest.finalize();
        // SHA-256's output is 32 bytes, so these 8 always exist.
        let mut leading = [0u8; 8];
        leading.copy_from_slice(&digest[..8]);
        u64::from_be_bytes(leading)
    }
}

/// The hex digits one field of a page token is rendered in. A `u64` written in
/// full, so the rendering is fixed-width and is its own canonical form.
const NEXT_TOKEN_FIELD_DIGITS: usize = 16;

/// The ONE rendering of a page token: the prefix, then the creation sequence the
/// previous page stopped at, then the fingerprint of the listing that minted it,
/// each zero-padded to [`NEXT_TOKEN_FIELD_DIGITS`] lowercase hex digits. The
/// whole token stays inside `NextToken`'s modelled pattern
/// `^[a-zA-Z0-9_=-]{1,4096}$`.
fn render_next_token(seq: u64, fingerprint: u64) -> String {
    format!(
        "{NEXT_TOKEN_PREFIX}{seq:0width$x}{fingerprint:0width$x}",
        width = NEXT_TOKEN_FIELD_DIGITS
    )
}

/// The `nextToken` that resumes `listing` after the tunnel whose creation
/// sequence is `seq`.
pub fn encode_next_token(listing: TunnelListing<'_>, seq: u64) -> String {
    render_next_token(seq, listing.fingerprint())
}

/// What a `nextToken` names when it is read against the listing being served.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PagePosition {
    /// The creation sequence the previous page of THIS listing stopped at: the
    /// listing resumes strictly after it.
    ResumeAfter(u64),
    /// A token of a different listing — another Region's endpoint, or another
    /// `thingName` filter. It names a position in that listing, and none here.
    OtherListing,
    /// Not a token this service can have minted.
    Unissued,
}

/// Read `token` as a position in `listing`.
///
/// Parsing the fields is not enough to accept one: `u64::from_str_radix` also
/// accepts a digit count, a sign and a letter case that [`render_next_token`]
/// never emits, so the parsed fields are re-rendered and the result compared
/// with what arrived — only a token identical to its own canonical rendering
/// was minted here, and every other string (truncated, re-cased, over-padded or
/// hand-written) is [`PagePosition::Unissued`]. A token that IS canonical still
/// names a position only in the listing whose fingerprint it carries.
pub fn decode_next_token(token: &str, listing: TunnelListing<'_>) -> PagePosition {
    let Some(digits) = token.strip_prefix(NEXT_TOKEN_PREFIX) else {
        return PagePosition::Unissued;
    };
    if digits.len() != 2 * NEXT_TOKEN_FIELD_DIGITS {
        return PagePosition::Unissued;
    }
    let (Some(sequence_digits), Some(fingerprint_digits)) = (
        digits.get(..NEXT_TOKEN_FIELD_DIGITS),
        digits.get(NEXT_TOKEN_FIELD_DIGITS..),
    ) else {
        return PagePosition::Unissued;
    };
    let (Ok(seq), Ok(fingerprint)) = (
        u64::from_str_radix(sequence_digits, 16),
        u64::from_str_radix(fingerprint_digits, 16),
    ) else {
        return PagePosition::Unissued;
    };
    if render_next_token(seq, fingerprint) != token {
        return PagePosition::Unissued;
    }
    if fingerprint == listing.fingerprint() {
        PagePosition::ResumeAfter(seq)
    } else {
        PagePosition::OtherListing
    }
}

/// The refusal for a `nextToken` that was minted by a DIFFERENT listing than
/// the one being served.
///
/// Such a token names a position in that other listing — another Region's
/// endpoint, or another `thingName` filter — and so names none here, exactly as
/// a string this service never minted does. The text names the two members a
/// caller has to keep stable while paging, so the request can be fixed from it
/// alone.
pub const NEXT_TOKEN_OTHER_LISTING_MESSAGE: &str =
    "nextToken was issued by a different listing: a page token resumes only the listing that \
     issued it, which is the Region the request was served in together with the thingName that \
     request filtered by";

/// `max_lifetime_timeout_minutes` as a duration, for computing a tunnel's
/// expiry.
///
/// The value is minutes. The model's `range(min: 1, max: 720)` is enforced
/// before any handler runs, so `minutes` is always positive here; the `max(1)`
/// only states that invariant rather than silently inventing a lifetime.
pub fn max_lifetime(minutes: i32) -> Duration {
    debug_assert!(
        minutes >= 1,
        "the model bounds maxLifetimeTimeoutMinutes at 1..720"
    );
    let minutes = u64::try_from(minutes.max(1)).unwrap_or(1);
    Duration::from_secs(minutes.saturating_mul(60))
}

/// The refusal message for a lifetime whose expiry instant cannot be
/// represented, i.e. one that [`max_lifetime`] cannot be added to the current
/// time without leaving the range `SystemTime` can hold.
///
/// `member` is the request member the lifetime arrived on and `minutes` is what
/// it carried, so the caller can fix the request from the text alone.
pub fn unrepresentable_expiry_message(member: &str, minutes: i32) -> String {
    format!("{member} of {minutes} minutes puts this tunnel's expiry out of range")
}
