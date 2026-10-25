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
//!   other connection operation reads or mutates that stored record. This service's
//!   endpoints are regional, so each record belongs to the Region its create was
//!   signed for and is resolved only for requests signed for that Region.
//!
//! Attach points are NOT held here: an attach point is a Direct Connect gateway of the
//! account, so `ListAttachPoints` resolves them against the service that owns them
//! ([`direct_connect`]) rather than reading a catalog of this service's own — and
//! `CreateConnection` resolves the attach point it is given through that same walk, so
//! what a Connection may be attached to is what the listing offers.
//!
//! Server-owned fields (`id`, `arn`, `sharedId`, `activationKey`, `ownerAccount`,
//! `provider`, `location`, `type`, `billingTier`, `state`) are computed here or in
//! the handler, never copied from the request.

pub mod direct_connect;

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use crate::ops::types::{
    AttachPoint, Bandwidths, Connection, ConnectionSummary, Environment, Provider,
    RemoteAccountIdentifier,
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

/// How long ONE leg of a new connection's provisioning progression takes: the move
/// out of `requested`, and then the move out of `pending`.
///
/// AWS publishes no duration for either leg (the real move out of `requested` waits
/// on a human at the partner portal), so this is an emulator-chosen span inside the
/// core's 250–750 ms window, at its short end so a created connection reaches its
/// ready state promptly while a caller reading straight back still observes the
/// earlier state and the refusals that go with it. Each leg's due instant is stored
/// and materialised on the next call; nothing ever sleeps on it.
pub const ACTIVATION_SPAN: Duration = Duration::from_millis(250);


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

/// The transition the clock owes a connection that has just entered `state` at
/// `entered`, or `None` where `state` owes nothing.
///
/// This is the documented `ConnectionState` progression of a connection being
/// brought up — "requested: the initial state of a connection. The state will
/// remain here until the Connection is accepted on the Partner portal" ->
/// "pending: the connection has been accepted and is being provisioned between AWS
/// and the Partner" -> "available: the connection has been fully provisioned" — and
/// it is owed by the CLOCK here because the emulator has no partner portal for
/// anyone to accept at. Every other state is either terminal (`available`,
/// `deleted`), impaired by a circuit fault the emulator cannot have (`down`,
/// `failed`), or already owed a transition by the call that set it (`deleting`,
/// `updating`), so none of them is advanced from here.
///
/// Derived from the state alone, so EVERY path that leaves a connection in a
/// provisioning state hands it on along the same progression — the create that
/// allocates it, and the clock leg or settled update that lands it there.
pub fn owed_progression(state: &str, entered: Instant) -> Option<PendingTransition> {
    let next = match state {
        "requested" => "pending",
        "pending" => "available",
        _ => return None,
    };
    Some(PendingTransition {
        due: entered + ACTIVATION_SPAN,
        state: next.to_string(),
    })
}

/// A persisted connection: the modelled resource plus the bookkeeping that is not
/// part of any response shape.
#[derive(Debug)]
pub struct StoredConnection {
    /// The resource exactly as `GetConnection` returns it.
    pub connection: Connection,
    /// The Region this connection belongs to: the one the `CreateConnection` that
    /// allocated it was signed for, which its ARN also carries. Only a request
    /// signed for this Region sees the record ([`visible_in`]).
    pub region: String,
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

/// Whether `stored` is a connection a request signed for `region` can see.
///
/// This service's endpoints are regional (`https://interconnect.{Region}.{dnsSuffix}`),
/// so a connection belongs to the Region it was created in: every read, update and
/// delete issued against another Region's endpoint sees nothing, exactly as it sees
/// nothing for an identifier that was never allocated. ONE predicate, so every
/// resolution path below scopes the store the same way.
pub fn visible_in(stored: &StoredConnection, region: &str) -> bool {
    stored.region == region
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
    /// The live connections, keyed by `id`, each record carrying the Region it
    /// belongs to: an operation reaches them through [`Self::connection_in`] /
    /// [`Self::connections_in`], so one Region's endpoint never answers for
    /// another's connections. A deleted connection is kept in the
    /// `deleted` state rather than dropped straight away, so a later
    /// `GetConnection` reports the terminal state instead of a 404 — and is
    /// evicted once [`DELETED_RECORD_RETENTION`] has elapsed. At most
    /// [`MAX_CONNECTIONS`] entries.
    pub connections: BTreeMap<String, StoredConnection>,
    /// `(Region, clientToken)` -> what that token created, so a retried
    /// `CreateConnection` replays the first outcome instead of allocating a second
    /// connection, and a retry carrying the same token with different parameters is
    /// refused. Keyed by the Region too ([`idempotency_key`]): a token is scoped to
    /// the regional endpoint that first carried it, so the same token sent to another
    /// Region is one that endpoint has never seen. Every
    /// entry names a connection in `connections`, and a dangling one is dropped on
    /// the next operation, so this map is bounded by [`MAX_STORED_RECORDS`] however
    /// many tokens a caller invents.
    pub idempotency_tokens: BTreeMap<(String, String), IdempotentCreate>,
    /// `activationKey` -> connection id: the reverse index
    /// `DescribeConnectionProposal` / `AcceptConnectionProposal` resolve a
    /// partner-portal key through. One entry per stored connection, dropped with it,
    /// so it is bounded by [`MAX_STORED_RECORDS`] too. The connection the entry names
    /// is still resolved through [`Self::connection_in`], so a key minted in one
    /// Region resolves to nothing in another.
    pub activation_keys: BTreeMap<String, String>,
}

impl Default for ComAmazonawsInterconnectInterconnectState {
    fn default() -> Self {
        let environments = seed_environments();
        Self {
            environments,
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
    /// ones: a newly created connection is accepted and then provisioned, a
    /// `deleting` connection becomes `deleted`, and an `updating` one settles back,
    /// at the instant stored when the call that started the change ran — whether or
    /// not anybody asked in between. It materialises only what was already owed, so
    /// it never moves a state a call did not start.
    ///
    /// A progression of several legs ([`owed_progression`]) is materialised leg by
    /// leg in ONE pass, each leg due a span after the one before it rather than
    /// after `now`: a caller that waits out the whole progression and then asks
    /// sees the state the clock owed it, not just the next leg of it. The pass
    /// terminates whatever the store holds — a leg's successor is due strictly
    /// later than the leg itself, and [`owed_progression`] runs forward to a state
    /// that owes nothing — so a single call does bounded work.
    pub fn materialise_due_transitions(&mut self, now: Instant) {
        let mut evicted: Vec<String> = Vec::new();
        for (id, stored) in &mut self.connections {
            while stored
                .pending_transition
                .as_ref()
                .is_some_and(|pending| pending.due <= now)
            {
                let Some(pending) = stored.pending_transition.take() else {
                    break;
                };
                stored.connection.state = pending.state;
                // Whatever the connection has just become may itself owe the next
                // leg, timed from the instant this one was due.
                stored.pending_transition = owed_progression(&stored.connection.state, pending.due);
                if stored.connection.state == DELETED_CONNECTION_STATE {
                    stored.evict_due = Some(pending.due + DELETED_RECORD_RETENTION);
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

    /// The stored connection `identifier` names IN `region`, which is the only
    /// connection a request signed for that Region can reach by identifier.
    ///
    /// A connection belongs to the Region it was created in ([`visible_in`]), so an
    /// identifier allocated through another Region's endpoint resolves to nothing
    /// here — the same answer an identifier that was never allocated gets.
    pub fn connection_in(&self, region: &str, identifier: &str) -> Option<&StoredConnection> {
        self.connections
            .get(identifier)
            .filter(|stored| visible_in(stored, region))
    }

    /// [`Self::connection_in`] for the operations that MUTATE the record they
    /// resolve, scoping the store by the same predicate.
    pub fn connection_in_mut(
        &mut self,
        region: &str,
        identifier: &str,
    ) -> Option<&mut StoredConnection> {
        self.connections
            .get_mut(identifier)
            .filter(|stored| visible_in(stored, region))
    }

    /// Every connection a request signed for `region` sees, in `ConnectionId` order
    /// (`connections` is a `BTreeMap`, so iteration already is).
    pub fn connections_in<'a>(
        &'a self,
        region: &'a str,
    ) -> impl Iterator<Item = (&'a String, &'a StoredConnection)> {
        self.connections
            .iter()
            .filter(move |(_, stored)| visible_in(stored, region))
    }

    /// The stored connection an `AmazonResourceName` names, for the tagging
    /// operations that identify a resource by its ARN rather than by its id.
    ///
    /// The ARN identifies the resource in FULL — partition, region and account
    /// included — so an ARN whose fields differ from the stored one names a
    /// resource this store does not hold, exactly as an unknown id does. The
    /// Region a record belongs to is one of those fields, so matching the stored
    /// ARN is what scopes these operations to the Region the caller addressed
    /// (the handlers refuse an ARN from another scope before getting here). A
    /// connection that has reached the terminal `deleted` state is gone too: the
    /// store keeps the record readable so a caller polling a delete sees the
    /// terminal state, but a deleted resource has no tag set to read or change.
    pub fn connection_by_arn(&self, arn: &str) -> Option<&StoredConnection> {
        let id = connection_id_from_arn(arn)?;
        self.connections.get(id).filter(|stored| holds_arn(stored, arn))
    }

    /// [`Self::connection_by_arn`] for the tagging operations that WRITE the tag
    /// set, resolving the same resource under the same rule.
    pub fn connection_by_arn_mut(&mut self, arn: &str) -> Option<&mut StoredConnection> {
        let id = connection_id_from_arn(arn)?;
        self.connections
            .get_mut(id)
            .filter(|stored| holds_arn(stored, arn))
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

/// The key a `clientToken` is held under in [`ComAmazonawsInterconnectInterconnectState::idempotency_tokens`]:
/// the Region the request addressed, then the token itself.
pub fn idempotency_key(region: &str, token: &str) -> (String, String) {
    (region.to_string(), token.to_string())
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

/// How the service words an `attachPoint` that resolves to nothing a connection can
/// be attached to on the named Environment.
///
/// An attach point has to be one the caller has access to and that is valid for the
/// Environment — the set `ListAttachPoints` reports for it — so the message names the
/// identifier the request carried and the environment it was offered for, and is
/// built from the request rather than from the store.
pub fn attach_point_not_valid_message(identifier: &str, environment_id: &str) -> String {
    format!(
        "{REFUSAL_ENVELOPE}{identifier} is not an attach point valid for environment \
         {environment_id}"
    )
}

/// The `RemoteAccountIdentifierType` an Environment declares when its partner
/// verifies the remote side by AWS account number — one of the two values that enum
/// models (`REMOTE_ACCOUNT_IDENTIFIER_TYPE_VALUES`).
pub const REMOTE_IDENTIFIER_TYPE_ACCOUNT: &str = "account";

/// The `RemoteAccountIdentifierType` an Environment declares when its partner
/// verifies the remote side by email address — the other value that enum models.
pub const REMOTE_IDENTIFIER_TYPE_EMAIL: &str = "email";

/// Whether `identifier` is an AWS account number: the twelve digits an account id has.
///
/// The one place that rule lives: an `AmazonResourceName`'s account field is judged by
/// it too ([`parse_connection_arn`]).
fn is_account_identifier(identifier: &str) -> bool {
    identifier.len() == ARN_ACCOUNT_ID_DIGITS && identifier.chars().all(|c| c.is_ascii_digit())
}

/// Whether `identifier` is an email address: a non-empty local part, one `@`, and a
/// non-empty domain.
///
/// No more than that: neither the model nor the service's docs state an email grammar,
/// so what is judged is the `@`-separated SHAPE the kind means, and the domain's own
/// form is left to the partner that verifies it. Judging it here would refuse
/// addresses the real service takes.
fn is_email_identifier(identifier: &str) -> bool {
    let Some((local, domain)) = identifier.split_once('@') else {
        return false;
    };
    !local.is_empty() && !domain.is_empty() && !domain.contains('@')
}

/// Why `supplied` is not the `remoteAccount` `environment` asks a create for — or
/// `None` when it is.
///
/// `Environment$remoteIdentifierType` is "the type of identifying information that
/// should be supplied to the `remoteAccount` parameter of a `CreateConnection` call
/// for this specific Environment": an Environment that declares one asks every create
/// on it for identifying information OF THAT KIND, because that is what its partner
/// verifies the remote side by. One that declares none asks for nothing, so nothing is
/// judged and anything the caller sends is accepted.
///
/// `remoteAccount` is OPTIONAL in the model precisely because whether it is needed is
/// the Environment's to say, so ABSENCE is refused on an Environment that declares a
/// type for the same reason the wrong kind is: that Environment's partner would have
/// nothing to verify the remote side with, and the connection could never be confirmed
/// on its portal.
pub fn remote_account_violation(
    environment: &Environment,
    supplied: Option<&RemoteAccountIdentifier>,
) -> Option<String> {
    let declared = environment.remote_identifier_type.as_deref()?;
    let Some(RemoteAccountIdentifier::Identifier(identifier)) = supplied else {
        return Some(format!(
            "{REFUSAL_ENVELOPE}environment {} requires remoteAccount to carry the {declared} of \
             the remote side",
            environment.environment_id
        ));
    };
    let is_of_declared_kind = match declared {
        REMOTE_IDENTIFIER_TYPE_ACCOUNT => is_account_identifier(identifier),
        REMOTE_IDENTIFIER_TYPE_EMAIL => is_email_identifier(identifier),
        // A type outside the ones the enum models is nothing this service can hold an
        // identifier to, so one that is present is taken as given.
        _ => true,
    };
    (!is_of_declared_kind).then(|| {
        format!(
            "{REFUSAL_ENVELOPE}remoteAccount does not carry the {declared} environment {} \
             requires",
            environment.environment_id
        )
    })
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

/// The bounds the model puts on `BillingTier` (`@range(min: 0, max: 8)`), which is
/// every value the member may ever carry.
pub const BILLING_TIER_RANGE: (i32, i32) = (0, 8);

/// Project a stored `Connection` into the `ConnectionSummary` the list operation
/// returns, so both views report one record rather than two.
///
/// `billingTier` is carried across like any other stored member: the tier a
/// connection is assigned belongs to the record, so the summary view reports the
/// same value the full view does rather than deciding one of its own.
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

/// How `DescribeConnectionProposal` words an `activationKey` that resolves to no
/// stored proposal.
///
/// `ActivationKey` is `@sensitive`, so — unlike the identifier-bearing refusals
/// above — this message names the MEMBER and not the value the request carried: a
/// key echoed into an error body would be a secret in a log.
pub fn proposal_not_retrievable_message() -> String {
    format!(
        "{REFUSAL_ENVELOPE}Unable to retrieve a connection proposal for the supplied activationKey, as it does not exist"
    )
}

/// How `UpdateConnection` words an identifier that names no stored connection.
///
/// A distinct construction from [`connection_not_retrievable_message`]: the update
/// path states that the connection does not exist for the identifier it was asked to
/// modify, which is the wording this operation is recorded answering.
pub fn connection_absent_message(identifier: &str) -> String {
    format!("{REFUSAL_ENVELOPE}Connection does not exist for {identifier}")
}

/// The resource-type segment every `AmazonResourceName` of this service carries:
/// the model's pattern is `^arn:aws[a-z-]*:interconnect:[^:]+:[0-9]{12}:connection/…$`,
/// so a connection is the only resource kind an ARN here can name (which is what the
/// tagging operations mean by "currently this supports Connection resources").
pub const ARN_RESOURCE_TYPE: &str = "connection";

/// How many digits an ARN's account-id field has (`[0-9]{12}` in that pattern).
const ARN_ACCOUNT_ID_DIGITS: usize = 12;

/// The partition every ARN this emulator mints carries.
///
/// `emulator_arn` builds `arn:aws:<service>:<region>:<account>:<resource>`, so `aws`
/// is the partition the requests this emulator serves address, while the member's
/// `aws[a-z-]*` pattern lets a caller SEND any partition.
pub const ARN_PARTITION: &str = "aws";

/// Every field an Interconnect connection ARN carries, plus the ARN itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConnectionArn<'a> {
    /// The ARN as the request carried it.
    pub text: &'a str,
    /// The partition field (`aws`, `aws-cn`, …).
    pub partition: &'a str,
    /// The Region field.
    pub region: &'a str,
    /// The twelve-digit account field.
    pub account: &'a str,
    /// The `ConnectionId` the resource field names.
    pub id: &'a str,
}

/// The fields of an `AmazonResourceName`, for the operations that identify a
/// connection by its ARN (`TagResource`, `UntagResource`, `ListTagsForResource`)
/// rather than by its id.
///
/// Parsed from the ARN's six colon-separated fields — `arn`, a partition the
/// pattern writes `aws[a-z-]*`, this service's [`ARN_NAMESPACE`], a region, a
/// twelve-digit account and `connection/<id>` — so anything that is not an
/// Interconnect connection ARN resolves to `None` rather than to some other
/// service's resource. The id itself is NOT re-checked against
/// `^(mcc|lmcc)-[a-z0-9]{8}$` here: an id outside it simply matches no stored
/// connection.
pub fn parse_connection_arn(arn: &str) -> Option<ConnectionArn<'_>> {
    let (qualifier, id) = arn.rsplit_once('/')?;
    let mut fields = qualifier.split(':');
    if fields.next() != Some("arn") {
        return None;
    }
    let partition = fields.next().filter(|partition| {
        partition
            .strip_prefix("aws")
            .is_some_and(|rest| rest.chars().all(|c| c.is_ascii_lowercase() || c == '-'))
    })?;
    if fields.next() != Some(ARN_NAMESPACE) {
        return None;
    }
    let region = fields.next().filter(|region| !region.is_empty())?;
    let account = fields.next().filter(|account| is_account_identifier(account))?;
    if fields.next() != Some(ARN_RESOURCE_TYPE) || fields.next().is_some() || id.is_empty() {
        return None;
    }
    Some(ConnectionArn {
        text: arn,
        partition,
        region,
        account,
        id,
    })
}

/// The `ConnectionId` an `AmazonResourceName` names — [`parse_connection_arn`]'s
/// resource field, for the callers that need only the id.
pub fn connection_id_from_arn(arn: &str) -> Option<&str> {
    parse_connection_arn(arn).map(|parsed| parsed.id)
}

/// How the service words an `arn` that is not an Interconnect connection ARN at
/// all, which is a fault in the request rather than a missing resource.
pub fn not_a_connection_arn_message(arn: &str) -> String {
    format!("{REFUSAL_ENVELOPE}{arn} is not the ARN of an Interconnect {ARN_RESOURCE_TYPE}")
}

/// Why `parsed` does not address the scope the request it arrived on is for — or
/// `None` when its partition, Region and account are all that scope's.
///
/// An ARN identifies a resource in FULL, so one whose partition, Region or account
/// is not the one the request addresses names a resource outside the scope the
/// caller is talking to: a fault in the REQUEST, not a resource this scope is
/// missing. The message is built from the request — the field that differs, the
/// value the ARN carried for it and the value this request is for — so a caller can
/// see WHICH field put the ARN elsewhere.
pub fn arn_scope_violation(parsed: &ConnectionArn<'_>, region: &str) -> Option<String> {
    let account = ::emulator_server::emulator_account_id();
    let (field, carried, addressed) = [
        ("partition", parsed.partition, ARN_PARTITION),
        ("region", parsed.region, region),
        ("account", parsed.account, account),
    ]
    .into_iter()
    .find(|(_, carried, addressed)| carried != addressed)?;
    Some(format!(
        "{REFUSAL_ENVELOPE}{} is in {field} {carried}, and this request is for {field} {addressed}",
        parsed.text
    ))
}

/// The shortest and longest a tag KEY may be: `TagKey` is modelled
/// `@length(min: 1, max: 128)`, so an empty key and one past 128 are both outside
/// what a `TagMap` can carry.
pub const TAG_KEY_LENGTH: (usize, usize) = (1, 128);

/// The shortest and longest a tag VALUE may be: `TagValue` is modelled
/// `@length(min: 0, max: 256)`, so a value may be empty but not longer than 256.
pub const TAG_VALUE_LENGTH: (usize, usize) = (0, 256);

/// The tag-key namespace a caller does not own.
///
/// AWS assigns its own system tags keys that begin with this prefix and reserves it:
/// a tagging call may not create, change or remove a key inside it, so the tags a
/// request can touch are only the caller's own. `TagKey` carries no `@pattern`, so
/// the reservation is a service-level rule rather than a modelled constraint, and it
/// is matched without regard to case the way the reservation is stated.
pub const RESERVED_TAG_KEY_PREFIX: &str = "aws:";

/// Whether `key` lies in the reserved [`RESERVED_TAG_KEY_PREFIX`] namespace.
///
/// The head is taken with `get` rather than by slicing, so a key whose opening bytes
/// are part of a multi-byte character answers `false` instead of panicking.
pub fn is_reserved_tag_key(key: &str) -> bool {
    key.get(..RESERVED_TAG_KEY_PREFIX.len())
        .is_some_and(|head| head.eq_ignore_ascii_case(RESERVED_TAG_KEY_PREFIX))
}

/// Why `keys` are not keys this caller may write or remove, worded for the caller —
/// or `None` when none of them is reserved.
///
/// Every tagging path judges its keys here — tag-on-create, the tagging operation and
/// the untagging one — so the reservation holds however a key reached the request, and
/// a refused call writes nothing. The lowest reserved key decides the message, so the
/// same request is always refused for the same key, and that key and the reserved
/// prefix are what the message names.
pub fn reserved_tag_key_violation<'a>(keys: impl IntoIterator<Item = &'a str>) -> Option<String> {
    keys.into_iter()
        .filter(|key| is_reserved_tag_key(key))
        .min()
        .map(|key| {
            format!(
                "{REFUSAL_ENVELOPE}the tag key {key} is in the {RESERVED_TAG_KEY_PREFIX} namespace that AWS reserves for its own tags, which a caller cannot create, change or remove"
            )
        })
}

/// Why a `TagMap` the request carries does not satisfy the lengths the model gives
/// `TagKey` and `TagValue`, worded for the caller — or `None` when every entry is
/// inside them. The entry-count half of the rule lives in [`tag_write_violation`],
/// which is what the handlers call.
///
/// Entries are walked in key order so the same request is always refused for the
/// same entry, and the message names the member and the bound it broke rather than
/// quoting a value back at the caller.
fn tag_length_violation(tags: &std::collections::HashMap<String, String>) -> Option<String> {
    let mut entries: Vec<(&String, &String)> = tags.iter().collect();
    entries.sort_by(|left, right| left.0.cmp(right.0));
    for (key, value) in entries {
        // `@length` on a string counts characters, not bytes, so a non-ASCII key is
        // measured the way the model measures it.
        let key_length = key.chars().count();
        let (key_min, key_max) = TAG_KEY_LENGTH;
        if !(key_min..=key_max).contains(&key_length) {
            return Some(format!(
                "{REFUSAL_ENVELOPE}a tag key must be between {key_min} and {key_max} characters long, and one of length {key_length} is not"
            ));
        }
        let value_length = value.chars().count();
        let (value_min, value_max) = TAG_VALUE_LENGTH;
        if !(value_min..=value_max).contains(&value_length) {
            return Some(format!(
                "{REFUSAL_ENVELOPE}the tag value for key {key} must be between {value_min} and {value_max} characters long, and one of length {value_length} is not"
            ));
        }
    }
    None
}

/// The most tags ONE resource holds at once.
///
/// A tag map is written from the request, so without a bound on its entry count a
/// caller could keep adding fresh keys to the same connection until the store filled
/// memory — the record-count bounds above ([`MAX_CONNECTIONS`],
/// [`MAX_STORED_RECORDS`]) limit how many resources exist, not how large one gets.
/// The model puts no `@length` on `TagMap`, so the number itself is the one AWS
/// publishes for a resource's tag set; what matters is that there IS one and that
/// exceeding it is refused, with nothing written, rather than growing the store.
pub const MAX_TAGS_PER_RESOURCE: usize = 50;

/// How many tags a resource currently holding `existing` would hold once `incoming`
/// is merged into it: a key already present takes the new value instead of adding an
/// entry, so only the keys that are new count towards the growth.
fn merged_tag_count(
    existing: Option<&std::collections::HashMap<String, String>>,
    incoming: &std::collections::HashMap<String, String>,
) -> usize {
    let held = existing.map(|tags| tags.len()).unwrap_or(0);
    let added = incoming
        .keys()
        .filter(|key| existing.is_none_or(|tags| !tags.contains_key(*key)))
        .count();
    held + added
}

/// Why writing `incoming` onto a resource that currently holds `existing` is not a
/// tag set this service stores, worded for the caller — or `None` when the write is
/// within every bound.
///
/// Both of this service's tag writers (tag-on-create and the tagging operation) judge
/// their input here, so one rule decides what a stored tag set can be however it was
/// reached: every entry inside the lengths the model gives `TagKey` and `TagValue`,
/// no key in the reserved [`RESERVED_TAG_KEY_PREFIX`] namespace, and at most
/// [`MAX_TAGS_PER_RESOURCE`] entries on the resource once the write is
/// merged. A request outside either bound is a fault in the INPUT — the 400 the
/// service models for one, never a missing resource and never a 501 — and is refused
/// before anything is written, so a refused write leaves the resource exactly as it
/// was. The length check is needed because the boundary validator holds a LIST's
/// members to their target's constraints but not a MAP's keys.
pub fn tag_write_violation(
    existing: Option<&std::collections::HashMap<String, String>>,
    incoming: &std::collections::HashMap<String, String>,
) -> Option<String> {
    if let Some(message) = tag_length_violation(incoming) {
        return Some(message);
    }
    if let Some(message) = reserved_tag_key_violation(incoming.keys().map(String::as_str)) {
        return Some(message);
    }
    let merged = merged_tag_count(existing, incoming);
    if merged > MAX_TAGS_PER_RESOURCE {
        return Some(format!(
            "{REFUSAL_ENVELOPE}a {ARN_RESOURCE_TYPE} holds at most {MAX_TAGS_PER_RESOURCE} tags, and this request would leave it with {merged}"
        ));
    }
    None
}

/// The terminal `ConnectionState` a completed `DeleteConnection` settles into.
pub const DELETED_CONNECTION_STATE: &str = "deleted";

/// Whether `stored` is the live resource `arn` names: the stored ARN matches field
/// for field, and the connection has not reached the terminal deleted state.
fn holds_arn(stored: &StoredConnection, arn: &str) -> bool {
    stored.connection.arn == arn && stored.connection.state != DELETED_CONNECTION_STATE
}

/// The `ConnectionState` a connection whose proposal has not been accepted yet
/// reports ("requested: The initial state of a connection. The state will remain here
/// until the Connection is accepted on the Partner portal").
///
/// It is the one state an acceptance can be made from: before it there is no
/// connection to accept, and after it the acceptance has already happened.
pub const AWAITING_ACCEPTANCE_CONNECTION_STATE: &str = "requested";

/// The `ConnectionState` an accepted proposal's connection enters ("pending: The
/// connection has been accepted and is being provisioned between AWS and the
/// Partner"), which is the first leg of the progression the clock then owes it.
pub const ACCEPTED_CONNECTION_STATE: &str = "pending";

/// The `ConnectionState` values a connection whose proposal HAS been accepted, and
/// whose circuit has not since been torn down or failed, reports: the accepted state
/// itself, the fully provisioned one, the one a provisioned circuit that is not
/// passing traffic reports, and the one an in-flight change reports.
pub const ACCEPTED_CONNECTION_STATES: [&str; 4] =
    [ACCEPTED_CONNECTION_STATE, "available", "down", "updating"];

/// Whether `state` is one a connection reports once its proposal has been accepted.
pub fn is_accepted_state(state: &str) -> bool {
    ACCEPTED_CONNECTION_STATES.contains(&state)
}

/// Whether accepting `connection`'s proposal with `attach_point`, `description` and
/// `tags` would leave the connection exactly as it already is.
///
/// What makes a repeated acceptance the SAME acceptance: the attach point it asks the
/// connection to be associated with is the one the connection holds, and every member
/// it supplies is already the connection's. One that asks for anything else is a
/// different acceptance of a proposal that has already been accepted, so it is
/// refused instead of re-associating a connection that is already provisioning.
pub fn acceptance_already_applied(
    connection: &Connection,
    attach_point: &AttachPoint,
    description: Option<&String>,
    tags: Option<&std::collections::HashMap<String, String>>,
) -> bool {
    attach_point_eq(&connection.attach_point, attach_point)
        && description.is_none_or(|supplied| *supplied == connection.description)
        && tags.is_none_or(|supplied| {
            supplied.iter().all(|(key, value)| {
                connection
                    .tags
                    .as_ref()
                    .and_then(|held| held.get(key))
                    .is_some_and(|held| held == value)
            })
        })
}

/// How the service words an acceptance of a proposal whose connection has already
/// been accepted and asks for something that connection does not hold.
///
/// `ActivationKey` is `@sensitive`, so the message names the MEMBER the request
/// carried the key in and the connection the acceptance already made, never the key.
pub fn proposal_already_accepted_message(connection_id: &str) -> String {
    format!(
        "{REFUSAL_ENVELOPE}The connection proposal for the supplied activationKey has already \
         been accepted as connection {connection_id}, whose attach point cannot be changed"
    )
}

/// How the service words an acceptance of a proposal whose connection is no longer
/// one an acceptance can be made on — it is being deleted, deleted, or failed.
///
/// Built from the store rather than from the request: the state the connection is in
/// is what refuses the call, so the message names it, and the sensitive
/// `activationKey` is named only as a member.
pub fn proposal_not_acceptable_message(connection_id: &str, state: &str) -> String {
    format!(
        "{REFUSAL_ENVELOPE}The connection proposal for the supplied activationKey names connection \
         {connection_id}, which is {state}, so it can no longer be accepted"
    )
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

/// The `AttachPointType` every descriptor carries.
///
/// `AttachPointType` is an enum whose ONLY modelled value is
/// `DirectConnectGateway`, so it is the only type a descriptor can report even
/// though `AttachPointDescriptor$type`'s prose also mentions an ARN form.
pub const ATTACH_POINT_TYPE_DIRECT_CONNECT_GATEWAY: &str = "DirectConnectGateway";
