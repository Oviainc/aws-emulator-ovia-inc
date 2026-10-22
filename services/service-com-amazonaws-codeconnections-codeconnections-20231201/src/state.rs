// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-codeconnections-codeconnections-20231201 service.

use std::collections::BTreeMap;
use std::time::{SystemTime, UNIX_EPOCH};

use sha2::{Digest, Sha256};

use crate::ops::types::{Connection, Tag};

/// The region an ARN carries when the request is unsigned, so no SigV4 credential
/// scope names one. `emulator_arn`'s own documentation gives `us-east-1` as the
/// emulator default.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The status a connection minted through the API, CLI or an SDK carries.
///
/// From the `Connection` shape's own documentation: "A connection created through
/// CloudFormation, the CLI, or the SDK is in `PENDING` status by default. You can
/// make its status `AVAILABLE` by updating the connection in the console." The
/// handshake that moves it is a console action, not something the clock owes, so a
/// connection this service mints stays `PENDING` until something updates it.
pub const CONNECTION_STATUS_ON_CREATE: &str = "PENDING";

/// The largest page `ListConnections` renders, whatever the caller asked for.
///
/// This is the published maximum of the modeled `MaxResults` member (`range 0..100`,
/// from `smithy_query(query="operation", operation="ListConnections")`), applied
/// server-side: a caller that asks for more, or that omits `MaxResults` entirely (the
/// member is modeled `default: 0`), still gets at most this many connections in one
/// response and a `NextToken` for the rest. Without the clamp a one-line request would
/// render the whole store in a single response.
pub const MAX_PAGE_SIZE: usize = 100;

/// The largest number of connections this emulator stores for the account.
///
/// A CHOSEN bound, not a quota read off any AWS document: the store lives in the
/// emulator's process, so the number of records a caller can accumulate has to be
/// capped somewhere. `CreateConnection` refuses past it with the modeled
/// `LimitExceededException` (429, "Exceeded the maximum limit for connections."),
/// which is the error the operation already declares for exactly this.
pub const MAX_CONNECTIONS: usize = 1000;

/// A connection as this service persists it.
///
/// One record per `ConnectionArn`; every member of the modeled `Connection` shape
/// that the API can observe is stored here, so `GetConnection` and
/// `ListConnections` both render from the same row rather than recomputing one.
#[derive(Debug, Clone)]
pub struct StoredConnection {
    /// Unique within the account — see [`ComAmazonawsCodeconnectionsCodeconnections20231201State::connection_name_taken`].
    pub connection_name: String,
    pub connection_arn: String,
    /// `None` when the create call named no provider: the member is optional.
    pub provider_type: Option<String>,
    /// The 12-digit account that owns the connection.
    pub owner_account_id: String,
    /// One of the modeled `ConnectionStatus` values (`PENDING`|`AVAILABLE`|`ERROR`).
    pub connection_status: String,
    /// The host the connection is associated with, when it has one.
    pub host_arn: Option<String>,
    /// The tags supplied at create time, in the order they were supplied.
    pub tags: Vec<Tag>,
}

impl StoredConnection {
    /// Render the stored row as the modeled `Connection` structure.
    ///
    /// The single projection from state onto the wire shape, so every reader
    /// (`GetConnection`, `ListConnections`) reports the same connection the same way.
    pub fn to_modeled(&self) -> Connection {
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

/// Per-service state for com-amazonaws-codeconnections-codeconnections-20231201. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsCodeconnectionsCodeconnections20231201State {
    /// Every connection this account holds, keyed by its `ConnectionArn`.
    ///
    /// A `BTreeMap`, so `ListConnections` walks a total, stable order and a page
    /// token can name a position in it.
    pub connections: BTreeMap<String, StoredConnection>,
    /// How many connections this store has ever minted: the sequence number behind
    /// the next connection's resource id (see [`connection_resource_id`]).
    ///
    /// It counts mints rather than live connections, so it never rewinds when one is
    /// deleted — which is what keeps a deleted connection's ARN from coming back.
    pub connections_minted: u64,
}

impl ComAmazonawsCodeconnectionsCodeconnections20231201State {
    /// Whether a connection with this name already exists in the account.
    ///
    /// The `Connection.ConnectionName` member documents "Connection names must be
    /// unique in an Amazon Web Services account", so this is the uniqueness check
    /// `CreateConnection` applies. It scans `connections` rather than keeping a
    /// second name index: one map is one source of truth, and a secondary index
    /// can only ever disagree with it.
    pub fn connection_name_taken(&self, name: &str) -> bool {
        self.connections
            .values()
            .any(|connection| connection.connection_name == name)
    }
}

/// A fresh resource id for a connection: 32 hex characters grouped the way a
/// connection id is grouped (8-4-4-4-12), to go after `connection/` in its ARN.
///
/// UNIQUENESS comes from `sequence`, a counter this store never rewinds
/// ([`ComAmazonawsCodeconnectionsCodeconnections20231201State::connections_minted`]),
/// so the ARN built from it is never reused — the `ConnectionArn` member documents
/// "The ARN is never reused if the connection is deleted" — even after the deleted
/// connection's name is freed and taken again. The digest is what makes the id OPAQUE,
/// the way this project's other services derive their opaque labels (see
/// `service-com-amazonaws-mediapackagev2-mediapackagev2`'s `state::egress_domain`), so
/// the id carries neither the account's name nor a count of its connections; mixing in
/// the minting instant keeps two emulator runs from minting the same first id.
pub fn connection_resource_id(account_id: &str, region: &str, sequence: u64) -> String {
    let minted_at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let mut hasher = Sha256::new();
    for field in [
        account_id.as_bytes(),
        region.as_bytes(),
        &sequence.to_le_bytes(),
        &minted_at.to_le_bytes(),
    ] {
        // Length-prefixed, so two different field lists cannot hash alike.
        hasher.update((field.len() as u64).to_le_bytes());
        hasher.update(field);
    }
    let digest = hex::encode(hasher.finalize());
    format!(
        "{}-{}-{}-{}-{}",
        &digest[0..8],
        &digest[8..12],
        &digest[12..16],
        &digest[16..20],
        &digest[20..32]
    )
}

/// Encode a `ListConnections` page token naming the last connection a page returned.
///
/// The token is the hex of that `ConnectionArn`: opaque to a caller, and a position
/// in the `connections` key order, so resuming skips exactly the keys at or before it
/// however the map changed in between. The modeled `NextToken` is 1..1024 characters
/// matching `^.*$`; an ARN is at most 256 characters, so its hex is at most 512.
pub fn encode_page_token(last_returned_arn: &str) -> String {
    hex::encode(last_returned_arn.as_bytes())
}

/// Decode a token [`encode_page_token`] produced, or `None` when it is not one.
///
/// A token only names a position if it decodes to an ARN of the kind this service
/// mints, so anything else — a hand-written string, or a hex blob that happens to
/// decode to text — is rejected rather than silently read as a position.
pub fn decode_page_token(token: &str) -> Option<String> {
    let decoded = String::from_utf8(hex::decode(token).ok()?).ok()?;
    is_connection_arn(&decoded).then_some(decoded)
}

/// The partitions the modeled `ConnectionArn` pattern `^arn:aws(-[\w]+)*:…` admits,
/// as AWS publishes them: the commercial partition and the two isolated ones.
const ARN_PARTITIONS: &[&str] = &["aws", "aws-cn", "aws-us-gov"];

/// The longest region token this service will put in an ARN. AWS region codes are
/// far shorter; the cap only keeps a caller-supplied scope from growing an ARN past
/// the modeled `ConnectionArn` length of 256.
const MAX_REGION_LEN: usize = 64;

/// Whether `candidate` is shaped like an AWS region code: non-empty lowercase ASCII,
/// digits and hyphens (`us-east-1`, `eu-west-1`, `cn-north-1`), within
/// [`MAX_REGION_LEN`].
///
/// This is the ONE rule both the ARN minter ([`request_region`]) and the page-token
/// reader ([`is_connection_arn`]) apply, so this service can never refuse a token it
/// issued itself.
fn is_region_token(candidate: &str) -> bool {
    !candidate.is_empty()
        && candidate.len() <= MAX_REGION_LEN
        && candidate
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

/// Whether `candidate` has the shape of a connection ARN this service mints:
/// `arn:<partition>:codeconnections:<region>:<12 digits>:connection/<id>`.
fn is_connection_arn(candidate: &str) -> bool {
    let fields: Vec<&str> = candidate.splitn(6, ':').collect();
    fields.len() == 6
        && fields[0] == "arn"
        && ARN_PARTITIONS.contains(&fields[1])
        && fields[2] == "codeconnections"
        && is_region_token(fields[3])
        && fields[4].len() == 12
        && fields[4].bytes().all(|byte| byte.is_ascii_digit())
        && fields[5].starts_with("connection/")
        && fields[5].len() > "connection/".len()
}

/// The region this request was signed for, or [`DEFAULT_REGION`] when it is unsigned
/// or the scope does not carry a region code.
///
/// The SigV4 credential scope is the only place a request states its region, so an
/// ARN this service mints is regional in exactly the region the caller addressed —
/// but the scope is caller-supplied text, and the value is load-bearing (it becomes
/// a field of every minted ARN, and the ARN is the pagination position). Only a value
/// shaped like a region code ([`is_region_token`]) is used; anything else falls back
/// to the default rather than being spliced into an ARN.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .filter(|region| is_region_token(region))
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsCodeconnectionsCodeconnections20231201State>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsCodeconnectionsCodeconnections20231201State>)
}
