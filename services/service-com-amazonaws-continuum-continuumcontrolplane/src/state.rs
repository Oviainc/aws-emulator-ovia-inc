// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-continuum-continuumcontrolplane service.
//!
//! One resource is persisted here so far: the **environment** (`/environments`),
//! the parent of every code-finding, code-issue, code-scan and collector
//! subresource this service addresses. Its record and the projections the
//! environment operations answer with live in [`environments`], so create, get,
//! update, list and delete cannot drift from each other.
//!
//! The helpers every handler shares — the region a request was signed for, the
//! wire form of a stored instant, and the opaque pagination-token codec every
//! `@paginated` listing of this service hands back — live here rather than in
//! any one `ops/` file.

mod environments;

pub use environments::{
    environment_arn, environment_not_found, CodeConfig, Environment, EnvironmentConfig,
    DEFAULT_PATCH_CREATION_POLICY, DEFAULT_SCAN_FREQUENCY, ENVIRONMENT_STATUS_ACTIVE,
    MAX_ENVIRONMENTS,
};

use std::collections::BTreeMap;
use std::time::SystemTime;

use service_common::page_tokens::PageTokenKey;

/// This service's ARN namespace (`awsService.arnNamespace`, also its sigv4
/// signing name), which `EnvironmentArn`'s pattern fixes as the third segment of
/// every ARN this service mints.
pub const ARN_SERVICE: &str = "continuum";

/// The region an ARN carries when the request's credential scope names none.
pub const DEFAULT_REGION: &str = "us-east-1";

/// What a handler says when the shared store cannot be reached at all. The
/// wording is the model's own for `InternalServerException`.
pub const STORE_UNAVAILABLE: &str = "The request couldn't be processed because of an internal \
                                     server issue. Try again later. If the problem persists, \
                                     contact AWS Support.";

/// `maxResults`' modelled ceiling, shared by every `@paginated` listing of this
/// service (`smithy_query operation=ListEnvironments`: range 1..100). It is also
/// the page size a listing that names no `maxResults` uses.
pub const MAX_RESULTS_CEILING: i32 = 100;

/// Per-service state for com-amazonaws-continuum-continuumcontrolplane. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsContinuumContinuumcontrolplaneState {
    /// Every live environment, keyed by its `environmentId`. A `BTreeMap` so
    /// `ListEnvironments` reports one stable order and a `nextToken` resumes
    /// after a key rather than into a snapshot of the store.
    pub environments: BTreeMap<String, Environment>,
    /// The monotonic counter minted ids derive from, so two creates in one
    /// process cannot produce the same `environmentId`.
    pub next_environment_seq: u64,
    /// The secret THIS store's pagination tokens are authenticated under; see
    /// [`encode_page_token`].
    pub page_token_key: PageTokenKey,
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsContinuumContinuumcontrolplaneState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsContinuumContinuumcontrolplaneState>)
}

/// The region this request was signed for, defaulting to [`DEFAULT_REGION`].
///
/// Every `*Arn` pattern in this model admits only `[a-z0-9-]+` in the region
/// segment, so a credential scope outside that grammar is not stamped into an
/// ARN.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .filter(|region| {
            !region.is_empty()
                && region
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        })
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// A stored instant as the wire timestamp every `Timestamp` member of this model
/// carries (`@timestampFormat("date-time")`).
pub fn timestamp(at: SystemTime) -> ::aws_smithy_types::DateTime {
    let since_epoch = at
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    // Seconds and sub-second nanos separately, so the instant the store holds
    // survives the conversion exactly rather than through a float.
    ::aws_smithy_types::DateTime::from_secs_and_nanos(
        i64::try_from(since_epoch.as_secs()).unwrap_or(i64::MAX),
        since_epoch.subsec_nanos(),
    )
}

/// Render the store key a page ended on as the opaque `nextToken` a listing
/// hands back: the cursor behind the authenticator THIS store holds the secret
/// for, so no caller can name a position this service never offered.
///
/// Hex keeps the token inside `NextToken`'s modelled length (1..2048) for every
/// key the store can hold, and the rendering lives here rather than in one
/// `ops/` file so two listings cannot issue tokens the other cannot read.
pub fn encode_page_token(token_key: &PageTokenKey, cursor: &str) -> String {
    ::hex::encode(token_key.mint(cursor))
}

/// The store key a `nextToken` carries, or `None` for any token THIS store did
/// not mint — one that is not hex, one too short to carry an authenticator, one
/// whose authenticator does not match the cursor it travels with, and one whose
/// cursor is not UTF-8 — which a listing refuses with its own modelled
/// `ValidationException`.
///
/// A token naming a key the store no longer holds is still a token this store
/// minted, and resumes after that key: an environment deleted between two pages
/// must not break the page after it.
pub fn decode_page_token(token_key: &PageTokenKey, token: &str) -> Option<String> {
    token_key.cursor_of(&::hex::decode(token).ok()?)
}

/// How many entries one page of a listing holds: the caller's `maxResults`, or
/// [`MAX_RESULTS_CEILING`] when the caller names none, clamped into the
/// modelled range.
///
/// The boundary walk already refuses a `maxResults` outside the modelled range
/// before a handler runs, so the clamp is defensive: it keeps the page size a
/// bound on the work one request can ask for whatever reaches it.
pub fn page_size(requested: Option<i32>) -> usize {
    requested.unwrap_or(MAX_RESULTS_CEILING).clamp(1, MAX_RESULTS_CEILING) as usize
}
