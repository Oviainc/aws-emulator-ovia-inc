// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-transfer-transferservice service.
//!
//! Everything AWS Transfer Family's server + user lifecycle needs to answer a read
//! from stored state lives here: the two resource maps, the id-minting counter, the
//! clock-due server state transition, the ARN shapes, and the opaque pagination
//! token codec the `List*` operations share.
//!
//! `UserCount` and `SshPublicKeyCount` are DERIVED at read time ([`
//! ComAmazonawsTransferTransferserviceState::user_count`],
//! [`User::ssh_public_key_count`]) rather than stored, so they cannot go stale when a
//! user or a key is added or removed.

use std::collections::BTreeMap;
use std::time::{Duration, Instant, SystemTime};

use crate::ops::types::{
    EndpointDetails, HomeDirectoryMapEntry, IdentityProviderDetails, PosixProfile, ProtocolDetails,
    ResourceNotFoundException, S3StorageOptions, Tag, WorkflowDetails,
};

/// The region used for an ARN when the request carries no SigV4 credential scope to
/// read one from. Matches the region the emulator's generated SDK profile uses.
pub const DEFAULT_REGION: &str = "us-east-1";

/// `arn:aws:transfer:…` — this service's ARN namespace, per `OPERATIONS.md`.
const ARN_SERVICE: &str = "transfer";

/// The security policy a server gets when `CreateServer` names none.
///
/// AWS Transfer Family's documented default security policy for a new server; it
/// satisfies `SecurityPolicyName`'s modelled pattern
/// `^Transfer[A-Za-z0-9]*SecurityPolicy-[A-Za-z0-9-]+$`.
pub const DEFAULT_SECURITY_POLICY_NAME: &str = "TransferSecurityPolicy-2018-11-05";

/// `Domain`'s documented default ("The default value is S3").
pub const DEFAULT_DOMAIN: &str = "S3";

/// `IdentityProviderType`'s documented default ("The default value is SERVICE_MANAGED").
pub const DEFAULT_IDENTITY_PROVIDER_TYPE: &str = "SERVICE_MANAGED";

/// `EndpointType`'s default: a server with no VPC endpoint settings is reachable over
/// the public internet.
pub const DEFAULT_ENDPOINT_TYPE: &str = "PUBLIC";

/// `IpAddressType`'s documented default ("The default value is IPV4").
pub const DEFAULT_IP_ADDRESS_TYPE: &str = "IPV4";

/// `Protocols`' default: SFTP alone, the only protocol a `PUBLIC`,
/// `SERVICE_MANAGED` server can serve.
pub const DEFAULT_PROTOCOL: &str = "SFTP";

/// The page bound a `List*` call uses when the caller sends no `MaxResults`.
///
/// Equal to `MaxResults`' modelled maximum (`range 1..1000`), so an unbounded
/// request still cannot answer more than one modelled page.
pub const MAX_PAGE_SIZE: usize = 1000;

/// How many servers the store holds before `CreateServer` refuses with the modelled
/// `ServiceUnavailableException` instead of growing.
///
/// AWS Transfer Family publishes an account- and Region-level quota on servers that an
/// account can have raised, and this emulator models neither Service Quotas nor any
/// account's adjustments — so this is the emulator's OWN bound on the store, not a
/// claim about any account's limit. It is far above any workflow an emulator serves,
/// and it is what stops `servers` growing without end: every record it holds is
/// released by `DeleteServer`, which also drops that server's users and their keys, so
/// the user map cannot outlive the bound either.
pub const MAX_STORED_SERVERS: usize = 200;

/// How many users ONE server holds before `CreateUser` refuses with the modelled
/// `ServiceUnavailableException` instead of growing.
///
/// Chosen on the same basis as [`MAX_STORED_SERVERS`] — the emulator's own bound, not
/// an account's quota. Together the two cap the whole store at
/// `MAX_STORED_SERVERS * MAX_USERS_PER_SERVER` users, and each stored record is itself
/// bounded by the modelled lengths of the members it holds (`HomeDirectoryMappings`
/// `len 1..50000`, `Policy` `len 0..2048`, `SshPublicKeyBody` `len 0..2048`, `Tags`
/// `len 1..50`), which the framework's boundary validator enforces before a handler
/// runs. `DeleteUser` releases one record; `DeleteServer` releases all of a server's.
pub const MAX_USERS_PER_SERVER: usize = 1_000;

/// How long after `CreateServer` a new server leaves `STARTING` for `ONLINE`.
///
/// AWS documents the transition ("it can take a couple of minutes for the server to
/// be completely operational") but not a span the caller can rely on, so this span is
/// CHOSEN: long enough that a caller acting straight after `CreateServer` still
/// observes `STARTING`, short enough not to stall a test. The due instant is stored
/// and materialised on the next read; nothing sleeps for it.
const SERVER_START_SPAN: Duration = Duration::from_millis(500);

/// A server's `State` right after `CreateServer`: instantiated, not yet able to serve
/// file operations.
pub const SERVER_STATE_STARTING: &str = "STARTING";

/// A server's `State` once the start the create began has completed.
pub const SERVER_STATE_ONLINE: &str = "ONLINE";

/// The HTTP status AWS Transfer Family answers `ResourceNotFoundException` with.
///
/// The model puts `@httpError(404)` on the shape, but the recorded Transfer wire
/// answers an unknown server or user with HTTP **400** and `__type`
/// `ResourceNotFoundException` — so the status on the wire is 400 and the shape's
/// `httpError` does not describe the service. Handlers return it through
/// [`not_found`], which keeps the body the model's own and changes only the status
/// line.
pub const RESOURCE_NOT_FOUND_HTTP_STATUS: u16 = 400;

/// `ResourceType` for the server resource: the resource name capitalized, as the
/// recorded Transfer error bodies carry it.
pub const RESOURCE_TYPE_SERVER: &str = "Server";

/// `ResourceType` for the user resource. Same convention as [`RESOURCE_TYPE_SERVER`].
pub const RESOURCE_TYPE_USER: &str = "User";

/// The not-found payload for the `resource_type` resource identified by `resource`.
///
/// The identifier travels in `Resource` ALONE: `Message` is a fixed generic string
/// derived from the resource type, so one caller's id never reaches the message a
/// client logs or asserts on. Every operation in this service that cannot find a
/// resource builds its error here, so the construction cannot drift between them.
pub fn resource_not_found(resource_type: &str, resource: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: format!("Unknown {}", resource_type.to_lowercase()),
        resource: resource.to_string(),
        resource_type: resource_type.to_string(),
    }
}

/// The not-found payload for a `ServerId` the store does not hold — never created, or
/// already deleted.
pub fn unknown_server(server_id: &str) -> ResourceNotFoundException {
    resource_not_found(RESOURCE_TYPE_SERVER, server_id)
}

/// The not-found payload for a `UserName` the named server does not hold.
pub fn unknown_user(user_name: &str) -> ResourceNotFoundException {
    resource_not_found(RESOURCE_TYPE_USER, user_name)
}

/// Lift an operation's modelled not-found variant into its error channel, answered
/// with [`RESOURCE_NOT_FOUND_HTTP_STATUS`] rather than the model's `@httpError`.
///
/// Generic over the operation's own `*ErrorModeled` enum, so every handler reaches
/// the one status decision through this function.
pub fn not_found<E>(error: E) -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::modeled_with_status(error, RESOURCE_NOT_FOUND_HTTP_STATUS)
}

/// One stored SSH public key of a user.
#[derive(Debug, Clone)]
pub struct SshKey {
    /// The minted `key-` + 17 hex-digit identifier (`^key-[0-9a-f]{17}$`).
    pub ssh_public_key_id: String,
    /// The key material exactly as the caller supplied it.
    pub ssh_public_key_body: String,
    /// When this key was added to the user.
    pub date_imported: SystemTime,
}

/// One stored Transfer Family user, keyed by `(ServerId, UserName)`.
#[derive(Debug, Clone)]
pub struct User {
    pub server_id: String,
    pub user_name: String,
    pub arn: String,
    pub role: String,
    pub home_directory: Option<String>,
    pub home_directory_type: Option<String>,
    pub home_directory_mappings: Vec<HomeDirectoryMapEntry>,
    pub policy: Option<String>,
    pub posix_profile: Option<PosixProfile>,
    pub ssh_public_keys: Vec<SshKey>,
    pub tags: Vec<Tag>,
}

impl User {
    /// `SshPublicKeyCount`: the live number of keys stored for this user.
    pub fn ssh_public_key_count(&self) -> i32 {
        i32::try_from(self.ssh_public_keys.len()).unwrap_or(i32::MAX)
    }
}

/// One stored Transfer Family server, keyed by `ServerId`.
#[derive(Debug, Clone)]
pub struct Server {
    pub server_id: String,
    pub arn: String,
    pub domain: String,
    pub identity_provider_type: String,
    pub identity_provider_details: Option<IdentityProviderDetails>,
    pub endpoint_type: String,
    pub endpoint_details: Option<EndpointDetails>,
    pub protocols: Vec<String>,
    pub protocol_details: Option<ProtocolDetails>,
    pub logging_role: Option<String>,
    pub security_policy_name: String,
    pub pre_authentication_login_banner: Option<String>,
    pub post_authentication_login_banner: Option<String>,
    /// The Base64 SHA256 fingerprint of the server's host key.
    ///
    /// Always `None` today: deriving it means parsing the PEM private key
    /// `CreateServer`/`ImportHostKey` accept, and `ImportHostKey` is still a 501, so
    /// there is no honest value to report. Reported as absent rather than invented.
    pub host_key_fingerprint: Option<String>,
    pub structured_log_destinations: Vec<String>,
    pub s3_storage_options: Option<S3StorageOptions>,
    pub ip_address_type: String,
    pub workflow_details: Option<WorkflowDetails>,
    pub certificate: Option<String>,
    pub state: String,
    /// The state this server owes the clock, and when it comes due.
    ///
    /// Set by `CreateServer` to `(now + SERVER_START_SPAN, "ONLINE")`; materialised by
    /// [`Server::settle`] on the next read, never by sleeping.
    pub state_due: Option<(Instant, String)>,
    pub tags: Vec<Tag>,
}

impl Server {
    /// Apply the state transition the clock has already made due, if any.
    ///
    /// Called by the reporting operations that render `State`. It writes only a change
    /// that was owed at `now` whether or not anybody called, so two reads at the same
    /// instant answer the same thing.
    pub fn settle(&mut self, now: Instant) {
        if let Some((due, next)) = self.state_due.clone() {
            if now >= due {
                self.state = next;
                self.state_due = None;
            }
        }
    }
}

/// Per-service state for com-amazonaws-transfer-transferservice. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsTransferTransferserviceState {
    /// Servers by `ServerId` (`"s-" + 17 hex`). `BTreeMap` so `ListServers` pages in a
    /// stable, resumable order.
    pub servers: BTreeMap<String, Server>,
    /// Users by `(ServerId, UserName)`, so one server's users are a contiguous,
    /// ordered range for `ListUsers`.
    pub users: BTreeMap<(String, String), User>,
    /// Monotonic counter behind every minted id, so ids are unique and reproducible.
    pub next_id_seq: u64,
}

impl ComAmazonawsTransferTransferserviceState {
    /// Mint the next `<prefix>-<17 hex digits>` identifier.
    ///
    /// `ServerId` is `^s-([0-9a-f]{17})$` and `SshPublicKeyId` is `^key-[0-9a-f]{17}$`,
    /// so both are this shape; `{:017x}` is exactly 17 lowercase hex digits for every
    /// counter value below 16^17, which a counter incremented once per created
    /// resource cannot reach.
    pub fn mint_id(&mut self, prefix: &str) -> String {
        self.next_id_seq += 1;
        format!("{prefix}-{:017x}", self.next_id_seq)
    }

    /// The users stored for `server_id`, in `ListUsers` paging order.
    ///
    /// A RANGE scan, not a filter over the whole map: the key is
    /// `(ServerId, UserName)`, so one server's users are a contiguous run, and this
    /// walks only that run. One allocation (the range's lower bound) for the whole
    /// walk.
    pub fn users_of<'a>(&'a self, server_id: &'a str) -> impl Iterator<Item = &'a User> + 'a {
        use ::std::ops::Bound;
        let lower = (server_id.to_string(), String::new());
        self.users
            .range((Bound::Included(lower), Bound::Unbounded))
            .take_while(move |((sid, _), _)| sid == server_id)
            .map(|(_, user)| user)
    }

    /// `UserCount`: the live number of users assigned to `server_id`.
    ///
    /// Walks only that server's run of the user map. A caller that needs the count for
    /// SEVERAL servers (`ListServers`) must use [`Self::user_counts`] instead, so the
    /// work stays one pass in total rather than one walk per listed server.
    pub fn user_count(&self, server_id: &str) -> i32 {
        i32::try_from(self.users_of(server_id).count()).unwrap_or(i32::MAX)
    }

    /// Whether `server_id` can take another user, i.e. is below
    /// [`MAX_USERS_PER_SERVER`].
    pub fn server_has_user_capacity(&self, server_id: &str) -> bool {
        self.users_of(server_id).count() < MAX_USERS_PER_SERVER
    }

    /// Whether the store can take another server, i.e. is below
    /// [`MAX_STORED_SERVERS`].
    pub fn has_server_capacity(&self) -> bool {
        self.servers.len() < MAX_STORED_SERVERS
    }

    /// `UserCount` for every server that has users, in ONE pass over the user map.
    ///
    /// `ListServers` renders up to [`MAX_PAGE_SIZE`] servers per call; counting each
    /// one separately would cost `page x users`, a product no single bound covers.
    /// This makes the whole page cost `users + page`.
    pub fn user_counts(&self) -> BTreeMap<&str, i32> {
        let mut counts: BTreeMap<&str, i32> = BTreeMap::new();
        for (server_id, _) in self.users.keys() {
            let count = counts.entry(server_id.as_str()).or_insert(0);
            *count = count.saturating_add(1);
        }
        counts
    }

    /// Materialise every server state transition the clock has already made due.
    pub fn settle_servers(&mut self, now: Instant) {
        for server in self.servers.values_mut() {
            server.settle(now);
        }
    }

    /// The due instant for a server created at `now`, and the state it owes.
    pub fn server_start_due(now: Instant) -> (Instant, String) {
        (now + SERVER_START_SPAN, SERVER_STATE_ONLINE.to_string())
    }
}

/// `arn:aws:transfer:<region>:<account>:server/<ServerId>`.
pub fn server_arn(region: &str, server_id: &str) -> String {
    ::engine::emulator_arn(ARN_SERVICE, region, &format!("server/{server_id}"))
}

/// `arn:aws:transfer:<region>:<account>:user/<ServerId>/<UserName>`.
pub fn user_arn(region: &str, server_id: &str, user_name: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("user/{server_id}/{user_name}"),
    )
}

/// The region the caller signed this request for.
///
/// Read from the SigV4 credential scope
/// (`Credential=<akid>/<date>/<region>/<service>/aws4_request`), which is the only
/// place the region appears in a request to an emulated endpoint. Parsed with
/// `emulator_server::sigv4_credential_scope`, the framework's single source of
/// SigV4-scope parsing, so this agrees with how routing and telemetry read the same
/// header — including on rejecting a header that is not a well-formed
/// `AWS4-HMAC-SHA256` scope. Falls back to [`DEFAULT_REGION`] when there is no such
/// scope, so an unsigned request still gets a well-formed ARN.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// Encode the exclusive start key of the next page as an opaque `NextToken`.
///
/// Hex rather than the raw key so the token reads as opaque to a caller and carries
/// no character a JSON string or a URL has to escape; it decodes back to exactly one
/// key, so paging resumes deterministically.
pub fn encode_page_token(next_key: &str) -> String {
    ::hex::encode(next_key.as_bytes())
}

/// Decode a `NextToken` back to the exclusive start key.
///
/// `None` when the token was not produced by [`encode_page_token`] — the caller gets
/// `InvalidNextTokenException` rather than a silently restarted listing.
pub fn decode_page_token(token: &str) -> Option<String> {
    String::from_utf8(::hex::decode(token).ok()?).ok()
}

/// The index range of one page of an ordered key list, and the next page's token key.
///
/// `all` must be sorted (it comes straight out of a `BTreeMap`). `after` is the
/// exclusive start key decoded from the caller's `NextToken`; `limit` is the page
/// bound. Returns the half-open index range to render and, only when more remain, the
/// exclusive start key of the next page. Indices rather than cloned keys, so a caller
/// can slice the records it already holds instead of looking each one up again.
pub fn page_bounds(
    all: &[&str],
    after: Option<&str>,
    limit: usize,
) -> (::std::ops::Range<usize>, Option<String>) {
    let start = match after {
        // A token naming a key that has since been deleted still resumes right after
        // where it pointed, because this is a position, not a lookup.
        Some(key) => all.partition_point(|candidate| *candidate <= key),
        None => 0,
    };
    let end = start.saturating_add(limit).min(all.len());
    // The token is the LAST key of this page, which is exactly the exclusive start of
    // the next one — and only when a next one exists.
    let next = if end < all.len() {
        all.get(start..end)
            .and_then(<[&str]>::last)
            .map(|key| (*key).to_string())
    } else {
        None
    };
    (start..end, next)
}

/// Turn a caller's `MaxResults` into a page bound.
///
/// The modelled `range 1..1000` is already enforced before a handler runs, so an
/// out-of-range value never reaches here; an absent one means "no caller-imposed
/// bound", which is [`MAX_PAGE_SIZE`].
pub fn page_limit(max_results: Option<i32>) -> usize {
    max_results
        .and_then(|n| usize::try_from(n).ok())
        .filter(|n| *n > 0)
        .unwrap_or(MAX_PAGE_SIZE)
        .min(MAX_PAGE_SIZE)
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsTransferTransferserviceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsTransferTransferserviceState>)
}
