// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-codestarconnections-codestar-connections-20191201 service.
//!
//! This module owns the service's persisted resources (connections and their tags)
//! plus the few request-derived helpers every handler needs: the ARN namespace, the
//! region the caller signed for, and the `NextToken` codec the paginated list
//! operations share. `src/lib.rs` is generated, so a helper several handlers reach
//! lives here rather than in a module of its own.

use std::collections::hash_map::RandomState;
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::hash::BuildHasher;
use std::time::SystemTime;

use crate::ops::types::{Connection, Host, Tag, VpcConfiguration};

/// This service's ARN namespace (`awsService.arnNamespace` in the Smithy model), used
/// for every ARN the service mints.
pub const ARN_NAMESPACE: &str = "codestar-connections";

/// The status a connection created through the API carries.
///
/// The model's `Connection` doc: "A connection created through CloudFormation, the
/// CLI, or the SDK is in `PENDING` status by default. You can make its status
/// `AVAILABLE` by updating the connection in the console." The handshake that moves a
/// connection to `AVAILABLE` is a console action, not something the clock owes, so a
/// connection this emulator mints stays `PENDING` until something updates it.
pub const CONNECTION_STATUS_PENDING: &str = "PENDING";

/// The statuses a host carries.
///
/// From the model's `Host$Status` doc ("The status of the host, such as PENDING,
/// AVAILABLE, VPC_CONFIG_DELETING, VPC_CONFIG_INITIALIZING, and
/// VPC_CONFIG_FAILED_INITIALIZATION", `smithy_query(query="shape", shape="Host")`).
/// `HostStatus` is modeled as a free-form string (`len 1..64`) rather than an enum, so
/// these are the values the doc names rather than a closed set the wire enforces.
pub const HOST_STATUS_PENDING: &str = "PENDING";
/// The status description a host carries while it is `PENDING`.
///
/// `CreateHost`'s own doc states what a pending host is waiting for: "A host created
/// through the CLI or the SDK is in `PENDING` status by default. You can make its
/// status `AVAILABLE` by setting up the host in the console." `Host$StatusMessage` is
/// modeled "The status description for the host" — so while the host sits in that
/// non-terminal status the service has a description to report, and this is it. The
/// WORDING is the emulator's (AWS documents none); what the API contract carries is
/// that a pending host reports a description at all.
pub const HOST_STATUS_MESSAGE_PENDING: &str =
    "Host is pending. Complete the host setup in the console to make it available.";

/// See [`HOST_STATUS_PENDING`].
pub const HOST_STATUS_AVAILABLE: &str = "AVAILABLE";
/// See [`HOST_STATUS_PENDING`].
pub const HOST_STATUS_VPC_CONFIG_DELETING: &str = "VPC_CONFIG_DELETING";
/// See [`HOST_STATUS_PENDING`].
pub const HOST_STATUS_VPC_CONFIG_INITIALIZING: &str = "VPC_CONFIG_INITIALIZING";
/// See [`HOST_STATUS_PENDING`].
pub const HOST_STATUS_VPC_CONFIG_FAILED_INITIALIZATION: &str = "VPC_CONFIG_FAILED_INITIALIZATION";

/// Every status [`HOST_STATUS_PENDING`]'s doc names, in the order it names them.
pub const HOST_STATUS_VALUES: &[&str] = &[
    HOST_STATUS_PENDING,
    HOST_STATUS_AVAILABLE,
    HOST_STATUS_VPC_CONFIG_DELETING,
    HOST_STATUS_VPC_CONFIG_INITIALIZING,
    HOST_STATUS_VPC_CONFIG_FAILED_INITIALIZATION,
];

/// The statuses a host cannot be deleted from.
///
/// `DeleteHost`'s own doc states the rule: "A host cannot be deleted if it is in the
/// VPC_CONFIG_INITIALIZING or VPC_CONFIG_DELETING state." Both are transitional VPC
/// provisioning states, so the refusal is the "the host is there but is not usable
/// right now" signal (`ResourceUnavailableException`) rather than an absence.
pub const HOST_STATUS_DELETE_BLOCKED: &[&str] = &[
    HOST_STATUS_VPC_CONFIG_INITIALIZING,
    HOST_STATUS_VPC_CONFIG_DELETING,
];

/// Region used when a request carries no SigV4 credential scope to read one from.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The page size `ListConnections` applies when the caller asks for none.
///
/// `MaxResults` is modeled `range 0..100` with `default 0`, so an omitted member
/// arrives as `0` — indistinguishable from "no page size requested". The modeled
/// maximum is the largest page the operation may ever return, so it is what an
/// unconstrained list uses.
pub const MAX_PAGE_SIZE: usize = 100;

/// The largest number of connections this emulator holds at once.
///
/// The service itself caps them: the model declares `LimitExceededException` on
/// `CreateConnection` with the doc "Exceeded the maximum limit for connections.", so
/// refusing past a maximum is the modeled behaviour. The real per-account quota is
/// adjustable and the model does not state it, so this value is the EMULATOR's bound —
/// chosen high enough that no realistic client workflow meets it, and finite so a
/// long-lived process cannot grow its state without limit.
pub const MAX_CONNECTIONS: usize = 1000;

/// The largest number of hosts this emulator holds at once.
///
/// A CHOSEN bound, exactly like [`MAX_CONNECTIONS`] and for the same reason: the store
/// lives in this process, so the number of records a caller can accumulate has to be
/// capped somewhere. `CreateHost` refuses past it with `LimitExceededException` (429),
/// the only error that operation models.
pub const MAX_HOSTS: usize = 1000;

/// The largest number of tags one resource holds.
///
/// `TagList` is modeled `len 0..200`, so 200 is the most tags the service ever accepts
/// in one request; keeping the stored map to that same maximum means a sequence of
/// merges cannot grow a resource past what a single request could set. Over the limit,
/// `TagResource` answers the `LimitExceededException` it declares.
pub const MAX_TAGS_PER_RESOURCE: usize = 200;

/// A connection as this service persists it.
///
/// The wire `Connection` shape is rebuilt from these fields by every reader, so a
/// create followed by a get or a list answers out of this record and nothing else.
#[derive(Debug, Clone)]
pub struct StoredConnection {
    /// The connection's generated id — the last segment of its ARN.
    pub connection_id: String,
    /// `ConnectionName` as supplied at create.
    pub connection_name: String,
    /// The ARN this connection is keyed by.
    pub connection_arn: String,
    /// `ProviderType`; the model does not require it at create, so it may be absent.
    pub provider_type: Option<String>,
    /// The 12-digit account id that owns the connection.
    pub owner_account_id: String,
    /// One of `CONNECTION_STATUS_VALUES`.
    pub connection_status: String,
    /// `HostArn`, only when one was supplied at create.
    pub host_arn: Option<String>,
    /// When the connection was created. Recorded resource metadata; the stable list
    /// order is `sequence`, which cannot be perturbed by a clock that steps back.
    pub created_at: SystemTime,
    /// Creation order, and the value `NextToken` resumes from.
    pub sequence: u64,
}

impl StoredConnection {
    /// This record as the modeled `Connection` structure every reader returns.
    ///
    /// `HostArn` stays absent when the connection was created without one, rather
    /// than being reported as an empty string.
    pub fn to_connection(&self) -> Connection {
        Connection {
            connection_name: Some(self.connection_name.clone()),
            connection_arn: Some(self.connection_arn.clone()),
            provider_type: self.provider_type.clone(),
            owner_account_id: Some(self.owner_account_id.clone()),
            connection_status: Some(self.connection_status.clone()),
            host_arn: self.host_arn.clone(),
        }
    }
}

/// A host as this service persists it.
///
/// One record per `HostArn`, holding every member of the modeled `Host` shape the API
/// can observe, so an operation that reports or deletes a host works off this one row
/// rather than recomputing one. `CreateHost` writes the row, `GetHost` renders it
/// through [`StoredHost::to_host`], and `DeleteHost` reads [`StoredHost::status`] (the
/// delete is refused while the host's VPC configuration is in flight) before removing
/// the whole record. `ListHosts` and `UpdateHost` report and update this same row (each
/// still a scaffold with its own open issue).
#[derive(Debug, Clone)]
pub struct StoredHost {
    /// `Name` as supplied at create.
    pub name: String,
    /// The ARN this host is keyed by.
    pub host_arn: String,
    /// One of `PROVIDER_TYPE_VALUES`; `CreateHost` models it required.
    pub provider_type: String,
    /// The endpoint of the infrastructure the host represents.
    pub provider_endpoint: String,
    /// The VPC configuration provisioned for the host, when one was supplied.
    pub vpc_configuration: Option<VpcConfiguration>,
    /// The host's `HostStatus` — one of `HOST_STATUS_VALUES`.
    pub status: String,
    /// The status description, when the service has one to report.
    pub status_message: Option<String>,
}

impl StoredHost {
    /// This record as the modeled `Host` structure every reader renders from.
    ///
    /// `GetHost` projects this onto its own (flattened, `HostArn`-less and
    /// `StatusMessage`-less) output shape, and `ListHosts` reports it whole, so both
    /// read ONE rendering of a stored host rather than each recomputing one.
    pub fn to_host(&self) -> Host {
        Host {
            name: Some(self.name.clone()),
            host_arn: Some(self.host_arn.clone()),
            provider_type: Some(self.provider_type.clone()),
            provider_endpoint: Some(self.provider_endpoint.clone()),
            vpc_configuration: self.vpc_configuration.clone(),
            status: Some(self.status.clone()),
            status_message: self.status_message.clone(),
        }
    }

    /// Whether this host's status forbids deleting it — see
    /// [`HOST_STATUS_DELETE_BLOCKED`].
    pub fn delete_blocked_by_status(&self) -> bool {
        HOST_STATUS_DELETE_BLOCKED.contains(&self.status.as_str())
    }
}

/// Per-service state for com-amazonaws-codestarconnections-codestar-connections-20191201. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsCodestarconnectionsCodestarConnections20191201State {
    /// Every connection in this emulator, keyed by its ARN.
    pub connections: BTreeMap<String, StoredConnection>,
    /// Every host in this emulator, keyed by its ARN. A connection may name one
    /// through `StoredConnection::host_arn`, which is the association `DeleteHost`
    /// refuses to break.
    pub hosts: BTreeMap<String, StoredHost>,
    /// Tags by resource ARN, shared by `TagResource` / `UntagResource` /
    /// `ListTagsForResource` and populated from `CreateConnection.Tags`.
    pub tags: BTreeMap<String, BTreeMap<String, String>>,
    /// The next creation-order sequence number to hand out.
    pub next_sequence: u64,
}

impl ComAmazonawsCodestarconnectionsCodestarConnections20191201State {
    /// Take the next creation-order sequence number.
    pub fn take_sequence(&mut self) -> u64 {
        let sequence = self.next_sequence;
        self.next_sequence = self.next_sequence.saturating_add(1);
        sequence
    }

    /// Every connection in creation order — the order `ListConnections` pages over.
    ///
    /// Borrowed, not cloned: the request sorts POINTERS (an O(n log n) pass over the
    /// store's size) and clones only the page it returns.
    pub fn connections_in_creation_order(&self) -> Vec<&StoredConnection> {
        let mut connections: Vec<&StoredConnection> = self.connections.values().collect();
        connections.sort_unstable_by_key(|connection| connection.sequence);
        connections
    }

    /// How many connections are currently associated with `host_arn`.
    ///
    /// `DeleteHost`'s doc states the precondition this answers: "Before you delete a
    /// host, all connections associated to the host must be deleted." The association
    /// lives on the CONNECTION (`StoredConnection::host_arn`, set at create), so it is
    /// read out of the connection store rather than held a second time on the host, and
    /// counted in place: the caller needs how many still hold the host, not which.
    pub fn connections_attached_to_host_count(&self, host_arn: &str) -> usize {
        self.connections
            .values()
            .filter(|connection| connection.host_arn.as_deref() == Some(host_arn))
            .count()
    }

    /// Whether this service holds a resource under `resource_arn`.
    ///
    /// The tag operations (`TagResource` / `UntagResource` / `ListTagsForResource`) are
    /// modeled over a `ResourceArn` rather than over one resource kind, so each
    /// resolves the ARN through this one predicate: EVERY resource kind this service
    /// persists is tag-addressable, and an ARN naming none of them is the
    /// `ResourceNotFoundException` those operations declare. `CreateHost` and
    /// `CreateConnection` both accept `Tags`, so both kinds have to resolve here.
    pub fn holds_resource(&self, resource_arn: &str) -> bool {
        self.connections.contains_key(resource_arn) || self.hosts.contains_key(resource_arn)
    }

    /// How many distinct keys `resource_arn` would hold after merging `tags`.
    pub fn tag_count_after_merge(&self, resource_arn: &str, tags: &[Tag]) -> usize {
        match self.tags.get(resource_arn) {
            None => tags
                .iter()
                .map(|tag| tag.key.as_str())
                .collect::<::std::collections::BTreeSet<&str>>()
                .len(),
            Some(stored) => {
                let mut keys: ::std::collections::BTreeSet<&str> =
                    stored.keys().map(String::as_str).collect();
                keys.extend(tags.iter().map(|tag| tag.key.as_str()));
                keys.len()
            }
        }
    }

    /// The tags held for `resource_arn`, as the modeled `TagList`.
    pub fn tag_list(&self, resource_arn: &str) -> Vec<Tag> {
        self.tags
            .get(resource_arn)
            .map(|tags| {
                tags.iter()
                    .map(|(key, value)| Tag {
                        key: key.clone(),
                        value: value.clone(),
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Merge `tags` onto whatever `resource_arn` already carries; a repeated key
    /// overwrites.
    pub fn merge_tags(&mut self, resource_arn: &str, tags: &[Tag]) {
        let stored = self.tags.entry(resource_arn.to_string()).or_default();
        for tag in tags {
            stored.insert(tag.key.clone(), tag.value.clone());
        }
    }
}

/// Whether `region` is syntactically an AWS region code.
///
/// An AWS region code is lowercase alphanumerics and hyphens (`us-east-1`,
/// `ap-southeast-4`, `cn-north-1`). The check matters because the region becomes an
/// ARN field: without it a caller's credential scope could put an ARN delimiter
/// (`:` or `/`) into an ARN this service mints and keys its state by.
fn is_region_code(region: &str) -> bool {
    !region.is_empty()
        && region.len() <= 32
        && region
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// The region the caller signed this request for, defaulting to [`DEFAULT_REGION`]
/// when the request carries no SigV4 credential scope — or one whose region is not a
/// region code.
///
/// Reading the scope rather than a fixed constant is what makes an ARN this service
/// mints match the endpoint the client addressed, so a client-constructed ARN
/// resolves against a stored one.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .filter(|region| is_region_code(region))
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// Mint the id of a new resource: a hyphenated UUID, which is the shape the real
/// service's connection and host ids carry (`connection/<uuid>`, `host/<uuid>` in the
/// ARN).
///
/// The low 48 bits carry the store's monotonic `sequence`, so an id is unique BY
/// CONSTRUCTION — never by probability — and is never handed out twice however many
/// connections one process mints; the remaining bits come from the process's
/// OS-seeded [`RandomState`], so one id does not reveal the next. The version and
/// variant nibbles are set per RFC 4122 (version 4, variant `10xx`), so a client
/// that parses the id as a UUID gets a well-formed one. Built from `std` alone: the
/// uniqueness this needs comes from state, not from a random-number source, so the
/// crate carries no UUID dependency for it.
///
/// One counter feeds every resource kind ([`ComAmazonawsCodestarconnectionsCodestarConnections20191201State::take_sequence`]),
/// so no two resources of this service — of the same kind or not — ever share an id.
pub fn mint_resource_uuid(sequence: u64) -> String {
    let mut bytes = [0u8; 16];
    // Two independently seeded hashes of the sequence: `RandomState::new` takes fresh
    // keys from the process's seed, so these differ from call to call.
    bytes[..8].copy_from_slice(&RandomState::new().hash_one(sequence).to_be_bytes());
    bytes[8..].copy_from_slice(&RandomState::new().hash_one(!sequence).to_be_bytes());
    // The sequence itself occupies the last 6 bytes, which is what makes the id
    // unique rather than merely unlikely to repeat.
    bytes[10..].copy_from_slice(&sequence.to_be_bytes()[2..]);
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;

    let mut hex = String::with_capacity(32);
    for byte in bytes {
        write!(hex, "{byte:02x}").expect("writing hex into a String cannot fail");
    }
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

/// The ARN of a connection: `arn:aws:codestar-connections:<region>:<account>:connection/<id>`.
pub fn connection_arn(region: &str, connection_id: &str) -> String {
    ::engine::emulator_arn(
        ARN_NAMESPACE,
        region,
        &format!("connection/{connection_id}"),
    )
}

/// The ARN of a host: `arn:aws:codestar-connections:<region>:<account>:host/<id>` — the
/// modeled `HostArn` form `^arn:aws(-[\w]+)*:codestar-connections:.+:[0-9]{12}:host\/.+$`.
pub fn host_arn(region: &str, host_id: &str) -> String {
    ::engine::emulator_arn(ARN_NAMESPACE, region, &format!("host/{host_id}"))
}

/// How this service reports a `HostArn` that resolves to no host it holds.
///
/// Shared by every operation that resolves one (`DeleteHost`, `GetHost`), so an absent
/// host reads the same whichever call found it absent. The message is built from the
/// REQUEST — it names the ARN that was supplied — which is the part of it that is a
/// contract.
pub fn host_not_found_message(host_arn: &str) -> String {
    format!("No host exists with the ARN: {host_arn}")
}

/// Encode a pagination cursor: the creation sequence the next page starts at.
///
/// Opaque to the caller (fixed-width hex), as `@paginated` tokens are — a client
/// must only hand it back unmodified.
pub fn encode_next_token(sequence: u64) -> String {
    format!("{sequence:016x}")
}

/// Decode a cursor produced by [`encode_next_token`]; `None` when it was not.
pub fn decode_next_token(token: &str) -> Option<u64> {
    if token.len() != 16 || !token.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    u64::from_str_radix(token, 16).ok()
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsCodestarconnectionsCodestarConnections20191201State>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsCodestarconnectionsCodestarConnections20191201State>)
}
