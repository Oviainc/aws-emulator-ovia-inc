// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-apigatewaymanagementapi-apigatewaymanagementapi service:
//! the WebSocket **connection registry** every operation of this service reads.
//!
//! # The registry is the whole service
//!
//! `ApiGatewayManagementApi` models exactly three operations — `PostToConnection`,
//! `GetConnection` and `DeleteConnection` — and all three address one WebSocket
//! connection by its id. Two refusals come out of this state, split along whether the
//! id is a connection IDENTIFIER of this service at all:
//!
//! * a `ConnectionId` the service never ISSUED
//!   ([`issued`](ComAmazonawsApigatewaymanagementapiApigatewaymanagementapiState::issued))
//!   identifies no connection, so it is refused as an invalid `ConnectionId` (HTTP 400
//!   `BadRequestException`, [`invalid_connection_id`]) before any connection is looked
//!   up;
//! * an id it issued that the
//!   [`connections`](ComAmazonawsApigatewaymanagementapiApigatewaymanagementapiState::connections)
//!   registry no longer holds is `GoneException` ("The connection with the provided id no
//!   longer exists"), **so a missing `connections` entry for an ISSUED id is the single
//!   source of truth for that error**.
//!
//! # The registration seam, and the rule it follows
//!
//! None of the three modeled operations CREATES a connection: in real AWS a
//! connection comes into being when a client completes a WebSocket handshake
//! against the API's own invoke endpoint — a data-plane event this management API
//! never serves. This emulator therefore registers a connection through exactly one
//! seam, [`register_connection`], which an emulator-internal caller (a test, or a
//! future WebSocket data plane for the API that owns the connection) calls on the
//! shared engine.
//!
//! The rule, stated once so no handler has to re-decide it:
//!
//! * A connection exists **only** after [`register_connection`] registered it, and
//!   stops existing the moment `DeleteConnection` removes it. There is deliberately
//!   NO auto-registration on first reference: auto-registration would make
//!   `GetConnection` answer 200 for an id that never existed, which is the one
//!   answer the real service never gives.
//! * `ConnectedAt` is fixed when the connection is registered and never moves
//!   again — re-registering an id that is already present changes nothing.
//! * `LastActiveAt` moves only when data actually flows to the connection, i.e. on a
//!   successful `PostToConnection`. `GetConnection` reports the registry and must
//!   not alter what a later caller sees, so it never bumps it.

use std::collections::{BTreeSet, HashMap};
use std::time::SystemTime;

pub use crate::ops::types::Identity;

/// One registered WebSocket connection: everything the three modeled operations
/// read or write about it.
///
/// `identity` reuses the modeled `Identity` shape (`SourceIp` / `UserAgent`) rather
/// than a second copy of those two members, so `GetConnection` returns the stored
/// record instead of rebuilding it.
#[derive(Debug, Clone)]
pub struct Connection {
    /// The `ConnectionId` (`@httpLabel`) value this record is keyed by.
    pub connection_id: String,
    /// When [`register_connection`] registered this connection. Serialized as
    /// `ConnectedAt`; never updated afterwards.
    pub connected_at: SystemTime,
    /// When data last flowed to this connection (a successful `PostToConnection`),
    /// or `connected_at` while none has. Serialized as `LastActiveAt`.
    pub last_active_at: SystemTime,
    /// The caller identity recorded for the connection at registration.
    pub identity: Identity,
    /// The most recently delivered `Data` payloads, in send order — a BOUNDED
    /// observation window, not a transcript.
    ///
    /// No modelled operation reads a connection's payloads, so this exists only for an
    /// emulator-internal caller ([`connection`]) to see what was delivered. Keeping
    /// every payload would let a caller grow shared engine state without limit (one
    /// `PostToConnection` may carry up to the modelled 131072 bytes, and the only
    /// release would be a `DeleteConnection` the same caller chooses to send), so
    /// [`Connection::push_delivered`] evicts the oldest entries until BOTH
    /// [`MAX_RETAINED_MESSAGES`] and [`MAX_RETAINED_BYTES`] hold. That caps the
    /// product of payload size and message count, which is what makes the store
    /// bounded.
    pub messages: Vec<Vec<u8>>,
    /// How many payloads `PostToConnection` has delivered to this connection in total,
    /// including ones [`Connection::messages`] has since evicted. A counter, so it
    /// stays exact without retaining bytes.
    pub delivered_count: u64,
    /// Total bytes delivered to this connection, including evicted payloads. Saturating,
    /// for the same reason.
    pub delivered_bytes: u64,
}

/// How many delivered payloads one connection retains for inspection.
///
/// An EMULATOR retention bound, not an AWS limit: AWS retains nothing a caller can read
/// back, so there is no documented duration or depth to honour here. Chosen large enough
/// that a caller can see the recent traffic it sent and small enough that the retained
/// bytes per connection are bounded by `MAX_RETAINED_BYTES` below.
pub const MAX_RETAINED_MESSAGES: usize = 16;

/// Total bytes of delivered payloads one connection retains for inspection (1 MiB).
///
/// The other half of the bound: with a per-payload maximum of 131072 bytes
/// (`crate::ops::post_to_connection::MAX_DATA_BYTES`), the retained bytes per connection
/// can never exceed this, however many posts arrive. Also an emulator bound, not an AWS
/// limit.
pub const MAX_RETAINED_BYTES: usize = 1024 * 1024;

impl Connection {
    /// Record one delivered payload: count it, add its bytes, keep it in the bounded
    /// observation window, and evict the oldest retained payloads until both
    /// [`MAX_RETAINED_MESSAGES`] and [`MAX_RETAINED_BYTES`] hold again.
    ///
    /// The counters are never evicted, so what was delivered stays exactly reportable
    /// while the retained bytes stay bounded.
    pub fn push_delivered(&mut self, payload: Vec<u8>) {
        self.delivered_count = self.delivered_count.saturating_add(1);
        self.delivered_bytes = self
            .delivered_bytes
            .saturating_add(payload.len() as u64);
        self.messages.push(payload);

        let mut retained_bytes: usize = self.messages.iter().map(Vec::len).sum();
        while self.messages.len() > MAX_RETAINED_MESSAGES
            || (retained_bytes > MAX_RETAINED_BYTES && self.messages.len() > 1)
        {
            retained_bytes -= self.messages.remove(0).len();
        }
    }
}

/// Per-service state for com-amazonaws-apigatewaymanagementapi-apigatewaymanagementapi. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsApigatewaymanagementapiApigatewaymanagementapiState {
    /// The connection registry, keyed by `ConnectionId`.
    pub connections: HashMap<String, Connection>,
    /// Every `ConnectionId` this service has ISSUED — the ids its connections are
    /// identified by, whether or not the connection is still open.
    ///
    /// A connection id is not an arbitrary string a caller may invent: API Gateway mints
    /// it when a client's handshake completes and the three `@connections` operations
    /// address a connection BY it, so the service can tell a connection identifier it
    /// issued from a value that is none. That distinction is what the two refusals are
    /// split along ([`issued`]): an identifier whose connection has closed is
    /// `GoneException`, and a value that never identified a connection is refused as an
    /// invalid `ConnectionId` before any connection is looked up.
    ///
    /// An id stays here after `DeleteConnection` removes the connection — that is what
    /// keeps a closed connection answering `GoneException` — so this set only grows, by
    /// one entry per connection [`register_connection`] registers.
    pub issued: BTreeSet<String>,
}

impl ComAmazonawsApigatewaymanagementapiApigatewaymanagementapiState {
    /// Whether `connection_id` is an id this service issued, i.e. one of its connections
    /// is or was identified by it. See [`Self::issued`].
    pub fn identifies_a_connection(&self, connection_id: &str) -> bool {
        self.issued.contains(connection_id)
    }
}

/// The sentence the three `@connections` operations refuse a `ConnectionId` that
/// identifies no connection of this service with: fixed text, then the `ConnectionId`
/// the request sent, verbatim.
///
/// One wording for all three, because the refusal is about the request's own
/// `ConnectionId` (`@httpLabel`) and not about what the operation would have done with
/// the connection.
pub fn invalid_connection_id_message(connection_id: &str) -> String {
    format!("Invalid connectionId: {connection_id}")
}

/// The refusal for a `ConnectionId` that identifies no connection of this service, as
/// the error channel of any of the three operations.
///
/// A value the service never issued is not a connection identifier at all, so it is a bad
/// request (HTTP 400 `BadRequestException`) rather than a connection that is gone: HTTP
/// 410 `GoneException` is "The connection with the provided id no longer exists", which
/// says the id DID identify a connection. A caller prunes its own record of a connection
/// on the 410, so answering it for an id that never existed makes that caller delete a
/// record it should keep.
pub fn invalid_connection_id<E>(connection_id: &str) -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::bad_request(invalid_connection_id_message(connection_id))
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsApigatewaymanagementapiApigatewaymanagementapiState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsApigatewaymanagementapiApigatewaymanagementapiState>)
}

/// The answer for a request whose engine does not carry this service's registry at
/// all — [`service_state`] returned `None`, so there is no state to compute any
/// response from.
///
/// This service models only client errors (403/410/429), none of which describes "the
/// store is not there", and answering one of them would tell the caller something
/// false about its own request. It renders as HTTP 501 instead, with wording of its
/// own: it is deliberately NOT
/// [`Scaffolded::not_implemented`](crate::scaffolded::Scaffolded::not_implemented),
/// whose message is the machine-readable "this operation is still a stub" record.
pub fn registry_unavailable<E>(operation: &str) -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::NotImplemented(crate::scaffolded::NotImplemented {
        message: format!(
            "the engine serving {operation} does not carry this service's connection registry"
        ),
    })
}

/// Register a WebSocket connection, the one seam that makes a connection exist.
///
/// `connected_at` is the instant the handshake completed; it becomes the record's
/// `ConnectedAt` and its initial `LastActiveAt`. Returns `true` when the connection
/// was registered, and `false` without touching anything when the id is already
/// registered (so `ConnectedAt` cannot be reset) or when `engine` does not carry this
/// service's state.
pub fn register_connection<Eng: Send + Sync + 'static>(
    engine: &Eng,
    connection_id: impl Into<String>,
    identity: Identity,
    connected_at: SystemTime,
) -> bool {
    let Some(state) = service_state(engine) else {
        return false;
    };
    let connection_id = connection_id.into();
    let mut guard = state.lock();
    if guard.connections.contains_key(&connection_id) {
        return false;
    }
    // Registering a connection is what ISSUES its id: from here on the id identifies a
    // connection of this service, and it goes on doing so after the connection closes.
    guard.issued.insert(connection_id.clone());
    guard.connections.insert(
        connection_id.clone(),
        Connection {
            connection_id,
            connected_at,
            last_active_at: connected_at,
            identity,
            messages: Vec::new(),
            delivered_count: 0,
            delivered_bytes: 0,
        },
    );
    true
}

/// The registry's record for `connection_id`, cloned, or `None` when no such
/// connection is registered.
///
/// The read side of the seam: it is how an emulator-internal caller observes what
/// `PostToConnection` delivered, since the service models no operation that returns
/// a connection's messages.
pub fn connection<Eng: Send + Sync + 'static>(
    engine: &Eng,
    connection_id: &str,
) -> Option<Connection> {
    service_state(engine)?.lock().connections.get(connection_id).cloned()
}
