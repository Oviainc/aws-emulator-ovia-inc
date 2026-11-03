// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-sso-swbportalservice service.
//!
//! The IAM Identity Center portal has no create API of its own: a sign-in session is
//! minted elsewhere (`CreateToken` in the OIDC service) and the account/role
//! assignments are made by an administrator. So everything the four portal operations
//! report is held here and seeded through [`ComAmazonawsSsoSwbportalserviceState::put_session`]
//! / [`ComAmazonawsSsoSwbportalserviceState::put_account`]; no handler ever synthesizes
//! an account, a role or a session.
//!
//! This module also owns the rules every handler shares, so none is written twice:
//! resolving a presented access token to a usable session
//! ([`ComAmazonawsSsoSwbportalserviceState::session`]), wording the refusal when it
//! resolves to nothing ([`unauthorized_session`], and [`unauthorized_response`] for the
//! gate in [`crate::service`] that runs before any handler), and cutting a sorted
//! listing into a page with an opaque cursor ([`page_of`]).

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// One IAM Identity Center sign-in session, keyed in [`ComAmazonawsSsoSwbportalserviceState::sessions`]
/// by the access token `CreateToken` issued for it.
///
/// `accounts` is the set of account ids the session's user is assigned to, held as a
/// `BTreeSet` so a listing walks it in a deterministic (account-id) order and a page
/// cursor over that order stays stable between calls.
#[derive(Debug, Clone)]
pub struct Session {
    /// Who the session signed in as. Reported by no operation; it is what the
    /// assignments belong to.
    pub subject: String,
    /// The account ids this session may see, ascending.
    pub accounts: BTreeSet<String>,
    /// Cleared by `Logout`, which invalidates the server-side sign-in session.
    pub active: bool,
    /// When the sign-in session stops being usable, whether or not anyone logs out.
    pub expires_at: SystemTime,
}

impl Session {
    /// Whether a request presenting this session's token is served at `now`: a
    /// logged-out or expired session is not, and every operation answers
    /// `UnauthorizedException` for it.
    pub fn is_usable_at(&self, now: SystemTime) -> bool {
        self.active && self.expires_at > now
    }
}

/// The message the portal's authentication gate rejects a presented bearer token with.
///
/// The gate is one gate: it answers the same `UnauthorizedException` whatever operation
/// was addressed, whatever the token looked like and whatever else the request carried,
/// so the text is a constant of the service and never derived from the request. The
/// Smithy model types `UnauthorizedException$message` as an optional `ErrorDescription`
/// without stating a text; this is the one the Portal API is recorded answering, on all
/// four bound operations and every class of unusable token.
pub const INVALID_SESSION_MESSAGE: &str = "Session token not found or invalid";

/// The portal's one authentication refusal, for a presented access token that names no
/// usable sign-in session (unknown, logged out or expired alike).
///
/// Every handler returns THIS value — wrapped in its own operation's error enum — rather
/// than a message of its own, so the gate cannot drift per operation and nothing about
/// the request or the emulator's wiring leaks into it.
pub fn unauthorized_session() -> crate::ops::types::UnauthorizedException {
    crate::ops::types::UnauthorizedException {
        message: Some(INVALID_SESSION_MESSAGE.to_string()),
    }
}

/// The SAME refusal as [`unauthorized_session`], already rendered as a wire response in
/// this service's own protocol envelope.
///
/// A request that presents no bearer credential at all never reaches a handler: the REST
/// router selects a route only when every `@required` http-bound member is on the
/// request, so the absent credential ends the request before any handler's gate could
/// run. `crate::service` answers that case through here, so the authentication refusal a
/// caller sees is the same one whether the credential was absent or merely named no
/// usable session.
///
/// The status and the `__type` come from the model, not from a literal here:
/// `UnauthorizedException` carries its own `@httpError` and is a common error of all four
/// bound operations, so one modeled value states the refusal for every route and
/// [`smithy_server_runtime::OperationError`] reads both off that shape.
///
/// `None` only if the protocol cannot serialize the error, which it cannot fail to do for
/// a shape of this service's own model; the caller then leaves the framework's answer in
/// place rather than inventing one.
pub fn unauthorized_response(
    protocol: &dyn smithy_server_runtime::ServerHttpProtocol,
) -> Option<aws_smithy_runtime_api::http::Response> {
    use smithy_server_runtime::OperationError;

    let refusal = crate::ops::list_accounts::ListAccountsErrorModeled::UnauthorizedException(
        unauthorized_session(),
    );
    protocol
        .serialize_error(
            refusal.as_serializable(),
            refusal.variant_schema(),
            refusal.shape_name(),
            refusal.http_status(),
        )
        .ok()
}

/// An AWS account the portal can report, with the roles assigned in it.
///
/// `roles` is a `BTreeSet`: role names are unique within an account and a listing of
/// them must be deterministically ordered for its page cursor to resume correctly.
#[derive(Debug, Clone)]
pub struct Account {
    pub account_id: String,
    pub account_name: String,
    pub email_address: String,
    /// The role names assigned in this account, ascending.
    pub roles: BTreeSet<String>,
}

/// A role credential `GetRoleCredentials` minted, stored so the same session asking
/// again for the same role sees the same credential until it expires.
///
/// The id and both secrets come from the emulator's credential store, so the
/// credential this service hands back is one the emulator itself will accept.
#[derive(Debug, Clone)]
pub struct RoleCredentialRecord {
    pub access_key_id: String,
    pub secret_access_key: String,
    pub session_token: String,
    /// Epoch milliseconds, the unit `RoleCredentials$expiration` is modeled in.
    pub expiration_millis: i64,
}

impl RoleCredentialRecord {
    /// Whether this credential is still usable at `now`.
    pub fn is_valid_at(&self, now: SystemTime) -> bool {
        self.expiration_millis > epoch_millis(now)
    }
}

/// How an issued role credential is keyed: the access token that asked for it, the
/// account, and the role. A second session asking for the same role gets its own
/// credential, exactly as a second sign-in would.
pub type IssuedCredentialKey = (String, String, String);

/// How long a minted role credential lasts.
///
/// IAM Identity Center returns the credentials of an IAM role session whose length is
/// "the duration configured in the permission set" (the `Logout` model documentation
/// says so in as many words), and the IAM Identity Center User Guide's "Set session
/// duration" page gives that permission-set default as 1 hour. Nothing in this API
/// lets a caller ask for another duration, so the default is the duration used.
pub const ROLE_SESSION_DURATION: Duration = Duration::from_secs(60 * 60);

/// The largest page any listing returns, which is also the largest `maxResults` the
/// model's `MaxResultType` range (1..100) admits — so it doubles as the page size for
/// a request that asks for no particular one.
pub const MAX_PAGE_SIZE: usize = 100;

/// Where an unrepresentable session expiry saturates: far enough out that no request
/// sees it as expired, and inside the range every platform's clock type can hold.
const CENTURY: Duration = Duration::from_secs(100 * 365 * 24 * 60 * 60);

/// [`CENTURY`] past the epoch, or the epoch itself on a platform that cannot hold it.
fn furthest_representable_expiry() -> SystemTime {
    UNIX_EPOCH.checked_add(CENTURY).unwrap_or(UNIX_EPOCH)
}

/// Per-service state for com-amazonaws-sso-swbportalservice. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsSsoSwbportalserviceState {
    /// Sign-in sessions by access token.
    pub sessions: HashMap<String, Session>,
    /// Accounts by account id, ascending (a `BTreeMap`, so listings and their cursors
    /// are deterministic).
    pub accounts: BTreeMap<String, Account>,
    /// Role credentials already minted, by [`IssuedCredentialKey`].
    pub issued_credentials: HashMap<IssuedCredentialKey, RoleCredentialRecord>,
}

impl ComAmazonawsSsoSwbportalserviceState {
    /// Seed (or replace) an account and the roles assigned in it.
    ///
    /// The portal models no create API, so this is how the assignments an
    /// administrator would have made get into the emulator: a test, or whatever stands
    /// an emulator up, calls it through [`service_state`]. The listing operations only
    /// ever read what this stored.
    pub fn put_account(
        &mut self,
        account_id: impl Into<String>,
        account_name: impl Into<String>,
        email_address: impl Into<String>,
        roles: impl IntoIterator<Item = impl Into<String>>,
    ) {
        let account_id = account_id.into();
        let account = Account {
            account_id: account_id.clone(),
            account_name: account_name.into(),
            email_address: email_address.into(),
            roles: roles.into_iter().map(Into::into).collect(),
        };
        self.accounts.insert(account_id, account);
    }

    /// Seed (or replace) an active sign-in session for `access_token`, assigned to
    /// `accounts` and usable for `ttl` from now.
    ///
    /// `ttl` is the caller's: the sign-in session length is a directory setting in IAM
    /// Identity Center, not something this API fixes.
    pub fn put_session(
        &mut self,
        access_token: impl Into<String>,
        subject: impl Into<String>,
        accounts: impl IntoIterator<Item = impl Into<String>>,
        ttl: Duration,
    ) {
        let session = Session {
            subject: subject.into(),
            accounts: accounts.into_iter().map(Into::into).collect(),
            active: true,
            // `SystemTime + Duration` PANICS on a ttl the platform's clock type cannot
            // represent, and `ttl` is the seeding caller's value, so saturate instead:
            // an unrepresentable expiry is "not expired" for every request anyway.
            expires_at: SystemTime::now()
                .checked_add(ttl)
                .unwrap_or_else(furthest_representable_expiry),
        };
        self.sessions.insert(access_token.into(), session);
    }

    /// The session `access_token` names, when it is one this service still serves at
    /// `now`. `None` covers all three unauthorized cases alike — unknown token,
    /// logged-out session, expired session — because the API distinguishes none of
    /// them. Word the refusal with [`unauthorized_session`], never with a message of
    /// the operation's own.
    pub fn session(&self, access_token: &str, now: SystemTime) -> Option<&Session> {
        let resolved = self
            .sessions
            .get(access_token)
            .filter(|session| session.is_usable_at(now));
        if resolved.is_none() {
            // The access attempt real IAM Identity Center would record. The token is
            // `@sensitive` and the subject is personal data, so neither is recorded:
            // the fact of the refusal is what an assertion needs.
            tracing::debug!(
                target: "sso.portal",
                "a presented access token named no usable sign-in session"
            );
        }
        resolved
    }

    /// Invalidate the sign-in session `access_token` names, as `Logout` does.
    /// Answers whether a usable session was invalidated; `false` is the unauthorized
    /// case.
    ///
    /// The record is kept (deactivated) rather than dropped: the credentials already
    /// minted for it are keyed by this token and, per the `Logout` documentation, they
    /// go on working for the rest of their own duration.
    pub fn invalidate_session(&mut self, access_token: &str, now: SystemTime) -> bool {
        match self.sessions.get_mut(access_token) {
            Some(session) if session.is_usable_at(now) => {
                session.active = false;
                // The sign-in session ended: the one state change a caller cannot see
                // directly. Recorded without the token or the subject.
                tracing::info!(target: "sso.portal", "a sign-in session was invalidated by Logout");
                true
            }
            _ => false,
        }
    }

    /// Who the role credential `access_key_id` speaks for: the subject of the sign-in
    /// session this portal issued that credential to, while the credential itself is
    /// still usable at `now`.
    ///
    /// This is the IAM Identity Center identity a request signed with that credential
    /// carries, which is what a service requiring a federated caller resolves a request
    /// through. `None` for every credential this portal did not issue (a long-term IAM
    /// access key, a session another service minted) and for one whose own duration has
    /// run out: neither carries an Identity Center identity.
    ///
    /// A credential whose sign-in session `Logout` ENDED still speaks for the same
    /// subject until its own expiration, which is why the session's usability is not
    /// required here: the credentials already minted for a session go on working for
    /// the rest of their own duration (see [`Self::invalidate_session`]), so the
    /// identity they carry cannot stop existing earlier than they do.
    pub fn signed_in_subject(&self, access_key_id: &str, now: SystemTime) -> Option<&str> {
        self.issued_credentials
            .iter()
            .find(|(_, record)| record.access_key_id == access_key_id && record.is_valid_at(now))
            .and_then(|((access_token, _, _), _)| self.sessions.get(access_token))
            .map(|session| session.subject.as_str())
    }

    /// The accounts `session` is assigned to, ascending by account id, skipping an
    /// assignment whose account this service holds no record of (there is nothing to
    /// report for it).
    pub fn visible_accounts(&self, session: &Session) -> Vec<&Account> {
        session
            .accounts
            .iter()
            .filter_map(|account_id| self.accounts.get(account_id))
            .collect()
    }

    /// The account `account_id` names, when `session` is assigned to it. `None` is the
    /// `ResourceNotFoundException` case: an account the presented session cannot see
    /// does not exist as far as that session is concerned.
    pub fn visible_account(&self, session: &Session, account_id: &str) -> Option<&Account> {
        session
            .accounts
            .contains(account_id)
            .then(|| self.accounts.get(account_id))
            .flatten()
    }
}

/// `at` as epoch milliseconds, the unit `RoleCredentials$expiration` is modeled in.
pub fn epoch_millis(at: SystemTime) -> i64 {
    let since_epoch = at.duration_since(UNIX_EPOCH).unwrap_or_default();
    i64::try_from(since_epoch.as_millis()).unwrap_or(i64::MAX)
}

/// One page of a listing: the items to report, and the cursor to continue from when
/// the listing was cut short.
#[derive(Debug, Clone)]
pub struct Page<T> {
    pub items: Vec<T>,
    /// `None` on the last page — a client stops when the token stops coming.
    pub next_token: Option<String>,
}

/// A presented `nextToken` that cannot be a cursor of the listing it was presented to:
/// not a cursor at all, or one scoped to a different listing or a different session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidCursor;

/// The version stamped into a cursor, so a token minted by an older encoding is
/// refused rather than silently misread.
const CURSOR_VERSION: &str = "v1";

/// The scope a cursor is bound to: the listing it walks, and the session that asked
/// for it. A cursor presented to another listing or by another session is refused.
///
/// The session is identified by a digest of its access token, never the token itself:
/// the token is `@sensitive` and a cursor travels back out in a response and into a
/// client's next URL, so the token must not be recoverable from it.
pub fn cursor_scope(listing: &str, access_token: &str) -> String {
    let digest = <sha2::Sha256 as sha2::Digest>::digest(access_token.as_bytes());
    format!("{listing}:{}", hex::encode(&digest[..16]))
}

/// An opaque continuation cursor for `scope`, resuming after `last_key`.
///
/// Hex rather than a bespoke encoding because the token only has to be opaque and
/// round-trip exactly; `hex` is already a workspace dependency. It is NOT
/// authenticated: a caller who reproduces the encoding can resume this listing at a
/// key of its choosing, which reaches nothing the same session's listing would not
/// already report (the items are built from that session's own assignments), and
/// authenticating it would need either a server secret or per-request state written by
/// a reporting operation.
pub fn encode_cursor(scope: &str, last_key: &str) -> String {
    hex::encode(format!("{CURSOR_VERSION}:{scope}:{last_key}"))
}

/// The key a cursor resumes after, when it is one [`encode_cursor`] produced for the
/// same `scope`. A cursor from another listing (a different account's roles, or another
/// session's accounts) is refused rather than applied to this one.
///
/// Admitted ONLY when the token is byte for byte the one [`encode_cursor`] mints for the
/// key it carries: the acceptance set is the mint's output set and nothing wider.
/// `hex::decode` reads spellings `hex::encode` never writes (upper- and mixed-case
/// digits), and such a spelling is a token no response of this service ever carried, so
/// re-minting the decoded key and comparing is what holds the two to exact inverses —
/// whatever the encoding is, and without a second grammar maintained beside it.
pub fn decode_cursor(scope: &str, token: &str) -> Result<String, InvalidCursor> {
    let bytes = hex::decode(token).map_err(|_| InvalidCursor)?;
    let decoded = String::from_utf8(bytes).map_err(|_| InvalidCursor)?;
    let last_key = decoded
        .strip_prefix(&format!("{CURSOR_VERSION}:{scope}:"))
        .ok_or(InvalidCursor)?;
    if encode_cursor(scope, last_key) != token {
        return Err(InvalidCursor);
    }
    Ok(last_key.to_string())
}

/// Cut `sorted` — a listing of `(sort key, item)` pairs in ascending key order — into
/// the page a request asked for.
///
/// `next_token` resumes strictly after the key it carries, so a continuation never
/// repeats or skips an item; `max_results` bounds the page, defaulting to
/// [`MAX_PAGE_SIZE`] when the caller names none; and the token comes back only when
/// items were left over, so the last page carries none.
///
/// A `max_results` OUTSIDE the modeled `MaxResultType` range (1..100) never reaches
/// here from the wire: the boundary validator refuses it before the handler runs, and
/// this service words that refusal as `InvalidRequestException` (the 400 rows in
/// `tests/all/portal_session.rs` pin it). The clamp below is therefore only a guard for
/// a direct, non-HTTP caller, not a silent reshaping of a request a client sent: a bound
/// that caller NAMED is carried into the modeled range — floored at 1 below it, capped at
/// [`MAX_PAGE_SIZE`] above it — and never folded into the "named none" case, so only the
/// ABSENCE of a bound takes the default page.
pub fn page_of<T>(
    sorted: impl IntoIterator<Item = (String, T)>,
    scope: &str,
    next_token: Option<&str>,
    max_results: Option<i32>,
) -> Result<Page<T>, InvalidCursor> {
    let after = match next_token {
        Some(token) => Some(decode_cursor(scope, token)?),
        None => None,
    };
    let limit = match max_results {
        // Clamped while the value is still SIGNED: `usize::try_from` fails for every
        // negative bound, so converting first would discard the sign and leave a named
        // bound indistinguishable from `None` — the default page, the far end of the
        // range from the floor a too-small bound belongs at.
        Some(requested) => usize::try_from(requested.max(1))
            .unwrap_or(MAX_PAGE_SIZE)
            .min(MAX_PAGE_SIZE),
        None => MAX_PAGE_SIZE,
    };

    let mut items = Vec::new();
    let mut last_key = String::new();
    let mut truncated = false;
    for (key, item) in sorted {
        if after.as_deref().is_some_and(|after| key.as_str() <= after) {
            continue;
        }
        if items.len() >= limit {
            truncated = true;
            break;
        }
        last_key = key;
        items.push(item);
    }

    Ok(Page {
        items,
        next_token: truncated.then(|| encode_cursor(scope, &last_key)),
    })
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsSsoSwbportalserviceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsSsoSwbportalserviceState>)
}
