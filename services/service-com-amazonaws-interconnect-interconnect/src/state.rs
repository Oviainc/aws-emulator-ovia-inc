// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-interconnect-interconnect service.
//!
//! Two resources live here:
//!
//! * **Environments** — AWS-managed. The model binds no `CreateEnvironment`
//!   operation, so the catalog is SEEDED once, when the engine first touches this
//!   state, and `ListEnvironments`/`GetEnvironment` read it. Both answer from the
//!   same records, so a `GetEnvironment` is byte-identical to the entry
//!   `ListEnvironments` returned for that id.
//! * **Connections** — caller-created. `CreateConnection` allocates one and every
//!   other connection operation reads or mutates that stored record.
//!
//! Server-owned fields (`id`, `arn`, `sharedId`, `activationKey`, `ownerAccount`,
//! `provider`, `location`, `type`, `billingTier`, `state`) are computed here or in
//! the handler, never copied from the request.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use crate::ops::types::{
    AttachPoint, AttachPointDescriptor, Bandwidths, Connection, ConnectionSummary, Environment,
    Provider, RemoteAccountIdentifier,
};

/// The region an ARN carries when the request is unsigned, so no SigV4 credential
/// scope names one. Matches the region the emulator's own clients default to.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The ARN namespace the model gives this service, which is the service segment of
/// every `Connection$arn` (`^arn:aws[a-z-]*:interconnect:…` per the member's pattern).
pub const ARN_NAMESPACE: &str = "interconnect";

/// The page size a list operation uses when the caller sends no `maxResults`.
///
/// `MaxResults` is modelled `@range(min: 1, max: 100)`, so the largest page a
/// caller can ask for is the natural default for one who asks for nothing.
pub const DEFAULT_PAGE_SIZE: usize = 100;

/// How long a connection stays in a transient, provisioning state (`deleting`
/// after `DeleteConnection`, `updating` after a bandwidth change) before the
/// clock settles it.
///
/// AWS documents no duration for either transition — only the states themselves —
/// so this is an emulator-chosen span inside the core's 250–750 ms window. It is
/// deliberately non-zero: a caller that reads straight back must still observe the
/// transient state and the refusals that go with it. The due instant is stored and
/// materialised on the next call; nothing ever sleeps on it.
pub const PROVISIONING_SPAN: Duration = Duration::from_millis(400);

/// How long a `deleted` connection stays readable before the store drops it.
///
/// A deleted connection has to stay readable for a while — the terminal state is
/// what a caller polling after `DeleteConnection` must see — but not forever, or a
/// caller that creates and deletes in a loop grows the store without bound. Once
/// the retention has elapsed the record and its two reverse-index entries are
/// removed by the same clock-driven pass that settles states, so a delete really
/// does release the memory and the identifier then reads as absent.
pub const DELETED_RECORD_RETENTION: Duration = Duration::from_secs(300);

/// The most connections an account HOLDS at once.
///
/// AWS enforces a per-account connection quota but publishes no number, so the
/// limit itself is emulator-chosen; what matters is that there IS one, and that
/// exceeding it is answered with the quota error the service models
/// (`ServiceQuotaExceededException`) rather than by growing the store for the life
/// of the process. A quota counts live resources, so a record that has reached the
/// terminal `deleted` state does not consume it — deleting a connection frees the
/// budget at once — and such records leave the store entirely after
/// [`DELETED_RECORD_RETENTION`], which is what bounds the memory.
pub const MAX_CONNECTIONS: usize = 200;

/// The most records the store holds at once, live and terminal together.
///
/// The quota above counts only live connections, so without a second bound a
/// create-and-delete loop could hold arbitrarily many `deleted` records until
/// their retention elapsed. This cap keeps the store bounded by COUNT as well as
/// by the clock: once it is reached, the oldest terminal record is dropped to make
/// room (a live connection is never dropped — the quota refuses those instead).
pub const MAX_STORED_RECORDS: usize = MAX_CONNECTIONS * 2;

/// How many times `CreateConnection` re-rolls a random `ConnectionId` before giving
/// up. Bounded so the allocation loop cannot spin on the shared lock; with eight
/// hex characters against at most [`MAX_CONNECTIONS`] records, exhausting it means
/// the random source is broken, which is a server fault.
pub const ID_ALLOCATION_ATTEMPTS: usize = 16;

/// A state change the clock owes a connection: at `due`, its state becomes `state`.
#[derive(Debug, Clone)]
pub struct PendingTransition {
    /// The instant the change became (or becomes) due.
    pub due: Instant,
    /// The `ConnectionState` the connection settles into.
    pub state: String,
}

/// A persisted connection: the modelled resource plus the bookkeeping that is not
/// part of any response shape.
#[derive(Debug)]
pub struct StoredConnection {
    /// The resource exactly as `GetConnection` returns it.
    pub connection: Connection,
    /// The remote-account identifier the creator supplied, which the partner
    /// verifies; `DescribeConnectionProposal` reads it.
    pub remote_account: Option<RemoteAccountIdentifier>,
    /// The transition the clock owes this connection, if any.
    pub pending_transition: Option<PendingTransition>,
    /// When this record leaves the store, set once it reaches `deleted`.
    pub evict_due: Option<Instant>,
}

/// What a `clientToken` was first used with, so a retry that carries the same
/// token but a DIFFERENT request is told so instead of being handed an unrelated
/// connection.
///
/// The request-owned members are kept here rather than read back off the stored
/// connection, because `UpdateConnection` may since have changed the record's
/// description or bandwidth: idempotency is about the request that was made, not
/// about the resource's current shape.
#[derive(Debug)]
pub struct IdempotentCreate {
    /// The connection that first call allocated.
    pub connection_id: String,
    /// `CreateConnection$environmentId` as it was sent.
    pub environment_id: String,
    /// `CreateConnection$bandwidth` as it was sent.
    pub bandwidth: String,
    /// `CreateConnection$attachPoint` as it was sent.
    pub attach_point: AttachPoint,
    /// `CreateConnection$description` as it was sent, absent included.
    pub description: Option<String>,
    /// `CreateConnection$tags` as they were sent, absent included.
    pub tags: Option<std::collections::HashMap<String, String>>,
}

impl IdempotentCreate {
    /// Whether a repeated request is the SAME request this token created a
    /// connection for.
    pub fn matches(
        &self,
        environment_id: &str,
        bandwidth: &str,
        attach_point: &AttachPoint,
        description: Option<&String>,
        tags: Option<&std::collections::HashMap<String, String>>,
    ) -> bool {
        self.environment_id == environment_id
            && self.bandwidth == bandwidth
            && attach_point_eq(&self.attach_point, attach_point)
            && self.description.as_ref() == description
            && self.tags.as_ref() == tags
    }
}

/// Per-service state for com-amazonaws-interconnect-interconnect. A handler reaches it through [`service_state`].
#[derive(Debug)]
pub struct ComAmazonawsInterconnectInterconnectState {
    /// The seeded Environment catalog, keyed by `environmentId`.
    pub environments: BTreeMap<String, Environment>,
    /// The attach points the CATALOG gives each Environment, keyed by
    /// `environmentId`. `ListAttachPoints` reads this and merges the Direct Connect
    /// gateways the caller owns into it, so a gateway the caller creates becomes a
    /// valid attach point without this seed being rewritten.
    pub attach_points: BTreeMap<String, Vec<AttachPointDescriptor>>,
    /// The live connections, keyed by `id`. A deleted connection is kept in the
    /// `deleted` state rather than dropped straight away, so a later
    /// `GetConnection` reports the terminal state instead of a 404 — and is
    /// evicted once [`DELETED_RECORD_RETENTION`] has elapsed. At most
    /// [`MAX_CONNECTIONS`] entries.
    pub connections: BTreeMap<String, StoredConnection>,
    /// `clientToken` -> what that token created, so a retried `CreateConnection`
    /// replays the first outcome instead of allocating a second connection, and a
    /// retry carrying the same token with different parameters is refused. Every
    /// entry names a connection in `connections`, and a dangling one is dropped on
    /// the next operation, so this map is bounded by [`MAX_STORED_RECORDS`] however
    /// many tokens a caller invents.
    pub idempotency_tokens: BTreeMap<String, IdempotentCreate>,
    /// `activationKey` -> connection id: the reverse index
    /// `DescribeConnectionProposal` / `AcceptConnectionProposal` resolve a
    /// partner-portal key through. One entry per stored connection, dropped with it,
    /// so it is bounded by [`MAX_STORED_RECORDS`] too.
    pub activation_keys: BTreeMap<String, String>,
}

impl Default for ComAmazonawsInterconnectInterconnectState {
    fn default() -> Self {
        let environments = seed_environments();
        let attach_points = seed_attach_points(&environments);
        Self {
            environments,
            attach_points,
            connections: BTreeMap::new(),
            idempotency_tokens: BTreeMap::new(),
            activation_keys: BTreeMap::new(),
        }
    }
}

impl ComAmazonawsInterconnectInterconnectState {
    /// Apply every state change the clock has already made due.
    ///
    /// Called at the top of each connection operation, including the reporting
    /// ones: a `deleting` connection becomes `deleted`, and an `updating` one
    /// settles back, at the instant stored when the call that started the change
    /// ran — whether or not anybody asked in between. It materialises only what was
    /// already owed, so it never moves a state a call did not start.
    pub fn materialise_due_transitions(&mut self, now: Instant) {
        let mut evicted: Vec<String> = Vec::new();
        for (id, stored) in &mut self.connections {
            let due = stored
                .pending_transition
                .as_ref()
                .is_some_and(|pending| pending.due <= now);
            if due {
                if let Some(pending) = stored.pending_transition.take() {
                    stored.connection.state = pending.state;
                    if stored.connection.state == "deleted" {
                        stored.evict_due = Some(now + DELETED_RECORD_RETENTION);
                    }
                }
            }
            if stored.evict_due.is_some_and(|evict_due| evict_due <= now) {
                evicted.push(id.clone());
            }
        }
        // A record whose retention has run out leaves the store with both of its
        // reverse-index entries, so neither index outlives the connection it names.
        for id in evicted {
            self.remove_connection(&id);
        }
        self.drop_dangling_index_entries();
    }

    /// Remove one connection and every index entry that names it, so the three maps
    /// are never out of step.
    fn remove_connection(&mut self, id: &str) {
        self.connections.remove(id);
        self.activation_keys
            .retain(|_, connection_id| connection_id != id);
        self.idempotency_tokens
            .retain(|_, created| created.connection_id != id);
    }

    /// Drop any index entry whose connection is no longer stored.
    ///
    /// This is what BOUNDS the two side indexes: every entry in `activation_keys`
    /// and `idempotency_tokens` names a connection in `connections`, and that map
    /// holds at most [`MAX_STORED_RECORDS`] records, so neither index can grow past
    /// that however many `clientToken`s a caller invents. Cheap (a scan of at most
    /// a few hundred entries) and called on every connection operation, so the
    /// invariant does not depend on each removal site remembering it.
    pub fn drop_dangling_index_entries(&mut self) {
        let connections = &self.connections;
        self.activation_keys
            .retain(|_, connection_id| connections.contains_key(connection_id));
        self.idempotency_tokens
            .retain(|_, created| connections.contains_key(&created.connection_id));
    }

    /// The state a connection settles into once its pending transition fires —
    /// its current state when nothing is pending.
    pub fn settled_state(stored: &StoredConnection) -> String {
        stored
            .pending_transition
            .as_ref()
            .map_or_else(|| stored.connection.state.clone(), |p| p.state.clone())
    }

    /// Make room for one more record: while the store is at [`MAX_STORED_RECORDS`],
    /// drop the terminal record whose retention expires first (the oldest one to
    /// have been deleted), together with its reverse-index entries. A live
    /// connection is never dropped, so a caller holding the quota is refused rather
    /// than quietly losing a connection.
    pub fn make_room_for_one(&mut self) {
        while self.connections.len() >= MAX_STORED_RECORDS {
            let oldest = self
                .connections
                .iter()
                .filter_map(|(id, stored)| stored.evict_due.map(|due| (due, id.clone())))
                .min()
                .map(|(_, id)| id);
            let Some(id) = oldest else {
                return;
            };
            self.remove_connection(&id);
        }
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsInterconnectInterconnectState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsInterconnectInterconnectState>)
}

/// The region this request was signed for, which is the region every ARN the
/// request mints carries. Falls back to [`DEFAULT_REGION`] for an unsigned request.
pub fn request_region(ctx: &smithy_server_runtime::OperationContext<'_>) -> String {
    ::emulator_server::sigv4_credential_scope(&ctx.parts().headers)
        .map_or_else(|| DEFAULT_REGION.to_string(), |scope| scope.region)
}

/// Two `Provider` values name the same partner on the same side of the union.
///
/// The union has no derived equality (its variants are the two partner kinds), and
/// `ListConnections`/`ListEnvironments` both filter on it.
pub fn provider_eq(left: &Provider, right: &Provider) -> bool {
    match (left, right) {
        (Provider::cloudServiceProvider(a), Provider::cloudServiceProvider(b))
        | (Provider::lastMileProvider(a), Provider::lastMileProvider(b)) => a == b,
        _ => false,
    }
}

/// Two `AttachPoint` values identify the same attach point, which is what the
/// `ListConnections` `attachPoint` filter compares.
pub fn attach_point_eq(left: &AttachPoint, right: &AttachPoint) -> bool {
    match (left, right) {
        (AttachPoint::directConnectGateway(a), AttachPoint::directConnectGateway(b))
        | (AttachPoint::Arn(a), AttachPoint::Arn(b)) => a == b,
        _ => false,
    }
}

/// The `ConnectionId` prefix an Environment's provider kind dictates.
///
/// The model's `Provider` members say it: a `cloudServiceProvider` environment
/// produces "Multicloud connections" (`mcc`), a `lastMileProvider` environment
/// "LastMile connections" (`lmcc`) — the two alternatives `ConnectionId`'s
/// `^(mcc|lmcc)-[a-z0-9]{8}$` pattern allows.
pub fn connection_id_prefix(provider: &Provider) -> &'static str {
    match provider {
        Provider::cloudServiceProvider(_) => "mcc",
        Provider::lastMileProvider(_) => "lmcc",
    }
}

/// A fresh `ConnectionId`: the provider-dictated prefix plus eight hex characters
/// (`[0-9a-f]`, taken from a v4 UUID's digits), which is inside the
/// `^(mcc|lmcc)-[a-z0-9]{8}$` alphabet the member's pattern allows.
pub fn new_connection_id(prefix: &str) -> String {
    let hex = uuid::Uuid::new_v4().simple().to_string();
    format!("{prefix}-{}", &hex[..8])
}

/// A fresh `ConnectionSharedId`: the hyphenated v4 UUID the member's pattern
/// (`^[0-9a-fA-F]{8}-…-[0-9a-fA-F]{12}$`, length exactly 36) describes.
pub fn new_shared_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

/// A fresh `ActivationKey` — the opaque token the creator brings to the partner
/// portal. 32 uppercase hex characters, well inside the modelled 1..2048 length.
pub fn new_activation_key() -> String {
    uuid::Uuid::new_v4()
        .simple()
        .to_string()
        .to_ascii_uppercase()
}

/// Project a stored `Connection` into the `ConnectionSummary` the list operation
/// returns, so both views report one record rather than two.
pub fn summary_of(connection: &Connection) -> ConnectionSummary {
    ConnectionSummary {
        id: connection.id.clone(),
        arn: connection.arn.clone(),
        description: connection.description.clone(),
        bandwidth: connection.bandwidth.clone(),
        attach_point: connection.attach_point.clone(),
        environment_id: connection.environment_id.clone(),
        provider: connection.provider.clone(),
        location: connection.location.clone(),
        r#type: connection.r#type.clone(),
        state: connection.state.clone(),
        shared_id: connection.shared_id.clone(),
        billing_tier: connection.billing_tier,
    }
}

/// The envelope this service puts on the client-facing refusals it words itself.
///
/// Every recorded Interconnect rejection message starts with it ("Unable to process
/// request: This environment model with id … could not be found", "Unable to process
/// request: Connection does not exist for …"), so the refusals below are built from
/// this one constant rather than each spelling the prefix out.
pub const REFUSAL_ENVELOPE: &str = "Unable to process request: ";

/// How the service words an `environmentId` (or `GetEnvironment$id`) that resolves to
/// no Environment.
///
/// An unresolvable environment is a CLIENT fault for this service — the caller named
/// a model that does not exist — so it is reported as `InterconnectClientException`
/// (HTTP 400), not as the connection-scoped `ResourceNotFoundException` (404), and the
/// message is built from the id the request carried.
pub fn environment_not_found_message(environment_id: &str) -> String {
    format!("{REFUSAL_ENVELOPE}This environment model with id {environment_id} could not be found")
}

/// How a connection READ (`GetConnection`, `DeleteConnection`) words an identifier
/// that names no stored connection: the retrieval could not be served.
pub fn connection_not_retrievable_message(identifier: &str) -> String {
    format!("{REFUSAL_ENVELOPE}Unable to retrieve connection {identifier}, as it does not exist")
}

/// How `UpdateConnection` words an identifier that names no stored connection.
///
/// A distinct construction from [`connection_not_retrievable_message`]: the update
/// path states that the connection does not exist for the identifier it was asked to
/// modify, which is the wording this operation is recorded answering.
pub fn connection_absent_message(identifier: &str) -> String {
    format!("{REFUSAL_ENVELOPE}Connection does not exist for {identifier}")
}

/// The `ConnectionState` values an `UpdateConnection` is permitted FROM.
///
/// The service states the rule in its own refusal ("only connections in 'requested'
/// or 'available' state can be updated"), so it is an allowlist: every other modelled
/// state — `pending`, `down`, `deleting`, `deleted`, `failed`, `updating` — is a
/// connection whose circuit is mid-flight or gone, and an update on it is refused.
pub const UPDATABLE_CONNECTION_STATES: [&str; 2] = ["requested", "available"];

/// Whether a connection in `state` may be updated at all.
pub fn is_updatable_state(state: &str) -> bool {
    UPDATABLE_CONNECTION_STATES.contains(&state)
}

/// How `UpdateConnection` words a refusal on a connection whose state does not permit
/// an update: the identifier from the request plus the permitted-state rule itself,
/// rendered from [`UPDATABLE_CONNECTION_STATES`] so the message cannot drift from the
/// guard that produced it.
pub fn connection_not_updatable_message(identifier: &str) -> String {
    let permitted: Vec<String> = UPDATABLE_CONNECTION_STATES
        .iter()
        .map(|state| format!("'{state}'"))
        .collect();
    format!(
        "{REFUSAL_ENVELOPE}Cannot update connection {identifier}: only connections in {} state can be updated",
        permitted.join(" or ")
    )
}

/// The tag `ListEnvironments` stamps on the cursors it issues.
pub const ENVIRONMENT_CURSOR_TAG: &str = "env:";
/// The tag `ListConnections` stamps on the cursors it issues.
pub const CONNECTION_CURSOR_TAG: &str = "conn:";
/// The tag `ListAttachPoints` stamps on the cursors it issues.
pub const ATTACH_POINT_CURSOR_TAG: &str = "ap:";

/// Read the key out of a page cursor this operation issued.
///
/// `None` for anything else — a hand-made token, or one from the other list
/// operation — which the handler refuses with the modelled validation error rather
/// than silently answering with an empty page.
pub fn decode_cursor<'a>(token: &'a str, tag: &str) -> Option<&'a str> {
    token.strip_prefix(tag).filter(|key| !key.is_empty())
}

/// Cut one page out of an ordered, already-filtered sequence of keyed items.
///
/// `items` must be in ascending key order (both stores are `BTreeMap`s, so
/// iteration already is). The cursor is the key of the last item handed out, and
/// the next page starts at the first key strictly greater than it — so a page is
/// never repeated or skipped even if the record a token names was deleted or
/// evicted by then. The returned token is `None` exactly when the page ran to the
/// end of the sequence; when there is one, it carries `token_tag` so the next call
/// can tell a cursor this operation issued from one it did not.
pub fn paginate<T>(
    items: Vec<(String, T)>,
    cursor: Option<&str>,
    max_results: Option<i32>,
    token_tag: &str,
) -> (Vec<T>, Option<String>) {
    let start = match cursor {
        Some(key_after) => items
            .iter()
            .position(|(key, _)| key.as_str() > key_after)
            .unwrap_or(items.len()),
        None => 0,
    };
    // Clamped at both ends on the server: `MaxResults` is modelled
    // `@range(min: 1, max: 100)`, and the page a caller gets is bounded here too
    // rather than only by the boundary validator.
    let limit = max_results.map_or(DEFAULT_PAGE_SIZE, |max| {
        max.clamp(1, DEFAULT_PAGE_SIZE as i32) as usize
    });

    let mut rest = items.into_iter().skip(start);
    let mut page = Vec::new();
    let mut last_key = None;
    for (key, item) in rest.by_ref().take(limit) {
        last_key = Some(key);
        page.push(item);
    }
    let token = if rest.next().is_some() {
        last_key.map(|key| format!("{token_tag}{key}"))
    } else {
        None
    };
    (page, token)
}

/// The Environment catalog this emulator presents.
///
/// AWS manages Environments itself and publishes no list, so the catalog is fixed
/// here. Every member the model marks `@required` is set, and the three
/// `EnvironmentState` values are all represented: a `limited` environment offers
/// fewer `available` bandwidths than it `supported`s (the state's own
/// documentation), and an `unavailable` one offers none, so `CreateConnection`
/// refuses on it.
///
/// `ProductType` is an unconstrained string, so the two values used here are taken
/// from the model's own wording: `"Multicloud"` is the `type` the
/// AcceptConnectionProposal / CreateConnection examples show for a
/// `cloudServiceProvider` environment, and `"LastMile"` is how
/// `Provider$lastMileProvider` names the other side's product ("LastMile
/// connections").
fn seed_environments() -> BTreeMap<String, Environment> {
    let catalog = vec![
        Environment {
            provider: Provider::cloudServiceProvider("Azure".to_string()),
            location: "us-east-1".to_string(),
            environment_id: "env-csp-azure-us-east-1".to_string(),
            state: "available".to_string(),
            bandwidths: Bandwidths {
                available: Some(vec!["1Gbps".to_string(), "10Gbps".to_string()]),
                supported: Some(vec![
                    "1Gbps".to_string(),
                    "10Gbps".to_string(),
                    "100Gbps".to_string(),
                ]),
            },
            r#type: "Multicloud".to_string(),
            activation_page_url: Some(
                "https://portal.example-azure.test/interconnect/activate".to_string(),
            ),
            remote_identifier_type: Some("account".to_string()),
        },
        Environment {
            provider: Provider::cloudServiceProvider("GoogleCloud".to_string()),
            location: "us-west-2".to_string(),
            environment_id: "env-csp-googlecloud-us-west-2".to_string(),
            state: "limited".to_string(),
            bandwidths: Bandwidths {
                available: Some(vec!["1Gbps".to_string()]),
                supported: Some(vec!["1Gbps".to_string(), "10Gbps".to_string()]),
            },
            r#type: "Multicloud".to_string(),
            activation_page_url: Some(
                "https://portal.example-googlecloud.test/interconnect/activate".to_string(),
            ),
            remote_identifier_type: Some("account".to_string()),
        },
        Environment {
            provider: Provider::cloudServiceProvider("OracleCloud".to_string()),
            location: "eu-west-1".to_string(),
            environment_id: "env-csp-oraclecloud-eu-west-1".to_string(),
            state: "available".to_string(),
            bandwidths: Bandwidths {
                available: Some(vec!["1Gbps".to_string(), "10Gbps".to_string()]),
                supported: Some(vec!["1Gbps".to_string(), "10Gbps".to_string()]),
            },
            r#type: "Multicloud".to_string(),
            activation_page_url: None,
            remote_identifier_type: None,
        },
        Environment {
            provider: Provider::lastMileProvider("Lumen".to_string()),
            location: "Ashburn, VA".to_string(),
            environment_id: "env-lmp-lumen-ashburn".to_string(),
            state: "available".to_string(),
            bandwidths: Bandwidths {
                available: Some(vec!["100Mbps".to_string(), "1Gbps".to_string()]),
                supported: Some(vec![
                    "100Mbps".to_string(),
                    "1Gbps".to_string(),
                    "10Gbps".to_string(),
                ]),
            },
            r#type: "LastMile".to_string(),
            activation_page_url: Some(
                "https://portal.example-lumen.test/interconnect/activate".to_string(),
            ),
            remote_identifier_type: Some("email".to_string()),
        },
        Environment {
            provider: Provider::lastMileProvider("Megaport".to_string()),
            location: "Sydney, NSW".to_string(),
            environment_id: "env-lmp-megaport-sydney".to_string(),
            state: "unavailable".to_string(),
            bandwidths: Bandwidths {
                available: Some(Vec::new()),
                supported: Some(vec!["1Gbps".to_string(), "10Gbps".to_string()]),
            },
            r#type: "LastMile".to_string(),
            activation_page_url: None,
            remote_identifier_type: Some("email".to_string()),
        },
    ];
    catalog
        .into_iter()
        .map(|environment| (environment.environment_id.clone(), environment))
        .collect()
}

/// The `AttachPointType` every seeded descriptor carries.
///
/// `AttachPointType` is an enum whose ONLY modelled value is
/// `DirectConnectGateway`, so it is the only type a descriptor can report even
/// though `AttachPointDescriptor$type`'s prose also mentions an ARN form.
pub const ATTACH_POINT_TYPE_DIRECT_CONNECT_GATEWAY: &str = "DirectConnectGateway";

/// How many attach points the n-th seeded Environment offers.
///
/// AWS publishes no count — an Environment offers whatever gateways the caller has
/// access to — so the shape of the seed is emulator-chosen. It varies with the
/// environment's position in the catalog (1, 2 or 3) so the catalog spans
/// environments whose attach points fit in one page and environments whose do not,
/// rather than making every `ListAttachPoints` page trivially the last one.
fn seeded_attach_point_count(index: usize) -> usize {
    1 + index % 3
}

/// The attach points each seeded Environment offers.
///
/// `AttachPointType`'s only modelled value is `DirectConnectGateway`, so each entry
/// names a Direct Connect gateway by the UUID that
/// `AttachPoint$directConnectGateway` requires. The UUID is DERIVED from the
/// environment id and the entry's ordinal (a name-based v5 UUID), so the seed cannot
/// fall out of step with the catalog however many environments it holds, and it is
/// stable across runs the way an AWS-owned resource id is.
fn seed_attach_points(
    environments: &BTreeMap<String, Environment>,
) -> BTreeMap<String, Vec<AttachPointDescriptor>> {
    environments
        .keys()
        .enumerate()
        .map(|(index, environment_id)| {
            let descriptors = (0..seeded_attach_point_count(index))
                .map(|ordinal| {
                    let gateway = uuid::Uuid::new_v5(
                        &uuid::Uuid::NAMESPACE_OID,
                        format!("{environment_id}#{ordinal}").as_bytes(),
                    );
                    AttachPointDescriptor {
                        r#type: ATTACH_POINT_TYPE_DIRECT_CONNECT_GATEWAY.to_string(),
                        identifier: gateway.to_string(),
                        name: format!("dxgw-{ordinal}-for-{environment_id}"),
                    }
                })
                .collect();
            (environment_id.clone(), descriptors)
        })
        .collect()
}
