// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-iotsecuretunneling-iotsecuredtunneling service.
//!
//! The service has exactly one resource — the tunnel — so this module holds the
//! tunnel store, the tag store keyed by tunnel ARN, and the small amount of
//! minting/projection logic more than one operation needs (ids, access tokens,
//! the `ListTunnels` page token, and the clock-owed close of a tunnel whose
//! `maxLifetimeTimeoutMinutes` has elapsed).

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::ops::types::Tag;

/// `TimeoutConfig$maxLifetimeTimeoutMinutes`: "The maximum amount of time (in
/// minutes) a tunnel can remain open. If not specified,
/// maxLifetimeTimeoutMinutes defaults to 720 minutes." (Smithy model doc for
/// `com.amazonaws.iotsecuretunneling#TimeoutConfig$maxLifetimeTimeoutMinutes`.)
pub const DEFAULT_MAX_LIFETIME_TIMEOUT_MINUTES: i32 = 720;

/// The page size `ListTunnels` applies when the caller sends no `maxResults`.
/// The model bounds `maxResults` at `range(min: 1, max: 100)`, so a full page is
/// 100 summaries and anything beyond it is reported through `nextToken`.
pub const MAX_LIST_PAGE_SIZE: i32 = 100;

/// The number of tunnel records this service stores.
///
/// AWS quotas tunnels per account — the modelled `LimitExceededException`
/// (HTTP 403) documents itself as "Thrown when a tunnel limit is exceeded" — so
/// an `OpenTunnel` past the limit is refused with that error rather than stored.
/// Enforcing it is also what keeps this store from growing without bound in a
/// long-lived emulator: each record's size is bounded by the modelled `@length`
/// and `@pattern` constraints on its members, and the record COUNT is bounded
/// here. A record leaves the store only through `CloseTunnel` with `delete`
/// set, which is the one call AWS documents as deleting a tunnel's data; the
/// number itself is this emulator's bound, since the model publishes none.
pub const MAX_TUNNELS: usize = 500;

/// The number of tags one tunnel may carry.
///
/// AWS's tagging convention across services is 50 tags per resource, and IoT
/// publishes that same per-resource limit. The modelled `TagList`
/// (`length(min: 1, max: 200)`) bounds ONE request's list but not the set a
/// resource accumulates over repeated `TagResource` calls, so the accumulated
/// key count is bounded here: a request that would take a tunnel past it is
/// refused and stores nothing. This is also what keeps the tag store from
/// growing without bound in a long-lived emulator — each key and value is
/// bounded by the modelled `@length` constraints on `Tag`, the keys per tunnel
/// by this constant, and the tunnels by [`MAX_TUNNELS`].
pub const MAX_TAGS_PER_TUNNEL: usize = 50;

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
    /// The instant the tunnel's `maxLifetimeTimeoutMinutes` runs out, after which
    /// the clock owes it a transition to `CLOSED`.
    pub expires_at: SystemTime,
    /// Creation order, so `ListTunnels` can report tunnels "by creation time in
    /// descending order" (the operation's own documentation) without depending on
    /// clock resolution for two tunnels opened in the same instant.
    pub creation_seq: u64,
}

/// Per-service state for com-amazonaws-iotsecuretunneling-iotsecuredtunneling. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsIotsecuretunnelingIotsecuredtunnelingState {
    /// Every tunnel, keyed by `tunnelId`.
    pub tunnels: BTreeMap<String, StoredTunnel>,
    /// The authoritative tag store, keyed by `tunnelArn` then by tag key. Shared
    /// by `OpenTunnel`, `TagResource`, `UntagResource`, `ListTagsForResource` and
    /// `DescribeTunnel`'s `tunnel.tags`.
    pub tags: BTreeMap<String, BTreeMap<String, String>>,
    /// `tunnelArn` -> `tunnelId`, so the tag operations resolve an ARN to a tunnel.
    pub arn_index: BTreeMap<String, String>,
    /// The soonest instant at which some OPEN tunnel's lifetime runs out, so the
    /// expiry sweep is a single comparison on the overwhelmingly common call
    /// where nothing is due yet, instead of a scan of the whole store under the
    /// shared lock. `None` means no tunnel is open.
    earliest_open_expiry: Option<SystemTime>,
    /// The next creation sequence number handed to a tunnel.
    next_creation_seq: u64,
}

impl ComAmazonawsIotsecuretunnelingIotsecuredtunnelingState {
    /// Claim the next creation sequence number.
    pub fn next_creation_seq(&mut self) -> u64 {
        let seq = self.next_creation_seq;
        self.next_creation_seq = seq.saturating_add(1);
        seq
    }

    /// Whether the store is full, i.e. whether another `OpenTunnel` must be
    /// refused with the modelled `LimitExceededException`. See [`MAX_TUNNELS`].
    pub fn is_at_tunnel_limit(&self) -> bool {
        self.tunnels.len() >= MAX_TUNNELS
    }

    /// Record a newly opened tunnel's expiry, so the sweep knows when to run.
    pub fn note_open_expiry(&mut self, expires_at: SystemTime) {
        self.earliest_open_expiry = Some(match self.earliest_open_expiry {
            Some(earliest) if earliest <= expires_at => earliest,
            _ => expires_at,
        });
    }

    /// The id of the tunnel `arn` names, if this account holds one.
    pub fn tunnel_id_for_arn(&self, arn: &str) -> Option<String> {
        self.arn_index.get(arn).cloned()
    }

    /// The number of distinct tag keys `arn` would carry after merging
    /// `incoming` into what is already stored. Stores nothing: a caller checks
    /// this against [`MAX_TAGS_PER_TUNNEL`] before writing.
    pub fn merged_tag_count(&self, arn: &str, incoming: &[Tag]) -> usize {
        let mut keys: ::std::collections::BTreeSet<&str> = self
            .tags
            .get(arn)
            .map(|stored| stored.keys().map(String::as_str).collect())
            .unwrap_or_default();
        keys.extend(incoming.iter().map(|tag| tag.key.as_str()));
        keys.len()
    }

    /// The tags stored for `arn`, as the modelled list — `None` when there are
    /// none, because `TagList` is `length(min: 1, max: 200)` and so cannot carry
    /// an empty list on the wire.
    pub fn tag_list(&self, arn: &str) -> Option<Vec<Tag>> {
        let tags = self.tags.get(arn)?;
        if tags.is_empty() {
            return None;
        }
        Some(
            tags.iter()
                .map(|(key, value)| Tag {
                    key: key.clone(),
                    value: value.clone(),
                })
                .collect(),
        )
    }

    /// Close every tunnel whose `maxLifetimeTimeoutMinutes` has run out.
    ///
    /// This is the one write a reporting operation may make: the transition was
    /// already owed at `now` by the clock — "The maximum amount of time (in
    /// minutes) a tunnel can remain open" — whether or not anybody called. It is
    /// idempotent, and it stamps `lastUpdatedAt` with the instant the transition
    /// came due rather than with the time of the call that noticed it.
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
                tunnel.status = TunnelStatus::Closed;
                tunnel.last_updated_at = tunnel.expires_at;
                tunnel.source_connection_state =
                    StoredConnectionState::disconnected(tunnel.expires_at);
                tunnel.destination_connection_state =
                    StoredConnectionState::disconnected(tunnel.expires_at);
            } else if earliest.is_none_or(|soonest| tunnel.expires_at < soonest) {
                earliest = Some(tunnel.expires_at);
            }
        }
        self.earliest_open_expiry = earliest;
    }
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

/// The region this request was signed for, or [`DEFAULT_REGION`] when it carries
/// no SigV4 credential scope to read one from.
///
/// Kept here rather than hoisted: the only crates above this one are the
/// read-only `vendor/` framework, and this workspace has no shared `services/`
/// crate (nor an editable root manifest to add one to), so a service-local
/// helper is the one place it can live.
pub fn request_region(ctx: &smithy_server_runtime::OperationContext<'_>) -> String {
    emulator_server::sigv4_credential_scope(&ctx.parts().headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The ARN resource prefix of a tunnel: a tunnel ARN addresses the resource as
/// `tunnel/<tunnelId>`.
const TUNNEL_ARN_RESOURCE_PREFIX: &str = "tunnel/";

/// The ARN of the tunnel `tunnel_id` in `region`.
pub fn tunnel_arn(region: &str, tunnel_id: &str) -> String {
    ::engine::emulator_arn(
        TUNNEL_ARN_SERVICE,
        region,
        &format!("{TUNNEL_ARN_RESOURCE_PREFIX}{tunnel_id}"),
    )
}

/// The `tunnelId` a resource ARN names, or `None` when `arn` is not of the form
/// `arn:<partition>:iot:<region>:<account>:tunnel/<tunnelId>`.
///
/// `TunnelId`'s modelled pattern `^[a-zA-Z0-9_\-+=:]{1,128}$` admits `:`, so the
/// resource segment is everything after the fifth colon rather than the sixth
/// colon-separated field.
pub fn tunnel_id_from_arn(arn: &str) -> Option<&str> {
    let resource = arn.splitn(6, ':').nth(5)?;
    let tunnel_id = resource.strip_prefix(TUNNEL_ARN_RESOURCE_PREFIX)?;
    (!tunnel_id.is_empty()).then_some(tunnel_id)
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
    // Version 4 and the RFC 4122 variant, so the id is well-formed as a UUID.
    id[6] = (id[6] & 0x0f) | 0x40;
    id[8] = (id[8] & 0x3f) | 0x80;
    let hex = hex::encode(id);
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

/// Mint a fresh client access token: 256 opaque bits, hex-encoded.
///
/// `ClientAccessToken` is an unconstrained, `@sensitive` string, so the only
/// requirements are that it is opaque, that two mints differ, and that it is not
/// derived from the request.
pub fn mint_access_token() -> String {
    hex::encode(minted_bytes())
}

/// The `nextToken` that resumes a `ListTunnels` page after the tunnel whose
/// creation sequence is `seq`.
pub fn encode_next_token(seq: u64) -> String {
    format!("{NEXT_TOKEN_PREFIX}{seq:016x}")
}

/// The creation sequence a `nextToken` resumes after, or `None` when the token
/// was not minted by [`encode_next_token`].
pub fn decode_next_token(token: &str) -> Option<u64> {
    let digits = token.strip_prefix(NEXT_TOKEN_PREFIX)?;
    u64::from_str_radix(digits, 16).ok()
}

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
