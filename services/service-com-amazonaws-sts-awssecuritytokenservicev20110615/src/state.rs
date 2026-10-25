// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-sts-awssecuritytokenservicev20110615 service.
//!
//! STS is a credential vendor: every operation either mints a session (`AssumeRole`,
//! `GetSessionToken`, `GetFederationToken`) or reports on one (`GetCallerIdentity`,
//! `GetAccessKeyInfo`). This module holds the registries those operations share —
//! the sessions keyed by the access key id each was minted under, plus the reading of
//! the role records the IAM store in the same engine holds — and the pure rules the
//! handlers apply to them, so a handler file contains only its own operation's decisions.
//!
//! Beside the sessions it keeps the encoded authorization messages the account has
//! issued: STS is also the service that decodes the opaque blob an authorization
//! failure hands a caller (`DecodeAuthorizationMessage`), and the authorization
//! detail behind such a blob is privileged, so the blob carries only the token that
//! names the record here.
//!
//! Secrets are deliberately NOT stored here. The access key id, secret access key
//! and session token are minted by the emulator's credential store
//! ([`mint_session`]), which keeps the secret material; this registry keeps only the
//! public facts a later call reports (who the credential speaks for, when it
//! expires, what was packed into it).

use std::collections::HashMap;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use emulator_server::CredentialStore;
use sha2::{Digest, Sha256};

use crate::ops::types::{Credentials, Tag};

/// `AssumeRole`'s default session length: "By default, the value is set to `3600`
/// seconds." (`AssumeRoleRequest$DurationSeconds`, this service's model).
pub const ASSUME_ROLE_DEFAULT_DURATION_SECONDS: i32 = 3600;

/// `GetSessionToken`'s and `GetFederationToken`'s default session length:
/// "Acceptable durations ... range from 900 seconds (15 minutes) to 129,600 seconds
/// (36 hours), with 43,200 seconds (12 hours) as the default."
/// (`GetSessionTokenRequest$DurationSeconds` / `GetFederationTokenRequest$DurationSeconds`,
/// this service's model).
pub const VENDED_SESSION_DEFAULT_DURATION_SECONDS: i32 = 43_200;

/// The budget the packed-policy percentage is measured against: "The plaintext that
/// you use for both inline and managed session policies can't exceed 2,048
/// characters." (`AssumeRole`'s documentation, this service's model). AWS reports
/// `PackedPolicySize` as "a percentage value that indicates the packed size of the
/// session policies and session tags combined passed in the request", so the
/// percentage here is of this documented plaintext budget.
pub const PACKED_POLICY_PLAINTEXT_BUDGET: usize = 2048;

/// The session-token size STS will not exceed, from the model's
/// `minimumSessionTokenSizeType` range (`0..4096`).
pub const SESSION_TOKEN_SIZE_LIMIT: usize = 4096;

/// The name of the ambient IAM user: the identity a request speaks for when its
/// access key id names no session this registry minted (the long-term key the
/// emulator publishes at start, or an unsigned request).
pub const AMBIENT_USER_NAME: &str = "emulator";

/// The most managed session policies one mint may name: "You can provide up to 10
/// managed policy ARNs" (`AssumeRoleRequest$PolicyArns`, this service's model). The
/// model declares no length on `policyDescriptorListType`, so the ceiling is enforced
/// here.
pub const MAX_MANAGED_SESSION_POLICIES: usize = 10;

/// A role's session length when IAM holds no `MaxSessionDuration` for it: "The maximum
/// session duration setting can have a value from 1 hour to 12 hours"
/// (`AssumeRoleRequest$DurationSeconds`), and IAM's own default is the low end of that
/// range, which is also this operation's default duration.
pub const ROLE_DEFAULT_MAX_SESSION_DURATION_SECONDS: i32 = ASSUME_ROLE_DEFAULT_DURATION_SECONDS;

/// The ceiling on a ROLE-CHAINED session: "Role chaining limits your Amazon Web
/// Services CLI or Amazon Web Services API role session to a maximum of one hour"
/// (`AssumeRoleRequest$DurationSeconds`, this service's model).
pub const ROLE_CHAINING_MAX_DURATION_SECONDS: i32 = 3600;

/// The IAM action a role's trust policy must allow for a principal to assume it.
pub const ASSUME_ROLE_ACTION: &str = "sts:AssumeRole";

/// The IAM action a role's trust policy must allow for a mint to carry session tags.
pub const TAG_SESSION_ACTION: &str = "sts:TagSession";

/// The IAM action a role's trust policy must allow for a mint to set a source identity.
pub const SET_SOURCE_IDENTITY_ACTION: &str = "sts:SetSourceIdentity";

/// The condition key a trust policy tests a request's `ExternalId` through.
pub const EXTERNAL_ID_CONDITION_KEY: &str = "sts:ExternalId";

/// The `Principal` spelling that names a whole account: the account's root user.
fn account_root_arn(account: &str) -> String {
    format!("arn:aws:iam::{account}:root")
}

/// The alphabet AWS unique ids (`AIDA…`, `AROA…`) are drawn from: uppercase
/// letters and digits. Matches the alphabet the emulator's own key ids use.
const UNIQUE_ID_ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";

/// An AWS unique id is a four-character type prefix plus 17 more characters, 21 in
/// total (`AROAJ4XQZ7X5YQ2R3TZ4A`).
const UNIQUE_ID_SUFFIX_LEN: usize = 17;

/// Who a credential speaks for, as `GetCallerIdentity` reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Identity {
    pub user_id: String,
    pub account: String,
    pub arn: String,
}

/// Which operation minted a session. Kept because the three kinds are not
/// interchangeable: only an `AssumeRole` session carries a role and a source
/// identity, and only a `GetFederationToken` session carries a federated user.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionKind {
    AssumedRole,
    SessionToken,
    FederationToken,
}

/// One minted session, keyed in [`ComAmazonawsStsAwssecuritytokenservicev20110615State::sessions`]
/// by the access key id it was minted under.
#[derive(Debug, Clone)]
pub struct Session {
    /// The registry key: the access key id the credential store minted.
    pub access_key_id: String,
    pub created_at: SystemTime,
    /// `created_at + duration_seconds`: the wall-clock instant the credential stops
    /// being valid.
    pub expires_at: SystemTime,
    pub duration_seconds: i32,
    /// What `GetCallerIdentity` answers for a request signed with this credential.
    pub identity: Identity,
    pub kind: SessionKind,
    pub source_identity: Option<String>,
    pub tags: Vec<(String, String)>,
    pub transitive_tag_keys: Vec<String>,
    pub packed_policy_size: Option<i32>,
    pub external_id: Option<String>,
    /// The ARN of the role an `AssumeRole` session was minted for, exactly as IAM holds
    /// it — the spelling a trust policy names when this session chains into another
    /// role. `None` for every other kind of session.
    pub role_arn: Option<String>,
    /// Set once the credential has been deleted from the credential store because
    /// the session expired, so the deletion happens exactly once.
    pub revoked: bool,
}

impl Session {
    /// Whether the clock has passed this session's expiry.
    pub fn is_expired(&self, now: SystemTime) -> bool {
        now >= self.expires_at
    }
}

/// What a caller asks for when it mints a session: everything about the session
/// that does not come from the credential store.
#[derive(Debug, Clone)]
pub struct NewSession {
    pub duration_seconds: i32,
    pub identity: Identity,
    pub kind: SessionKind,
    pub source_identity: Option<String>,
    pub tags: Vec<(String, String)>,
    pub transitive_tag_keys: Vec<String>,
    pub packed_policy_size: Option<i32>,
    pub external_id: Option<String>,
    /// See [`Session::role_arn`].
    pub role_arn: Option<String>,
}

impl NewSession {
    /// A session of `kind` for `identity`, lasting `duration_seconds`, carrying no
    /// policy, tags or source identity. Fill the rest with struct-update syntax.
    pub fn new(identity: Identity, kind: SessionKind, duration_seconds: i32) -> Self {
        Self {
            duration_seconds,
            identity,
            kind,
            source_identity: None,
            tags: Vec::new(),
            transitive_tag_keys: Vec::new(),
            packed_policy_size: None,
            external_id: None,
            role_arn: None,
        }
    }
}

/// A minted session: the credential to hand back to the caller, and the record this
/// registry keeps of it.
#[derive(Debug, Clone)]
pub struct MintedSession {
    pub credentials: Credentials,
    pub session: Session,
}

/// The session credential a request presented has expired. Every operation refuses
/// such a request rather than answering it as some other identity.
#[derive(Debug, Clone)]
pub struct ExpiredSession {
    pub access_key_id: String,
}

impl ExpiredSession {
    /// The wording AWS uses for an expired session credential.
    pub fn message(&self) -> String {
        "The security token included in the request is expired".to_string()
    }
}

/// The most sessions this registry holds at once.
///
/// A mint is a request-driven insert, so the registry needs a declared ceiling: at
/// the cap a mint makes room by giving up sessions the clock has already expired,
/// and is refused outright rather than dropping a LIVE session — a credential this
/// service handed out keeps working for as long as it was promised. The cap also
/// bounds the per-request expiry sweep, which is therefore O(cap), not O(calls made
/// so far). The value is a chosen emulator ceiling, far above any test or demo
/// workload; AWS publishes no such limit.
pub const MAX_SESSIONS: usize = 4096;

/// How long an expired session's record is kept after its expiry.
///
/// Keeping it briefly is what lets a caller still presenting the credential be told
/// the token EXPIRED instead of being quietly answered as some other identity; past
/// this window the record is pruned, so an idle emulator's registry shrinks back to
/// its live sessions. A chosen retention window, not a duration AWS documents.
pub const EXPIRED_SESSION_RETENTION: Duration = Duration::from_secs(3600);

/// Why a session could not be minted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MintError {
    /// The registry is full of sessions that have not expired yet.
    AtCapacity,
    /// The credential store minted a credential without its secrets, which the kind
    /// asked for always carries.
    NoSecrets,
}

impl MintError {
    /// Why the request was refused, for the caller to act on.
    pub fn message(&self) -> String {
        match self {
            Self::AtCapacity => format!(
                "this emulator holds at most {MAX_SESSIONS} sessions that have not expired yet; \
                 let some expire before minting another"
            ),
            Self::NoSecrets => {
                "the credential store minted a session without its secrets; no session was created"
                    .to_string()
            }
        }
    }
}

/// Per-service state for com-amazonaws-sts-awssecuritytokenservicev20110615. A handler reaches it through [`service_state`].
///
/// One store, bounded by [`MAX_SESSIONS`]: the sessions this service minted. There is
/// deliberately no map for role ids — a role's id belongs to the role, so it is read
/// from the IAM store in the same engine ([`resolve_role`]), the service that holds the
/// role; remembering one here would add a request-driven store without making anything
/// more stable.
#[derive(Debug, Default)]
pub struct ComAmazonawsStsAwssecuritytokenservicev20110615State {
    /// Every session this service minted and has not pruned, keyed by its access key
    /// id. At most [`MAX_SESSIONS`] entries; each entry's tags and transitive tag keys
    /// are bounded by [`MAX_SESSION_TAGS`] — the model's own `tagListType` /
    /// `tagKeyListType` maximum, which the mint path checks over the EFFECTIVE set (a
    /// chained mint's own tags plus the ones it carries forward), so a role chain
    /// cannot grow one record past it — and each key and value by
    /// [`MAX_TAG_KEY_CHARS`] / [`MAX_TAG_VALUE_CHARS`].
    pub sessions: HashMap<String, Session>,
    /// Every encoded authorization message the account has issued and not yet let
    /// expire, keyed by the token the blob carries. At most
    /// [`MAX_AUTHORIZATION_MESSAGES`] entries.
    pub authorization_messages: HashMap<String, AuthorizationMessage>,
    /// How many encoded authorization messages this account has ever issued. It only
    /// ever counts up — unlike the registry's size, which shrinks as messages expire
    /// or give way at the cap — which is what makes every token distinct.
    pub authorization_messages_issued: u64,
}

/// Shorthand for this service's state, for use inside the crate.
pub type StsState = ComAmazonawsStsAwssecuritytokenservicev20110615State;

impl StsState {
    /// The session an access key id names, expired or not.
    pub fn session(&self, access_key_id: &str) -> Option<&Session> {
        self.sessions.get(access_key_id)
    }

    /// Record a minted session under its access key id.
    ///
    /// Refuses once the registry holds [`MAX_SESSIONS`], so NO path can push it past
    /// the cap; [`mint_session`] calls [`StsState::make_room`] first, which is what
    /// normally keeps that from happening.
    pub fn insert(&mut self, session: Session) -> Result<(), MintError> {
        let replacing = self.sessions.contains_key(&session.access_key_id);
        if !replacing && self.sessions.len() >= MAX_SESSIONS {
            return Err(MintError::AtCapacity);
        }
        self.sessions.insert(session.access_key_id.clone(), session);
        Ok(())
    }

    /// Make room for one more session: materialise due expiries (which prunes the
    /// records whose retention window has run out), then give up expired sessions,
    /// earliest expiry first, until the registry is under the cap. A session that has
    /// NOT expired is never dropped to make room; when only such sessions remain the
    /// mint is refused.
    pub fn make_room(&mut self, now: SystemTime, store: &CredentialStore) -> Result<(), MintError> {
        self.revoke_expired(now, store);
        while self.sessions.len() >= MAX_SESSIONS {
            let giveable = self
                .sessions
                .values()
                .filter(|session| session.is_expired(now))
                .min_by_key(|session| session.expires_at)
                .map(|session| session.access_key_id.clone());
            match giveable {
                Some(access_key_id) => self.discard(&access_key_id, store),
                None => return Err(MintError::AtCapacity),
            }
        }
        Ok(())
    }

    /// Drop a session and the credential it was minted with — for a mint this
    /// service decided, after minting, that it cannot honour.
    pub fn discard(&mut self, access_key_id: &str, store: &CredentialStore) {
        self.sessions.remove(access_key_id);
        let _ = store.delete(access_key_id);
    }

    /// Materialise every expiry the clock has already made due: the credential of an
    /// expired session is deleted from the credential store, so a request signed
    /// with it no longer verifies, and a record whose [`EXPIRED_SESSION_RETENTION`]
    /// window has run out is dropped altogether. Both are owed at the same instant
    /// whether or not anybody called, which is why a reporting operation may do
    /// them, and the pruning is what keeps the registry's size tied to the sessions
    /// still in play rather than to every session ever minted.
    ///
    /// Inside the retention window the session RECORD is kept, so a caller still
    /// presenting the credential is told the token expired instead of being quietly
    /// answered as the ambient identity.
    pub fn revoke_expired(&mut self, now: SystemTime, store: &CredentialStore) {
        for session in self.sessions.values_mut() {
            if !session.revoked && session.is_expired(now) {
                let _ = store.delete(&session.access_key_id);
                session.revoked = true;
            }
        }
        self.sessions
            .retain(|_, session| now < session.expires_at + EXPIRED_SESSION_RETENTION);
    }
}

// ---------------------------------------------------------------------------
// Encoded authorization messages
// ---------------------------------------------------------------------------

/// The envelope every encoded authorization message this account issues starts with,
/// before it is base64-encoded. It is what separates a blob this service minted from
/// any other run of bytes a caller may hand `DecodeAuthorizationMessage`, and it
/// carries a version so a later envelope can be told from this one.
const ENCODED_MESSAGE_ENVELOPE: &[u8] = b"STSEncodedAuthorizationMessage/1:";

/// How long an encoded authorization message stays decodable.
///
/// A CHOSEN window, not a duration AWS documents — the model says only that the
/// message may have "expired". It is long enough for a caller that was handed a blob
/// by an authorization failure to decode it, and short enough that the registry
/// shrinks back on an idle emulator.
pub const ENCODED_MESSAGE_VALIDITY: Duration = Duration::from_secs(3600);

/// The most encoded authorization messages this registry holds at once.
///
/// Issuing one is a request-driven insert, so the registry needs a declared ceiling.
/// At the cap the oldest record gives way: unlike a credential, an encoded message is
/// a diagnostic the caller may decode once, not a promise this service made for a
/// stated duration. A chosen emulator ceiling; AWS publishes no such limit.
pub const MAX_AUTHORIZATION_MESSAGES: usize = 1024;

/// How many digest bytes name one record: 16, rendered as 32 hex characters — far
/// more than enough to keep the [`MAX_AUTHORIZATION_MESSAGES`] records held at once
/// apart.
const AUTHORIZATION_MESSAGE_TOKEN_BYTES: usize = 16;

/// What an authorization failure decided, as the decoded message reports it: whether
/// the request was allowed, whether a `Deny` statement said so explicitly, and the
/// context of the request it was decided for.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AuthorizationDecision {
    pub allowed: bool,
    /// Whether the refusal came from an explicit `Deny` rather than from the absence
    /// of an `Allow`.
    pub explicit_deny: bool,
    /// The principal that made the request: its unique id and its ARN.
    pub principal_id: String,
    pub principal_arn: String,
    /// The requested action and resource.
    pub action: String,
    pub resource: String,
    /// The condition keys in the context of the request, each with its values.
    pub conditions: Vec<(String, Vec<String>)>,
    /// The policy statements the decision matched, each by its statement id and its
    /// effect. Empty when the request matched none, which is what an implicit deny
    /// looks like.
    pub matched_statements: Vec<(String, String)>,
}

/// One encoded authorization message the account has issued, keyed in
/// [`StsState::authorization_messages`] by the token its blob carries.
#[derive(Debug, Clone)]
pub struct AuthorizationMessage {
    /// The registry key: the token the encoded blob names this record by.
    pub token: String,
    pub issued_at: SystemTime,
    /// `issued_at + ENCODED_MESSAGE_VALIDITY`: past this instant the message is no
    /// longer decodable.
    pub expires_at: SystemTime,
    /// The document `DecodeAuthorizationMessage` answers with.
    pub decoded_message: String,
}

impl StsState {
    /// The record a token names, expired or not.
    pub fn authorization_message(&self, token: &str) -> Option<&AuthorizationMessage> {
        self.authorization_messages.get(token)
    }

    /// Materialise every encoded-message expiry the clock has already made due: a
    /// message past its validity window cannot be decoded any more, whether or not
    /// anybody called, so its record is dropped. That is what keeps the registry's
    /// size tied to the messages still decodable rather than to every one ever
    /// issued.
    pub fn expire_authorization_messages(&mut self, now: SystemTime) {
        self.authorization_messages
            .retain(|_, message| now < message.expires_at);
    }

    /// Record an issued encoded authorization message, bounded by
    /// [`MAX_AUTHORIZATION_MESSAGES`]: expiries the clock has made due are
    /// materialised first, and at the cap the earliest-issued record gives way, so NO
    /// path can push the registry past the cap.
    pub fn record_authorization_message(&mut self, message: AuthorizationMessage, now: SystemTime) {
        self.expire_authorization_messages(now);
        while self.authorization_messages.len() >= MAX_AUTHORIZATION_MESSAGES {
            let oldest = self
                .authorization_messages
                .values()
                .min_by(|one, other| {
                    (one.issued_at, one.token.as_str()).cmp(&(other.issued_at, other.token.as_str()))
                })
                .map(|held| held.token.clone());
            match oldest {
                Some(token) => {
                    self.authorization_messages.remove(&token);
                }
                None => break,
            }
        }
        self.authorization_messages
            .insert(message.token.clone(), message);
    }
}

/// The decoded message for an authorization decision: the document AWS's
/// `DecodeAuthorizationMessage` answers with, which states whether the request was
/// allowed and whether a deny was explicit, the statements the decision matched, and
/// the context it was decided in — the principal, the action, the resource and the
/// condition keys of the request.
pub fn authorization_decision_document(decision: &AuthorizationDecision) -> String {
    let matched: Vec<serde_json::Value> = decision
        .matched_statements
        .iter()
        .map(|(statement_id, effect)| {
            serde_json::json!({ "statementId": statement_id, "effect": effect })
        })
        .collect();
    let conditions: Vec<serde_json::Value> = decision
        .conditions
        .iter()
        .map(|(key, values)| {
            let values: Vec<serde_json::Value> = values
                .iter()
                .map(|value| serde_json::json!({ "value": value }))
                .collect();
            serde_json::json!({ "key": key, "values": { "items": values } })
        })
        .collect();
    serde_json::json!({
        "allowed": decision.allowed,
        "explicitDeny": decision.explicit_deny,
        "matchedStatements": { "items": matched },
        // Nothing here evaluates a policy it could not read, so there is never a
        // failure to report — the member is still part of the document's shape.
        "failures": { "items": [] },
        "context": {
            "principal": { "id": decision.principal_id, "arn": decision.principal_arn },
            "action": decision.action,
            "resource": decision.resource,
            "conditions": { "items": conditions },
        },
    })
    .to_string()
}

/// Issue an encoded authorization message for an authorization decision: record the
/// decoded document and hand back the opaque blob the denying operation returns to its
/// caller.
///
/// The blob is the envelope and the token that names the record, and nothing
/// else: the authorization detail "can contain privileged information that the user
/// who requested the operation should not see" (this operation's documentation), so it
/// stays here and never travels with the blob. It is rendered in the URL-safe base64
/// alphabet with the padding stripped, the form an encoded authorization message
/// travels in.
pub fn encode_authorization_message(
    state: &mut StsState,
    now: SystemTime,
    decision: &AuthorizationDecision,
) -> String {
    let decoded_message = authorization_decision_document(decision);
    state.authorization_messages_issued = state.authorization_messages_issued.wrapping_add(1);
    let token = authorization_message_token(
        &decoded_message,
        now,
        state.authorization_messages_issued,
    );
    state.record_authorization_message(
        AuthorizationMessage {
            token: token.clone(),
            issued_at: now,
            expires_at: now + ENCODED_MESSAGE_VALIDITY,
            decoded_message,
        },
        now,
    );
    let mut payload = ENCODED_MESSAGE_ENVELOPE.to_vec();
    payload.extend_from_slice(token.as_bytes());
    ::aws_smithy_types::base64::encode(payload)
        .replace('+', "-")
        .replace('/', "_")
        .trim_end_matches('=')
        .to_string()
}

/// The decoded message an encoded authorization message carries, or the wording to
/// refuse it with.
///
/// A message decodes only when it is base64 of this account's envelope AND names a
/// record the registry still holds: bytes that are not base64 at all, base64 of
/// something this account never issued, and a message whose validity window has run
/// out are all messages this service cannot decode — exactly the cases
/// `InvalidAuthorizationMessageException` reports. A read, so nothing recorded here
/// changes: the same message decodes again to the same document for as long as it is
/// valid.
pub fn decode_authorization_message(
    state: &StsState,
    encoded: &str,
    now: SystemTime,
) -> Result<String, String> {
    let Some(bytes) = encoded_message_bytes(encoded) else {
        return Err(invalid_authorization_message(encoded.as_bytes()));
    };
    bytes
        .strip_prefix(ENCODED_MESSAGE_ENVELOPE)
        .and_then(|token| std::str::from_utf8(token).ok())
        .and_then(|token| state.authorization_message(token))
        .filter(|record| now < record.expires_at)
        .map(|record| record.decoded_message.clone())
        .ok_or_else(|| invalid_authorization_message(&bytes))
}

/// The bytes an encoded message carries, or `None` when it is not base64 at all.
///
/// Read in BOTH base64 alphabets — the standard one (`+`, `/`) and the URL-safe one
/// (`-`, `_`), which is the one AWS's own encoded authorization messages travel in —
/// and with the padding optional, because the padding carries no data. Whitespace and
/// anything outside those alphabets — "invalid characters, such as line breaks", as
/// the modelled error puts it — is not base64 here, so such a message carries no
/// bytes at all. The decoding itself is `aws_smithy_types::base64`, the codec the wire
/// serializer already uses for every blob.
fn encoded_message_bytes(encoded: &str) -> Option<Vec<u8>> {
    // Only a message in the URL-safe alphabet needs a copy; one already in the
    // standard alphabet is decoded where it lies.
    let standard: std::borrow::Cow<'_, str> = if encoded.contains(['-', '_']) {
        std::borrow::Cow::Owned(
            encoded
                .chars()
                .map(|character| match character {
                    '-' => '+',
                    '_' => '/',
                    other => other,
                })
                .collect(),
        )
    } else {
        std::borrow::Cow::Borrowed(encoded)
    };
    if let Ok(bytes) = ::aws_smithy_types::base64::decode(standard.as_ref()) {
        return Some(bytes);
    }
    let missing_padding = (4 - standard.len() % 4) % 4;
    if missing_padding == 0 || standard.ends_with('=') {
        return None;
    }
    let mut padded = standard.into_owned();
    for _ in 0..missing_padding {
        padded.push('=');
    }
    ::aws_smithy_types::base64::decode(padded).ok()
}

/// The wording a message this service cannot decode is refused with: the sentence
/// AWS answers, followed by the bytes of the message rendered as signed 8-bit values.
///
/// Those bytes are all the service can report of a blob it did not issue — nothing
/// else about it is known to it — and they are the decoded bytes, which is what the
/// message actually carried. A blob that is not base64 at all has none, so its own
/// bytes are what gets reported. The rendering is bounded by the request: the model
/// caps `EncodedMessage` at 10,240 characters, and the boundary validator refuses a
/// longer one before any handler runs.
fn invalid_authorization_message(bytes: &[u8]) -> String {
    let mut rendered = String::new();
    for (index, byte) in bytes.iter().enumerate() {
        if index > 0 {
            rendered.push_str(", ");
        }
        rendered.push_str(&byte.cast_signed().to_string());
    }
    format!("Message is not valid: [{rendered}]")
}

/// The token that names one recorded decision: hex of a SHA-256 digest over the
/// decoded document, the instant the message was issued and how many messages this
/// account has issued so far — so a token is a function of the message it names, and
/// two messages for the same decision, even at the same instant, get different tokens.
///
/// It is an identifier, not a secret: what it names is a decision this emulator itself
/// recorded, and the caller it was handed to is the one the decision is about.
fn authorization_message_token(document: &str, issued_at: SystemTime, issued: u64) -> String {
    let nanos = issued_at
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_nanos());
    let mut digest = Sha256::new();
    digest.update(document.as_bytes());
    digest.update(nanos.to_be_bytes());
    digest.update(issued.to_be_bytes());
    digest
        .finalize()
        .iter()
        .take(AUTHORIZATION_MESSAGE_TOKEN_BYTES)
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsStsAwssecuritytokenservicev20110615State>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsStsAwssecuritytokenservicev20110615State>)
}

/// The identity a request speaks for, resolved from the access key id it signed
/// with: the identity of the session that key was minted for, or the ambient
/// identity when the key names no session here.
///
/// Expiries the clock has made due are materialised first, so an expired session is
/// refused rather than answered.
pub fn resolve_caller(
    state: &mut StsState,
    access_key_id: &str,
    now: SystemTime,
    store: &CredentialStore,
) -> Result<Identity, ExpiredSession> {
    resolve_request_caller(state, access_key_id, now, store, ambient_identity())
        .map(|caller| caller.identity)
}

/// Who a request speaks for, and everything about its credential a later decision needs:
/// which operation minted it (`None` for a long-term access key), and — for a session —
/// the source identity and session tags it carries forward.
#[derive(Debug, Clone)]
pub struct Caller {
    pub identity: Identity,
    /// Which STS operation minted the credential, or `None` when the key names no
    /// session here: a long-term IAM access key, or the key this emulator publishes.
    pub kind: Option<SessionKind>,
    pub source_identity: Option<String>,
    pub tags: Vec<(String, String)>,
    pub transitive_tag_keys: Vec<String>,
    /// The role ARN of an assumed-role credential; see [`Session::role_arn`].
    pub role_arn: Option<String>,
}

impl Caller {
    /// A caller presenting no session credential: the long-term identity `identity`.
    fn long_term(identity: Identity) -> Self {
        Self {
            identity,
            kind: None,
            source_identity: None,
            tags: Vec::new(),
            transitive_tag_keys: Vec::new(),
            role_arn: None,
        }
    }

    /// Every spelling of this caller a role's trust policy may name in its `Principal`
    /// element: its own ARN, the account's root user (which delegates to the whole
    /// account) and the bare account id AWS accepts in that root's place — plus, for an
    /// assumed-role session, the ARN of the ROLE it is a session of, because that is the
    /// principal a trust policy names to admit a chained assume.
    #[must_use]
    pub fn trust_policy_principals(&self) -> Vec<String> {
        let mut named = vec![
            self.identity.arn.clone(),
            account_root_arn(&self.identity.account),
            self.identity.account.clone(),
        ];
        if let Some(role_arn) = &self.role_arn {
            named.push(role_arn.clone());
        }
        named
    }
}

/// The caller a request's access key id names, with everything its credential carries.
///
/// `long_term` is the identity the key speaks for when it names no session this registry
/// minted — the IAM user that holds the access key ([`long_term_identity`]), or the
/// ambient identity when nothing holds it. Expiries the clock has made due are
/// materialised first, so an expired session is refused rather than answered.
pub fn resolve_request_caller(
    state: &mut StsState,
    access_key_id: &str,
    now: SystemTime,
    store: &CredentialStore,
    long_term: Identity,
) -> Result<Caller, ExpiredSession> {
    state.revoke_expired(now, store);
    match state.session(access_key_id) {
        Some(session) if session.is_expired(now) => Err(ExpiredSession {
            access_key_id: access_key_id.to_string(),
        }),
        Some(session) => Ok(Caller {
            identity: session.identity.clone(),
            kind: Some(session.kind),
            source_identity: session.source_identity.clone(),
            tags: session.tags.clone(),
            transitive_tag_keys: session.transitive_tag_keys.clone(),
            role_arn: session.role_arn.clone(),
        }),
        None => Ok(Caller::long_term(long_term)),
    }
}

/// This service's view of the IAM store in the SAME shared engine: the account's users,
/// their access keys and their roles.
///
/// The IAM service's OWN accessor is called rather than copied, so STS resolves a role
/// and an access key against the very records `iam:CreateRole` and `iam:CreateAccessKey`
/// wrote. `None` means the engine carries no IAM store — a router that mounts STS
/// without IAM, where the account can hold no role and no user at all.
pub type IamState = ::service_com_amazonaws_iam_awsidentitymanagementv20100508::state::ComAmazonawsIamAwsidentitymanagementv20100508State;

/// See [`IamState`].
pub fn iam_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<IamState>> {
    ::service_com_amazonaws_iam_awsidentitymanagementv20100508::state::service_state(engine)
}

/// The identity a LONG-TERM access key speaks for: the IAM user that holds it.
///
/// "Returns details about the IAM user or role whose credentials are used to call the
/// operation" (`GetCallerIdentity`, this service's model), so a request signed with an
/// IAM user's own access key speaks for THAT user — its `UserId` and its ARN as IAM
/// holds them, never a fixed one. A key IAM holds no record of is the ambient identity
/// ([`ambient_identity`]): the long-term key this emulator publishes at start.
///
/// The key's `AccessKey$Status` is deliberately NOT consulted. Whether a credential
/// verifies at all is the emulator's AUTHENTICATION layer's decision, taken from the
/// credential store before any handler runs — this service never checks a signature —
/// and that store, not IAM's record, is where a deactivated key would have to stop
/// verifying. Reading the status here would instead report an Inactive key as some OTHER
/// identity, which is the very substitution this function exists to remove.
pub fn long_term_identity(iam: Option<&IamState>, access_key_id: &str) -> Identity {
    let user = iam.and_then(|iam| {
        let record = iam.access_keys.get(access_key_id)?;
        iam.users.get(&record.user_key)
    });
    match user {
        Some(user) => Identity {
            user_id: user.user_id.clone(),
            account: ::engine::emulator_account_id().to_string(),
            arn: user.arn.clone(),
        },
        None => ambient_identity(),
    }
}

/// A role as IAM holds it, reduced to the facts an `AssumeRole` decision needs.
#[derive(Debug, Clone)]
pub struct ResolvedRole {
    /// `Role$RoleId`, exactly the value `iam:CreateRole`/`iam:GetRole` reported, which
    /// is what `AssumedRoleUser$AssumedRoleId` is built from.
    pub role_id: String,
    /// The role name in the case IAM holds it.
    pub role_name: String,
    /// `Role$Arn`, the resource a trust-policy decision is made about.
    pub arn: String,
    /// The account the role belongs to.
    pub account: String,
    /// `Role$AssumeRolePolicyDocument`: the trust policy, as IAM stored it.
    pub trust_policy: Option<String>,
    /// `Role$MaxSessionDuration`, defaulted to
    /// [`ROLE_DEFAULT_MAX_SESSION_DURATION_SECONDS`] when IAM holds none.
    pub max_session_duration: i32,
}

/// The role an `AssumeRole` request's `RoleArn` names, as IAM holds it.
///
/// `None` when the account holds no such role — which is also the answer for a role ARN
/// in ANOTHER account, because this emulator holds exactly one account's IAM records. The
/// stored role's own ARN must match the one the request named, so a role of the same name
/// under a different path is not resolved either.
pub fn resolve_role(iam: &IamState, role_arn: &str) -> Option<ResolvedRole> {
    let named = parse_role_arn(role_arn)?;
    let key = ::service_com_amazonaws_iam_awsidentitymanagementv20100508::state::name_key(
        &named.name,
    );
    let role = iam.roles.get(&key).filter(|role| role.arn == role_arn)?;
    Some(ResolvedRole {
        role_id: role.role_id.clone(),
        role_name: role.role_name.clone(),
        arn: role.arn.clone(),
        account: named.account,
        trust_policy: role.assume_role_policy_document.clone(),
        max_session_duration: role
            .max_session_duration
            .unwrap_or(ROLE_DEFAULT_MAX_SESSION_DURATION_SECONDS),
    })
}

impl ResolvedRole {
    /// Whether this role's trust policy lets `principals` perform `action` on it, given
    /// the condition keys `context` carries.
    ///
    /// The DECISION is [`::service_common::iam_authorization`], the tree's one evaluation
    /// of IAM policy documents against a request, read as the RESOURCE-based policy it is
    /// — a trust policy is attached to the role, so its statements name the principals
    /// they grant to and a statement naming no resource covers the role itself. A role
    /// whose trust policy this cannot read as a policy, or which carries none at all,
    /// grants nothing: nothing is assumed on evidence the account does not hold.
    #[must_use]
    pub fn trust_policy_allows(
        &self,
        action: &str,
        principals: &[String],
        context: &[(String, String)],
    ) -> bool {
        let Some(document) = self.trust_policy.as_deref() else {
            return false;
        };
        let request = ::service_common::iam_authorization::Request::new(action, &self.arn, principals)
            .with_context(context)
            .with_published_keys(ASSUME_ROLE_PUBLISHED_CONDITION_KEYS);
        ::service_common::iam_authorization::resource_decision(document, &request)
            == ::service_common::iam_authorization::Decision::Allowed
    }
}

/// How AWS words an authorization refusal: the principal, the action and the resource.
///
/// STS does not disclose whether the resource EXISTS, so a role the account does not
/// hold and a role whose trust policy withholds the grant are worded alike.
pub fn not_authorized_message(caller_arn: &str, action: &str, resource: &str) -> String {
    format!("User: {caller_arn} is not authorized to perform: {action} on resource: {resource}")
}

/// Whether a credential of `kind` may call the STS operation `operation`.
///
/// Each kind of temporary credential this service vends is documented with the STS
/// operations it may NOT call, and they differ, so the rule is the credential's class
/// rather than one blanket ban:
///
/// * an `AssumeRole` session "cannot call the Amazon Web Services STS
///   `GetFederationToken` or `GetSessionToken` API operations" (`AssumeRole`);
/// * a `GetSessionToken` session "cannot call any STS API except `AssumeRole` or
///   `GetCallerIdentity`" (`GetSessionToken`);
/// * a `GetFederationToken` session "cannot call any STS operations except
///   `GetCallerIdentity`" (`GetFederationToken`).
///
/// A long-term access key (`None`) may call every one of them: `GetSessionToken` and
/// `GetFederationToken` "must be called by using the long-term ... credentials of an IAM
/// user" (both operations' documentation, this service's model).
#[must_use]
pub fn caller_may_call(kind: Option<SessionKind>, operation: &str) -> bool {
    match kind {
        None => true,
        Some(SessionKind::AssumedRole) => {
            !matches!(operation, "GetFederationToken" | "GetSessionToken")
        }
        Some(SessionKind::SessionToken) => {
            matches!(operation, "AssumeRole" | "GetCallerIdentity")
        }
        Some(SessionKind::FederationToken) => operation == "GetCallerIdentity",
    }
}

/// The condition keys an `AssumeRole` mint MODELS — the keys whose absence from
/// [`assume_role_condition_context`] means the request carried no value for them, rather
/// than this emulator not knowing the key. Only these get IAM's empty-set reading under a
/// `ForAllValues:` / `ForAnyValue:` qualifier; a clause over any other key stays
/// undecided and withholds the grant.
pub const ASSUME_ROLE_PUBLISHED_CONDITION_KEYS: &[&str] = &[
    "sts:RoleSessionName",
    "sts:ExternalId",
    "sts:SourceIdentity",
    "sts:TransitiveTagKeys",
    "aws:PrincipalArn",
    "aws:PrincipalAccount",
    "aws:PrincipalType",
    "aws:userid",
    "aws:TagKeys",
];

/// The condition keys STS publishes about one role-session mint, for a trust policy's
/// `Condition` to be decided against.
///
/// Each is a key AWS documents for an `sts:AssumeRole` request: the `aws:` keys that
/// describe the CALLING principal, the `sts:` keys the operation's own members supply,
/// the `aws:RequestTag/<key>` of each session tag the mint sets, and the multi-valued
/// `aws:TagKeys` / `sts:TransitiveTagKeys` — passed as one pair per value, which is how
/// [`::service_common::iam_authorization`] reads a multi-valued key. Nothing is invented:
/// a key the request does not determine is left out, so a clause over it stays undecided
/// and withholds the grant.
pub fn assume_role_condition_context(
    caller: &Caller,
    role_session_name: &str,
    external_id: Option<&str>,
    source_identity: Option<&str>,
    tags: &[(String, String)],
    transitive_tag_keys: &[String],
) -> Vec<(String, String)> {
    let mut context = vec![
        (
            "sts:RoleSessionName".to_string(),
            role_session_name.to_string(),
        ),
        // For a role session the principal AWS names here is the ROLE, not the session:
        // the session's own spelling is `aws:userid`.
        (
            "aws:PrincipalArn".to_string(),
            caller
                .role_arn
                .clone()
                .unwrap_or_else(|| caller.identity.arn.clone()),
        ),
        (
            "aws:PrincipalAccount".to_string(),
            caller.identity.account.clone(),
        ),
        (
            "aws:PrincipalType".to_string(),
            caller_principal_type(caller.kind).to_string(),
        ),
        ("aws:userid".to_string(), caller.identity.user_id.clone()),
    ];
    if let Some(external_id) = external_id {
        context.push((
            EXTERNAL_ID_CONDITION_KEY.to_string(),
            external_id.to_string(),
        ));
    }
    if let Some(source_identity) = source_identity {
        context.push((
            "sts:SourceIdentity".to_string(),
            source_identity.to_string(),
        ));
    }
    // The tags the CALLING principal already carries, which is what a target role's trust
    // policy tests when it gates a chained assume on the session it is being made from.
    for (key, value) in &caller.tags {
        context.push((format!("aws:PrincipalTag/{key}"), value.clone()));
    }
    for (key, value) in tags {
        context.push((format!("aws:RequestTag/{key}"), value.clone()));
        context.push(("aws:TagKeys".to_string(), key.clone()));
    }
    for key in transitive_tag_keys {
        context.push(("sts:TransitiveTagKeys".to_string(), key.clone()));
    }
    context
}

/// How `aws:PrincipalType` names the calling principal's kind.
fn caller_principal_type(kind: Option<SessionKind>) -> &'static str {
    match kind {
        Some(SessionKind::AssumedRole) => "AssumedRole",
        Some(SessionKind::FederationToken) => "FederatedUser",
        Some(SessionKind::SessionToken) | None => "User",
    }
}

/// The modelled character class a session tag's key and value must satisfy
/// (`tagKeyType` / `tagValueType`, this service's model): letters, separators, numbers
/// and `_.:/=+-@`. The two shapes declare the same class, differing only in whether the
/// empty string is admitted, which the length bounds below state.
const TAG_TEXT_PATTERN: &str = r"^[\p{L}\p{Z}\p{N}_.:/=+\-@]*$";

/// The most session tags one session may hold: "You can pass up to 50 session tags"
/// (`AssumeRoleRequest$Tags`), which is also the model's own `tagListType` and
/// `tagKeyListType` length maximum.
///
/// The boundary walk bounds only the list a REQUEST carries. A chained mint also carries
/// the calling session's transitive tags forward, so the EFFECTIVE set is bounded by this
/// same maximum before a session is recorded — otherwise one stored session record would
/// grow by a request's worth of tags at every step of a role chain.
pub const MAX_SESSION_TAGS: usize = 50;

/// The longest session tag KEY the model admits (`tagKeyType`, length 1..128), counted
/// the way the operation's own documentation counts it: "The plaintext session tag keys
/// can't exceed 128 characters" (`AssumeRoleRequest$Tags`).
pub const MAX_TAG_KEY_CHARS: usize = 128;

/// The longest session tag VALUE the model admits (`tagValueType`, length 0..256): "the
/// values can't exceed 256 characters" (`AssumeRoleRequest$Tags`).
pub const MAX_TAG_VALUE_CHARS: usize = 256;

/// The compiled [`TAG_TEXT_PATTERN`], compiled once for the process.
fn tag_text_regex() -> &'static ::regex::Regex {
    static COMPILED: ::std::sync::OnceLock<::regex::Regex> = ::std::sync::OnceLock::new();
    COMPILED.get_or_init(|| {
        ::regex::Regex::new(TAG_TEXT_PATTERN).expect("the modelled tag pattern compiles")
    })
}

/// The first session tag whose key or value falls outside what the model admits, worded
/// the way AWS words a constraint violation: the member path, the value it rejected and
/// the requirement it broke.
///
/// The model constrains a session tag's key and value (`tagKeyType` length 1..128 and
/// `tagValueType` length 0..256, both over one character class), but those constraints sit
/// on members NESTED inside `tagListType`, which the boundary walk does not descend into
/// — so they are checked here, before anything is minted. Lengths are counted in
/// CHARACTERS, as the operation's documentation states the same limits.
pub fn invalid_session_tag(tags: Option<&[Tag]>) -> Option<String> {
    let violation = |position: usize, member: &str, value: &str, requirement: String| {
        Some(format!(
            "1 validation error detected: Value '{value}' at 'tags.{position}.member.{member}' \
             failed to satisfy constraint: {requirement}"
        ))
    };
    for (index, tag) in tags.unwrap_or(&[]).iter().enumerate() {
        let position = index + 1;
        let key_length = tag.key.chars().count();
        if key_length == 0 || key_length > MAX_TAG_KEY_CHARS {
            return violation(
                position,
                "key",
                &tag.key,
                format!(
                    "Member must have length less than or equal to {MAX_TAG_KEY_CHARS} and \
                     greater than or equal to 1"
                ),
            );
        }
        if tag.value.chars().count() > MAX_TAG_VALUE_CHARS {
            return violation(
                position,
                "value",
                &tag.value,
                format!("Member must have length less than or equal to {MAX_TAG_VALUE_CHARS}"),
            );
        }
        for (member, value) in [("key", &tag.key), ("value", &tag.value)] {
            if !tag_text_regex().is_match(value) {
                return violation(
                    position,
                    member,
                    value,
                    format!(
                        "Member must satisfy regular expression pattern: {}",
                        ::service_common::validation::unanchored(TAG_TEXT_PATTERN)
                    ),
                );
            }
        }
    }
    None
}

/// How AWS words the refusal of an STS operation a temporary credential may not call:
/// the operation, and the fact that the request presented session credentials.
#[must_use]
pub fn session_credentials_message(operation: &str) -> String {
    format!("Cannot call {operation} with session credentials")
}

/// The role id STS reports for a role ARN: a pure function of the ARN, so the same
/// role keeps one id across every session and two roles never share one — without
/// anything being remembered per request.
pub fn role_id(role_arn: &str) -> String {
    unique_id("AROA", role_arn)
}

/// The identity of the ambient caller: the IAM user whose long-term key this
/// emulator publishes. Every account and ARN comes from the emulator's own account
/// contract, and the user id is derived from the ARN, so it is the same on every
/// call and across restarts.
pub fn ambient_identity() -> Identity {
    let arn = ::engine::emulator_arn("iam", "", &format!("user/{AMBIENT_USER_NAME}"));
    Identity {
        user_id: unique_id("AIDA", &arn),
        account: ::engine::emulator_account_id().to_string(),
        arn,
    }
}

/// An AWS unique id: `prefix` plus 17 characters derived from `seed`.
///
/// The characters are a SHA-256 digest of `seed` mapped onto the id alphabet, so the
/// id is a function of the thing it names — the same role ARN always yields the same
/// role id, two different ones practically never collide, and nothing about the id
/// depends on when it was asked for. This is NOT base32: the mapping only needs to
/// be stable and well distributed, never decodable.
pub fn unique_id(prefix: &str, seed: &str) -> String {
    let digest = Sha256::digest(seed.as_bytes());
    let suffix = digest
        .iter()
        .take(UNIQUE_ID_SUFFIX_LEN)
        .map(|byte| char::from(UNIQUE_ID_ALPHABET[usize::from(*byte) % UNIQUE_ID_ALPHABET.len()]));
    prefix.chars().chain(suffix).collect()
}

/// An ARN in STS's own namespace. STS ARNs are global, so the region segment is
/// empty, and the account is the one the session belongs to: the account of the role
/// for an assumed role, the caller's for a federated user.
///
/// `engine::emulator_arn` fixes the account segment to the emulator's own account,
/// which is right for a resource the emulator owns; the account here is instead
/// whichever account the request named, exactly as AWS reports it.
pub fn sts_arn(account: &str, resource: &str) -> String {
    format!("arn:aws:sts::{account}:{resource}")
}

/// The account that owns a role, and the role's name, from a role ARN.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoleRef {
    pub account: String,
    pub name: String,
}

/// Split an IAM role ARN into the account that owns the role and the role's name.
///
/// An ARN is `arn:<partition>:<service>:<region>:<account>:<resource>`; a role's
/// resource is `role/<path…>/<name>`, where the path is optional and only the last
/// segment is the role's name — which is why the assumed-role ARN STS reports
/// carries the name without the path. `None` when the value is not an IAM role ARN
/// of an account.
pub fn parse_role_arn(arn: &str) -> Option<RoleRef> {
    let mut fields = arn.splitn(6, ':');
    let (Some("arn"), Some(partition), Some("iam"), Some(""), Some(account), Some(resource)) = (
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
    ) else {
        return None;
    };
    if partition.is_empty() || !is_account_id(account) {
        return None;
    }
    let name = resource.strip_prefix("role/")?.split('/').next_back()?;
    if name.is_empty() {
        return None;
    }
    Some(RoleRef {
        account: account.to_string(),
        name: name.to_string(),
    })
}

/// An AWS account id is 12 digits.
pub fn is_account_id(value: &str) -> bool {
    value.len() == 12 && value.bytes().all(|b| b.is_ascii_digit())
}

/// The percentage of the packed-policy budget a request's session policies and
/// session tags take up, or `None` when the request carries none of them — which is
/// when AWS reports no `PackedPolicySize` at all.
///
/// AWS packs the inline session policy, the managed policy ARNs and the session tags
/// into one payload and reports its size as a percentage; a request over 100 percent
/// is refused with `PackedPolicyTooLargeException`. The percentage is rounded up, so
/// anything packed at all reports at least one percent.
pub fn packed_policy_size(policy: Option<&str>, policy_arns: &[String], tags: &[Tag]) -> Option<i32> {
    let policy_bytes = policy.map_or(0, str::len);
    let arn_bytes: usize = policy_arns.iter().map(String::len).sum();
    let tag_bytes: usize = tags.iter().map(|tag| tag.key.len() + tag.value.len()).sum();
    let packed = policy_bytes + arn_bytes + tag_bytes;
    if packed == 0 {
        return None;
    }
    let percent = packed
        .saturating_mul(100)
        .div_ceil(PACKED_POLICY_PLAINTEXT_BUDGET);
    Some(i32::try_from(percent).unwrap_or(i32::MAX))
}

/// The ARNs of the managed session policies a request named.
pub fn policy_arns(policy_arns: Option<&[crate::ops::types::PolicyDescriptorType]>) -> Vec<String> {
    policy_arns
        .unwrap_or(&[])
        .iter()
        .filter_map(|descriptor| descriptor.arn.clone())
        .collect()
}

/// The session tags a request named, as key/value pairs.
pub fn tag_pairs(tags: Option<&[Tag]>) -> Vec<(String, String)> {
    tags.unwrap_or(&[])
        .iter()
        .map(|tag| (tag.key.clone(), tag.value.clone()))
        .collect()
}

/// The first session tag key that repeats an earlier one, ignoring case. AWS refuses
/// a request that passes the same session tag key twice.
pub fn duplicate_tag_key(tags: Option<&[Tag]>) -> Option<&str> {
    let tags = tags.unwrap_or(&[]);
    tags.iter().enumerate().find_map(|(index, tag)| {
        tags[..index]
            .iter()
            .any(|earlier| earlier.key.eq_ignore_ascii_case(&tag.key))
            .then_some(tag.key.as_str())
    })
}

/// The transitive session tag key the calling session carries forward that the request
/// passes AGAIN — `None` when it passes none of them.
///
/// A key the calling session carries forward is already set on the session this mint
/// continues, so a request may not set it a second time: "if you used temporary
/// credentials to perform this operation, the new session inherits any transitive session
/// tags from the calling session. If you pass a session tag with the same key as an
/// inherited tag, the operation fails" (`AssumeRoleRequest$Tags`, this service's model).
/// Keys are compared without case, as a repeat within one request is
/// ([`duplicate_tag_key`]).
pub fn inherited_tag_key_collision<'a>(
    caller: &'a Caller,
    requested: &[(String, String)],
) -> Option<&'a str> {
    caller
        .tags
        .iter()
        .map(|(key, _)| key.as_str())
        .find(|key| {
            caller
                .transitive_tag_keys
                .iter()
                .any(|transitive| transitive.eq_ignore_ascii_case(key))
                && requested
                    .iter()
                    .any(|(named, _)| named.eq_ignore_ascii_case(key))
        })
}

/// The transitive session tags a chained mint carries FORWARD from the calling session:
/// "If you set a tag key as transitive, the corresponding key and value passes to
/// subsequent sessions in a role chain" (`AssumeRoleRequest$TransitiveTagKeys`, this
/// service's model).
///
/// A tag the new request names itself is not inherited — a request that names one is
/// refused before this is reached ([`inherited_tag_key_collision`]) — and tag keys are
/// compared without case because they "are not case sensitive, but case is preserved"
/// (`GetFederationToken`, this service's model).
pub fn inherited_transitive_tags(
    caller: &Caller,
    requested: &[(String, String)],
) -> Vec<(String, String)> {
    caller
        .tags
        .iter()
        .filter(|(key, _)| {
            caller
                .transitive_tag_keys
                .iter()
                .any(|transitive| transitive.eq_ignore_ascii_case(key))
                && !requested
                    .iter()
                    .any(|(named, _)| named.eq_ignore_ascii_case(key))
        })
        .cloned()
        .collect()
}

/// Why a session policy is not a JSON policy document, as the message
/// `MalformedPolicyDocumentException` carries — `None` when it is one.
///
/// An IAM policy document is a JSON object, and the refusal's message "describes the
/// specific error" (`MalformedPolicyDocumentException`, this service's model), so it
/// is built from the submitted document rather than restating the member's type: a
/// document that does not parse is reported with the first offending character and
/// its character code, what the parser expected in its place, and the line and column
/// the failure sits at; a document that parses but is not an object is reported with
/// the JSON type it carries instead.
pub fn policy_document_error(policy: &str) -> Option<String> {
    match serde_json::from_str::<serde_json::Value>(policy) {
        Ok(value) if value.is_object() => None,
        Ok(value) => Some(format!(
            "The policy document must be a JSON object, not {}",
            json_type_name(&value)
        )),
        Err(parse) => Some(policy_parse_message(policy, &parse)),
    }
}

/// The JSON type a value carries, named the way the refusal names it.
fn json_type_name(value: &serde_json::Value) -> &'static str {
    match value {
        serde_json::Value::Null => "null",
        serde_json::Value::Bool(_) => "a JSON boolean",
        serde_json::Value::Number(_) => "a JSON number",
        serde_json::Value::String(_) => "a JSON string",
        serde_json::Value::Array(_) => "a JSON array",
        serde_json::Value::Object(_) => "a JSON object",
    }
}

/// The parse failure of a policy document, as the refusal reports it: the offending
/// character and its code (or end-of-input), what was expected there, and the
/// position of the failure in the submitted document.
fn policy_parse_message(document: &str, parse: &serde_json::Error) -> String {
    let (line, column) = (parse.line(), parse.column());
    // The parser renders "<cause> at line <l> column <c>"; the position is reported
    // in the location below, so only the cause is wanted here.
    let rendered = parse.to_string();
    let cause = rendered
        .split(" at line ")
        .next()
        .unwrap_or(&rendered)
        .trim();
    // A document that ends mid-value has no offending character to name: the
    // position is the end of what was submitted.
    let offender = if cause.starts_with("EOF while parsing") {
        None
    } else {
        character_at(document, line, column)
    };
    let expected = expected_where_parsing_stopped(cause, offender);
    let diagnosis = match offender {
        Some(offender) => format!(
            "Unexpected character ('{}' (code {})): {expected}",
            render_character(offender),
            u32::from(offender)
        ),
        None => format!("Unexpected end-of-input: {expected}"),
    };
    // The location names the line and column but NOT the document: the service does
    // not echo a submitted policy back in its refusal.
    format!(
        "Unexpected IOException: {diagnosis}\n at [Source: REDACTED \
         (`StreamReadFeature.INCLUDE_SOURCE_IN_LOCATION` disabled); \
         line: {line}, column: {column}]"
    )
}

/// What the parser expected where it stopped, from its own diagnosis of the failure.
/// A cause with no phrasing of its own is reported as the parser states it, so a
/// failure this list does not name still describes itself.
fn expected_where_parsing_stopped(cause: &str, offender: Option<char>) -> &str {
    // A comma is trailing because what follows it closes the collection, so what was
    // expected depends on which collection that is: an object wants its next field
    // name, an array its next value.
    let cause = match (cause, offender) {
        ("trailing comma", Some(']')) => "expected value",
        ("trailing comma", _) => "key must be a string",
        (cause, _) => cause,
    };
    match cause {
        "key must be a string" => "was expecting double-quote to start field name",
        "expected value" => {
            "was expecting a valid value (JSON String, Number, Array, Object or token \
             'null', 'true' or 'false')"
        }
        "expected ident" => "was expecting a valid token ('null', 'true' or 'false')",
        "expected `:`" => "was expecting a colon to separate field name and value",
        "expected `,` or `}`" => "was expecting comma to separate Object entries",
        "expected `,` or `]`" => "was expecting comma to separate Array entries",
        "trailing characters" => "was expecting end of the document",
        "EOF while parsing an object" => "expected close marker for Object",
        "EOF while parsing a list" => "expected close marker for Array",
        "EOF while parsing a string" => "expected close marker for String",
        "EOF while parsing a value" => "expected a value",
        other => other,
    }
}

/// The character the failure sits on: the `column`th byte of the `line`th line of
/// the submitted document, where both are counted from one, and `None` when the
/// position is past its end. The parser counts a column in bytes, so a position
/// inside a multi-byte character names that character.
fn character_at(document: &str, line: usize, column: usize) -> Option<char> {
    if line == 0 || column == 0 {
        return None;
    }
    let mut start = 0;
    for _ in 1..line {
        start += document[start..].find('\n')? + 1;
    }
    let mut index = start + column - 1;
    if index >= document.len() {
        return None;
    }
    while index > start && !document.is_char_boundary(index) {
        index -= 1;
    }
    document[index..].chars().next()
}

/// A character as the diagnosis prints it: itself when it is printable, an escape
/// when it is a control character that would otherwise be invisible in the message.
fn render_character(character: char) -> String {
    if character.is_control() {
        character.escape_default().to_string()
    } else {
        character.to_string()
    }
}

/// Mint a session credential and record the session it belongs to.
///
/// The access key id, the secret access key and the session token are minted by the
/// emulator's credential store, which is the only thing that creates credential
/// material: the returned credential is therefore one a later signed request
/// verifies against, and the id it carries is the registry key
/// `GetCallerIdentity` and `GetAccessKeyInfo` resolve.
///
/// Room is made in the bounded registry BEFORE any credential material is created,
/// so a refused mint leaves nothing behind. Fails only when the registry is full of
/// live sessions, or if the store minted a credential without the secrets the kind it
/// is asked for always carries — the caller refuses the request rather than answering
/// with a credential that is missing half of itself.
pub fn mint_session(
    store: &CredentialStore,
    state: &mut StsState,
    now: SystemTime,
    request: NewSession,
) -> Result<MintedSession, MintError> {
    use emulator_server::aws_credentials::{AWS_SESSION, SECRET_ACCESS_KEY, SESSION_TOKEN};

    state.make_room(now, store)?;

    let minted = store.create(&AWS_SESSION);
    let secrets = minted
        .secret(SECRET_ACCESS_KEY)
        .zip(minted.secret(SESSION_TOKEN));
    let Some((secret_access_key, session_token)) = secrets else {
        let _ = store.delete(&minted.id);
        return Err(MintError::NoSecrets);
    };
    let (secret_access_key, session_token) =
        (secret_access_key.to_string(), session_token.to_string());
    let expires_at = now + Duration::from_secs(u64::try_from(request.duration_seconds).unwrap_or(0));

    let credentials = Credentials {
        access_key_id: minted.id.clone(),
        secret_access_key,
        session_token,
        expiration: ::aws_smithy_types::DateTime::from_secs(unix_seconds(expires_at)),
    };
    let session = Session {
        access_key_id: minted.id,
        created_at: now,
        expires_at,
        duration_seconds: request.duration_seconds,
        identity: request.identity,
        kind: request.kind,
        source_identity: request.source_identity,
        tags: request.tags,
        transitive_tag_keys: request.transitive_tag_keys,
        packed_policy_size: request.packed_policy_size,
        external_id: request.external_id,
        role_arn: request.role_arn,
        revoked: false,
    };
    if let Err(full) = state.insert(session.clone()) {
        let _ = store.delete(&session.access_key_id);
        return Err(full);
    }
    Ok(MintedSession {
        credentials,
        session,
    })
}

/// Honour a request's `MinimumSessionTokenSize`, or say why it cannot be.
///
/// "STS increases the session token to at least this size, regardless of its actual
/// content" (`MinimumSessionTokenSize`, this service's model). The emulator's
/// session token is minted by the credential store and cannot be padded without
/// invalidating it, so a request asking for a token larger than the store mints is
/// refused — with the session it already minted dropped again — rather than answered
/// with a token smaller than it asked for.
pub fn honour_minimum_token_size(
    state: &mut StsState,
    store: &CredentialStore,
    minted: &MintedSession,
    minimum: Option<i32>,
) -> Result<(), String> {
    let size = minted.credentials.session_token.len();
    let minimum = minimum.unwrap_or(0).max(0);
    if usize::try_from(minimum).unwrap_or(0) <= size {
        return Ok(());
    }
    state.discard(&minted.credentials.access_key_id, store);
    Err(format!(
        "MinimumSessionTokenSize {minimum} is larger than the {size}-byte session token this \
         emulator mints, and a minted session token cannot be padded (the limit is \
         {SESSION_TOKEN_SIZE_LIMIT} bytes)"
    ))
}

/// The percentage of the session-token size limit a minted token takes up, and its
/// size in bytes: `SessionTokenUtilization` and `SessionTokenSize`, both measured on
/// the token actually returned.
pub fn session_token_metrics(credentials: &Credentials) -> (i32, i32) {
    let size = credentials.session_token.len();
    let utilization = size.saturating_mul(100) / SESSION_TOKEN_SIZE_LIMIT;
    (
        i32::try_from(utilization).unwrap_or(i32::MAX),
        i32::try_from(size).unwrap_or(i32::MAX),
    )
}

/// Seconds since the Unix epoch, for a timestamp on the wire. A time before the
/// epoch cannot arise here (every instant is `now` or later), and reads as the epoch
/// if it ever did.
fn unix_seconds(at: SystemTime) -> i64 {
    at.duration_since(UNIX_EPOCH)
        .map(|since| i64::try_from(since.as_secs()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}
