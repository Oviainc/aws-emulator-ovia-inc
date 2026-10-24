// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-sts-awssecuritytokenservicev20110615 service.
//!
//! STS is a credential vendor: every operation either mints a session (`AssumeRole`,
//! `GetSessionToken`, `GetFederationToken`) or reports on one (`GetCallerIdentity`,
//! `GetAccessKeyInfo`). This module holds the registries those operations share —
//! the sessions keyed by the access key id each was minted under, plus the role id
//! STS keeps stable per role ARN — and the pure rules the handlers apply to them, so
//! a handler file contains only its own operation's decisions.
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
/// deliberately no second map for role ids — a role id is a pure function of the role
/// ARN ([`role_id`]), so remembering one would add a request-driven store without
/// making anything more stable.
#[derive(Debug, Default)]
pub struct ComAmazonawsStsAwssecuritytokenservicev20110615State {
    /// Every session this service minted and has not pruned, keyed by its access key
    /// id. At most [`MAX_SESSIONS`] entries; each entry's tags and transitive tag
    /// keys are bounded by the model (`tagListType` / `tagKeyListType`, 0..50).
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
    state.revoke_expired(now, store);
    match state.session(access_key_id) {
        Some(session) if session.is_expired(now) => Err(ExpiredSession {
            access_key_id: access_key_id.to_string(),
        }),
        Some(session) => Ok(session.identity.clone()),
        None => Ok(ambient_identity()),
    }
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

/// Whether a session policy is a JSON policy document. An IAM policy document is a
/// JSON object; anything else — invalid JSON, or valid JSON that is not an object —
/// is what `MalformedPolicyDocumentException` reports.
pub fn is_policy_document(policy: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(policy).is_ok_and(|value| value.is_object())
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
