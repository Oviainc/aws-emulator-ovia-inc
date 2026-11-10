// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-accountaccessruntime-awsaccountaccessruntime service.
//!
//! The runtime API models no create or delete operation: a tenant's role entitlements
//! are made by an administrator (through the Account Access control plane), and the two
//! runtime operations only READ them. So the entitlements live here and arrive through
//! [`ComAmazonawsAccountaccessruntimeAwsaccountaccessruntimeState::put_role`] — whatever
//! stands an emulator up, or a test, seeds them through [`service_state`]; no handler
//! ever invents a tenant or a role. A freshly built store already holds the one tenant
//! this emulator serves ([`bootstrap_tenant_id`]) so a booted emulator has something to
//! list, and every other tenant is unknown until it is seeded, which is the empty
//! listing rather than an error.
//!
//! This module also owns the rules both handlers share, so neither is written twice:
//! resolving a tenant's entitlements
//! ([`ComAmazonawsAccountaccessruntimeAwsaccountaccessruntimeState::roles`] and
//! [`ComAmazonawsAccountaccessruntimeAwsaccountaccessruntimeState::entitlement`]),
//! stamping an expiry without risking an overflow panic ([`expiry_after`]), ending the
//! credential sets the clock has made spent ([`end_expired_sessions`]), cutting a sorted
//! listing into a page with an opaque cursor ([`page_of`]), and wording the refusal of a
//! numeric member whose modelled type carries no range ([`below_minimum`]).

use std::collections::BTreeMap;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// How long a credential set lasts when the request names no `sessionDuration`.
///
/// `GetRoleCredentialsRequest$sessionDuration` documents only that "the service uses the
/// default session duration" in that case. What the credentials are is an IAM role
/// session, and an IAM role session's default length is one hour — the default
/// `AssumeRole` documents for `DurationSeconds` ("the default value of 3600 seconds") —
/// so that is the window used here. It is still capped by the role's own maximum below,
/// which the same member says the value "can't exceed".
pub const DEFAULT_SESSION_DURATION: Duration = Duration::from_secs(60 * 60);

/// The longest session a seeded entitlement admits unless the seeding caller names
/// another: the "maximum session duration configured for the role" that
/// `GetRoleCredentialsRequest$sessionDuration` is measured against. IAM creates a role
/// with a one-hour maximum session duration, so an entitlement starts there.
pub const DEFAULT_MAX_SESSION_DURATION: Duration = Duration::from_secs(60 * 60);

/// The most roles one `ListRoles` page carries, and the page size a request that names
/// no `maxResults` gets. `maxResults` is modelled as a plain `Integer` with no range and
/// the API publishes no page size, so this bound is the emulator's own — it is what
/// keeps one response from being unbounded in the size of the store.
pub const MAX_PAGE_SIZE: usize = 100;

/// The tenant a freshly built store holds.
///
/// Derived, never a literal: this emulator serves exactly one account
/// (`engine::emulator_account_id`), and `TenantId` is documented as beginning with `aa-`
/// (3 to 24 characters of lowercase letters, numbers and hyphens), so the tenant of that
/// account is that account id under the documented prefix.
pub fn bootstrap_tenant_id() -> String {
    format!("aa-{}", ::engine::emulator_account_id())
}

/// One role a tenant is entitled to assume: an `AccessDetails` of `ListRoles`, and the
/// `GetRoleCredentials` lookup key.
#[derive(Debug, Clone)]
pub struct RoleEntitlement {
    /// The 12-digit account the role lives in -> `AccessDetails$accountId`.
    pub account_id: String,
    /// `AccessDetails$accountName`, which is documented as absent when the account name
    /// cannot be resolved — so it is optional here too.
    pub account_name: Option<String>,
    /// `AccessDetails$roleArn`, unique within the tenant.
    pub role_arn: String,
    /// The role's own maximum session duration: the limit a requested `sessionDuration`
    /// cannot exceed.
    pub max_session_duration: Duration,
}

impl RoleEntitlement {
    /// An entitlement to `role_arn` in `account_id`, with no resolvable account name and
    /// the default maximum session duration.
    pub fn new(account_id: impl Into<String>, role_arn: impl Into<String>) -> Self {
        Self {
            account_id: account_id.into(),
            account_name: None,
            role_arn: role_arn.into(),
            max_session_duration: DEFAULT_MAX_SESSION_DURATION,
        }
    }

    /// The same entitlement, with the account's name resolvable.
    #[must_use]
    pub fn with_account_name(mut self, account_name: impl Into<String>) -> Self {
        self.account_name = Some(account_name.into());
        self
    }

    /// The same entitlement, with the role configured to allow sessions up to
    /// `max_session_duration`.
    #[must_use]
    pub fn with_max_session_duration(mut self, max_session_duration: Duration) -> Self {
        self.max_session_duration = max_session_duration;
        self
    }

    /// The window a request asking for no particular `sessionDuration` gets: the
    /// service default, never longer than this role allows.
    pub fn default_session_duration(&self) -> Duration {
        DEFAULT_SESSION_DURATION.min(self.max_session_duration)
    }
}

/// A tenant's entitlements, by `roleArn`.
///
/// A `BTreeMap` for two reasons: a role ARN is unique within a tenant and is what
/// `GetRoleCredentials` looks a role up by, and a listing of them must walk a
/// deterministic order for a page cursor over that order to resume correctly between
/// calls.
#[derive(Debug, Clone, Default)]
pub struct Tenant {
    roles: BTreeMap<String, RoleEntitlement>,
}

/// A credential set `GetRoleCredentials` minted, as stored.
///
/// The id and both secrets come from the emulator's credential store, so the credentials
/// this service hands back are ones the emulator itself accepts for signing, and nothing
/// here chooses them.
#[derive(Clone)]
pub struct IssuedSession {
    pub tenant_id: String,
    pub role_arn: String,
    /// The `sessionName` the request asked for, when it named one.
    pub session_name: Option<String>,
    pub access_key_id: String,
    pub secret_access_key: String,
    pub session_token: String,
    /// -> `ProgrammaticAccessDetails$expiresAt`.
    pub expires_at: SystemTime,
}

impl IssuedSession {
    /// Whether this credential set is still inside its own duration at `now`.
    pub fn is_valid_at(&self, now: SystemTime) -> bool {
        self.expires_at > now
    }
}

/// Redacts the two secrets rather than deriving `Debug`, so a record of this state
/// cannot carry a credential into a log: `ProgrammaticAccessDetails` is `@sensitive`, and
/// the access key id is kept because it is what a log line has to name a session by.
impl std::fmt::Debug for IssuedSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IssuedSession")
            .field("tenant_id", &self.tenant_id)
            .field("role_arn", &self.role_arn)
            .field("session_name", &self.session_name)
            .field("access_key_id", &self.access_key_id)
            .field("secret_access_key", &"<redacted>")
            .field("session_token", &"<redacted>")
            .field("expires_at", &self.expires_at)
            .finish()
    }
}

/// Per-service state for com-amazonaws-accountaccessruntime-awsaccountaccessruntime. A handler reaches it through [`service_state`].
#[derive(Debug)]
pub struct ComAmazonawsAccountaccessruntimeAwsaccountaccessruntimeState {
    /// Entitlements by tenant id, ascending, so a listing and its cursor are
    /// deterministic.
    tenants: BTreeMap<String, Tenant>,
    /// Every credential set `GetRoleCredentials` minted and the clock has not yet made
    /// spent, oldest first. Its growth is bounded by its records' own expiry: every call
    /// ends the records the clock has made due (see [`end_expired_sessions`]), so a
    /// long-running emulator holds only the sessions still inside their duration.
    sessions: Vec<IssuedSession>,
}

/// A store already holding the tenant this emulator serves, so a booted emulator can
/// answer `ListRoles` and `GetRoleCredentials` before anything seeds it. Both roles are
/// built from the account this emulator presents, not written out: one whose account name
/// resolves and one whose does not, since `AccessDetails$accountName` is optional.
impl Default for ComAmazonawsAccountaccessruntimeAwsaccountaccessruntimeState {
    fn default() -> Self {
        let mut state = Self {
            tenants: BTreeMap::new(),
            sessions: Vec::new(),
        };
        let account = ::engine::emulator_account_id();
        let tenant = bootstrap_tenant_id();
        state.put_role(
            &tenant,
            RoleEntitlement::new(
                account,
                ::engine::emulator_arn("iam", "", "role/AdministratorAccess"),
            )
            .with_account_name("emulator"),
        );
        state.put_role(
            &tenant,
            RoleEntitlement::new(
                account,
                ::engine::emulator_arn("iam", "", "role/ReadOnlyAccess"),
            ),
        );
        state
    }
}

impl ComAmazonawsAccountaccessruntimeAwsaccountaccessruntimeState {
    /// Seed (or replace) one role `tenant_id` is entitled to assume.
    ///
    /// The runtime API models no create operation, so this is how an administrator's
    /// entitlements get into the emulator. Both handlers only ever read what this stored.
    pub fn put_role(&mut self, tenant_id: impl Into<String>, role: RoleEntitlement) {
        self.tenants
            .entry(tenant_id.into())
            .or_default()
            .roles
            .insert(role.role_arn.clone(), role);
    }

    /// Forget the role `role_arn` of `tenant_id`, as revoking the entitlement would.
    /// Answers whether one was held.
    pub fn remove_role(&mut self, tenant_id: &str, role_arn: &str) -> bool {
        self.tenants
            .get_mut(tenant_id)
            .is_some_and(|tenant| tenant.roles.remove(role_arn).is_some())
    }

    /// The roles `tenant_id` is entitled to assume, ascending by role ARN — the order
    /// `ListRoles` reports them in and the order its cursor resumes through. A tenant
    /// this service holds no entitlements for has none, which is the empty listing.
    pub fn roles(&self, tenant_id: &str) -> Vec<&RoleEntitlement> {
        self.tenants
            .get(tenant_id)
            .map(|tenant| tenant.roles.values().collect())
            .unwrap_or_default()
    }

    /// The entitlement of `tenant_id` to `role_arn`. `None` is the `AccessDeniedException`
    /// case: the tenant is not entitled to that role, whether because the role is not one
    /// of its entitlements or because the tenant has none at all.
    pub fn entitlement(&self, tenant_id: &str, role_arn: &str) -> Option<&RoleEntitlement> {
        self.tenants
            .get(tenant_id)
            .and_then(|tenant| tenant.roles.get(role_arn))
    }

    /// Record a credential set `GetRoleCredentials` minted. It is what the response
    /// reports, read back out of here rather than from the local value.
    pub fn record_session(&mut self, session: IssuedSession) {
        self.sessions.push(session);
    }

    /// The credential set `access_key_id` names, held for `tenant_id`, while it is still
    /// inside its duration at `now`.
    ///
    /// The tenant is part of the key, not a detail of the record: one store holds every
    /// tenant's minted credential sets, so a lookup that matched the id alone would
    /// resolve another tenant's session and its secrets.
    pub fn session(
        &self,
        tenant_id: &str,
        access_key_id: &str,
        now: SystemTime,
    ) -> Option<&IssuedSession> {
        self.sessions.iter().find(|session| {
            session.tenant_id == tenant_id
                && session.access_key_id == access_key_id
                && session.is_valid_at(now)
        })
    }

    /// Every credential set this service has minted and not yet ended, oldest first.
    pub fn sessions(&self) -> &[IssuedSession] {
        &self.sessions
    }

    /// Forget every credential set whose advertised `expiresAt` has passed at `now`, and
    /// answer the access key id of each so the caller can end it in the emulator's
    /// credential store too (which is what [`end_expired_sessions`] does).
    ///
    /// The expiry is the instant the CLOCK makes a credential spent, so ending it is owed
    /// then — whether or not anybody asks for that role again. Nothing about the request
    /// that happens to materialise it enters into which records end: every record the
    /// clock has made due does.
    pub fn expire_sessions(&mut self, now: SystemTime) -> Vec<String> {
        let mut ended = Vec::new();
        self.sessions.retain(|session| {
            let valid = session.is_valid_at(now);
            if !valid {
                ended.push(session.access_key_id.clone());
            }
            valid
        });
        ended
    }
}

/// End every minted credential set the clock has made spent at `now`: drop its record
/// here and forget the credential it names in the emulator's credential store, so the
/// emulator stops accepting it for signing at exactly the `expiresAt` the response that
/// minted it advertised.
///
/// Both handlers call this before they serve, because the expiry is due at its own
/// instant and not at the next call about the same role. It materialises only what the
/// clock already owed, so no response changes because it ran, and a credential still
/// inside its duration is untouched.
///
/// A store that no longer holds the credential is already in the state this asks for, so
/// the delete's error is nothing to report.
pub fn end_expired_sessions(
    state: &mut ComAmazonawsAccountaccessruntimeAwsaccountaccessruntimeState,
    credential_store: &::emulator_server::CredentialStore,
    now: SystemTime,
) {
    for access_key_id in state.expire_sessions(now) {
        let _ = credential_store.delete(&access_key_id);
        ::tracing::debug!(
            target: "accountaccessruntime",
            access_key_id = %access_key_id,
            "a minted role credential reached its expiration and was ended"
        );
    }
}

/// Where an unrepresentable expiry saturates: far enough out that no request sees it as
/// expired, and inside the range every platform's clock type can hold.
const CENTURY: Duration = Duration::from_secs(100 * 365 * 24 * 60 * 60);

/// [`CENTURY`] past the epoch, or the epoch itself on a platform that cannot hold it.
fn furthest_representable_expiry() -> SystemTime {
    UNIX_EPOCH.checked_add(CENTURY).unwrap_or(UNIX_EPOCH)
}

/// `at` advanced by `duration` — the ONE way this service stamps an expiry.
///
/// `SystemTime + Duration` PANICS on a sum the platform's clock type cannot represent,
/// so every expiry is computed here instead. An unrepresentable sum saturates at the
/// furthest instant the clock can hold, which is "not expired" for every request anyway,
/// and never earlier than `at` itself.
pub fn expiry_after(at: SystemTime, duration: Duration) -> SystemTime {
    at.checked_add(duration)
        .unwrap_or_else(|| furthest_representable_expiry().max(at))
}

/// How a numeric member whose modelled type carries no range is refused below its
/// smallest meaningful value, in the wording a validation failure carries: the value, the
/// member it was given for, and the bound it has to satisfy.
///
/// `maxResults` and `sessionDuration` are both plain `Integer`s in this model, so the
/// framework's boundary validation passes any value and the rule is the handler's to
/// state.
pub fn below_minimum(member: &str, value: i32, minimum: i32) -> String {
    format!(
        "1 validation error detected: Value '{value}' at '{member}' failed to satisfy \
         constraint: Member must have value greater than or equal to {minimum}"
    )
}

/// One page of a listing: the items to report, and the cursor to continue from when the
/// listing was cut short.
#[derive(Debug, Clone)]
pub struct Page<T> {
    pub items: Vec<T>,
    /// `None` on the last page — a client stops when the token stops coming.
    pub next_token: Option<String>,
}

/// A presented `nextToken` that cannot be a cursor of the listing it was presented to:
/// not a cursor at all, or one scoped to another tenant's role listing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidCursor;

/// The version stamped into a cursor, so a token minted by an older encoding is refused
/// rather than silently misread.
const CURSOR_VERSION: &str = "v1";

/// The scope a role-listing cursor is bound to. A cursor presented to another tenant's
/// listing is refused rather than applied to it.
pub fn roles_cursor_scope(tenant_id: &str) -> String {
    format!("roles:{tenant_id}")
}

/// An opaque continuation cursor for `scope`, resuming after `last_key`.
///
/// Hex rather than a bespoke encoding because the token only has to be opaque and
/// round-trip exactly. The token encodes a POSITION, not a secret and not a capability:
/// it names the role the previous page stopped on, which the caller already received, and
/// it is only ever read for the tenant the request names.
pub fn encode_cursor(scope: &str, last_key: &str) -> String {
    ::hex::encode(format!("{CURSOR_VERSION}:{scope}:{last_key}"))
}

/// The key a cursor resumes after, when it is one [`encode_cursor`] produced for the same
/// `scope`. A cursor from another tenant's listing is refused rather than applied here.
///
/// Admitted ONLY when the token is byte for byte the one [`encode_cursor`] mints for the
/// key it carries: the acceptance set is the mint's output set and nothing wider.
/// `hex::decode` reads spellings `hex::encode` never writes (upper- and mixed-case
/// digits), and such a spelling is a token no response of this service ever carried, so
/// re-minting the decoded key and comparing is what holds the two to exact inverses —
/// whatever the encoding is, and without a second grammar maintained beside it.
pub fn decode_cursor(scope: &str, token: &str) -> Result<String, InvalidCursor> {
    let bytes = ::hex::decode(token).map_err(|_| InvalidCursor)?;
    let decoded = String::from_utf8(bytes).map_err(|_| InvalidCursor)?;
    let last_key = decoded
        .strip_prefix(&format!("{CURSOR_VERSION}:{scope}:"))
        .ok_or(InvalidCursor)?;
    if encode_cursor(scope, last_key) != token {
        return Err(InvalidCursor);
    }
    Ok(last_key.to_string())
}

/// Cut `sorted` — a listing of `(sort key, item)` pairs in ascending key order — into the
/// page a request asked for.
///
/// `next_token` resumes strictly after the key it carries, so a continuation never
/// repeats or skips an item; `max_results` bounds the page, defaulting to
/// [`MAX_PAGE_SIZE`] when the caller names none and capped there when the caller asks for
/// more, so one response is never unbounded in the size of the store; and the token comes
/// back only when items were left over, so the last page carries none.
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
        // range from the floor a too-small bound belongs at. A bound below one does not
        // reach here from the wire at all: `ListRoles` refuses it.
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
) -> Option<::engine::ServiceState<ComAmazonawsAccountaccessruntimeAwsaccountaccessruntimeState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsAccountaccessruntimeAwsaccountaccessruntimeState>)
}
