// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-partnercentralaccount-partnercentralaccount service.
//!
//! The connection-invitation -> connection lifecycle lives here: one stored record
//! per invitation and per connection, so `CreateConnectionInvitation`,
//! `GetConnectionInvitation`, `ListConnectionInvitations`,
//! `AcceptConnectionInvitation`, `GetConnection` and `CancelConnection` all read and
//! write ONE copy of each resource. The helpers beside the records — id and ARN
//! minting, the record-to-modelled-shape projections, the pagination cursor codec,
//! the idempotency fingerprint — are shared by those handlers rather than duplicated
//! per operation.
//!
//! The email verification codes `SendEmailVerificationCode` sends live here too:
//! one record per `(catalog, address)`, which is where the operations that SPEND a
//! code find the code this service actually sent.

use std::collections::hash_map::DefaultHasher;
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::hash::{Hash, Hasher};
use std::time::{Duration, SystemTime};

use crate::ops::types::{
    AccountSummary, ConnectionInvitationSummary, ConnectionTypeDetail, Participant,
    PartnerProfileSummary, SellerProfileSummary,
};

/// The region an emulated request is treated as having been made in when its SigV4
/// credential scope does not name one (an unsigned or anonymous request).
pub const DEFAULT_REGION: &str = "us-east-1";

/// The `arnNamespace` this service's ARNs carry, from the model's `aws.api#service`
/// trait (`smithy_query(query="service_info")`: `arnNamespace: partnercentral`).
pub const ARN_NAMESPACE: &str = "partnercentral";

/// The page size the list operations use when the caller sends no `MaxResults`: the
/// model's declared `@default` for that member (`MaxResults` default 20, range
/// 1..100).
pub const DEFAULT_MAX_RESULTS: usize = 20;

/// `InvitationStatus` values this service assigns (`INVITATION_STATUS_VALUES`).
pub const INVITATION_PENDING: &str = "PENDING";
/// The status an accepted invitation carries.
pub const INVITATION_ACCEPTED: &str = "ACCEPTED";
/// The status an invitation carries once its `ExpiresAt` has passed unanswered.
pub const INVITATION_EXPIRED: &str = "EXPIRED";

/// The `ParticipantType` of the caller that creates an invitation: it is the one
/// sending it (`PARTICIPANT_TYPE_VALUES`: RECEIVER | SENDER).
pub const PARTICIPANT_SENDER: &str = "SENDER";

/// `ConnectionType` values (`CONNECTION_TYPE_VALUES`): a collaboration between two
/// Partner Central partner profiles.
pub const CONNECTION_TYPE_OPPORTUNITY_COLLABORATION: &str = "OPPORTUNITY_COLLABORATION";
/// `ConnectionType` value for a connection to a subsidiary's AWS account.
pub const CONNECTION_TYPE_SUBSIDIARY: &str = "SUBSIDIARY";

/// `ConnectionTypeStatus` values (`CONNECTION_TYPE_STATUS_VALUES`).
pub const CONNECTION_TYPE_ACTIVE: &str = "ACTIVE";
/// The status a cancelled connection type carries.
pub const CONNECTION_TYPE_CANCELED: &str = "CANCELED";

/// `ValidationException$Reason` for a request this service refuses on its input
/// (`VALIDATION_EXCEPTION_REASON_VALUES`).
pub const REASON_REQUEST_VALIDATION_FAILED: &str = "REQUEST_VALIDATION_FAILED";
/// `ResourceNotFoundException$Reason` for an invitation that is not stored
/// (`RESOURCE_NOT_FOUND_EXCEPTION_REASON_VALUES`).
pub const REASON_CONNECTION_INVITATION_NOT_FOUND: &str = "CONNECTION_INVITATION_NOT_FOUND";
/// `ResourceNotFoundException$Reason` for a connection that is not stored.
pub const REASON_CONNECTION_NOT_FOUND: &str = "CONNECTION_NOT_FOUND";
/// `ConflictException$Reason` for an invitation whose status does not allow the
/// transition being asked for (`CONFLICT_EXCEPTION_REASON_VALUES`).
pub const REASON_INCOMPATIBLE_CONNECTION_INVITATION_STATE: &str =
    "INCOMPATIBLE_CONNECTION_INVITATION_STATE";
/// `ConflictException$Reason` for a connection type that is already cancelled.
pub const REASON_INCOMPATIBLE_CONNECTION_STATE: &str = "INCOMPATIBLE_CONNECTION_STATE";
/// `ConflictException$Reason` for a `ClientToken` replayed with a different request.
pub const REASON_CONFLICT_CLIENT_TOKEN: &str = "CONFLICT_CLIENT_TOKEN";
/// `ServiceQuotaExceededException$Reason` for a create that would take the stored
/// invitations past [`MAX_INVITATIONS`]
/// (`SERVICE_QUOTA_EXCEEDED_EXCEPTION_REASON_VALUES`).
pub const REASON_LIMIT_EXCEEDED_OPEN_CONNECTION_INVITATION: &str =
    "LIMIT_EXCEEDED_NUMBER_OF_OPEN_CONNECTION_INVITATION";
/// `ServiceQuotaExceededException$Reason` for an acceptance that would take the
/// stored connections past [`MAX_CONNECTIONS`].
pub const REASON_LIMIT_EXCEEDED_ACTIVE_CONNECTION: &str =
    "LIMIT_EXCEEDED_NUMBER_OF_ACTIVE_CONNECTION";
/// `ServiceQuotaExceededException$Reason` for a send that would take the email
/// addresses holding a verification code past [`MAX_VERIFICATION_EMAILS`].
pub const REASON_LIMIT_EXCEEDED_EMAIL: &str = "LIMIT_EXCEEDED_NUMBER_OF_EMAIL";
/// `FieldValidationError$Code` for a member whose value this service rejects
/// (`FIELD_VALIDATION_CODE_VALUES`).
pub const FIELD_CODE_INVALID_VALUE: &str = "INVALID_VALUE";
/// `FieldValidationError$Code` for a member whose FORM this service rejects.
pub const FIELD_CODE_INVALID_STRING_FORMAT: &str = "INVALID_STRING_FORMAT";
/// `FieldValidationError$Code` for a list member carrying more entries than it may.
pub const FIELD_CODE_TOO_MANY_VALUES: &str = "TOO_MANY_VALUES";

/// The resource a `ResourceNotFoundException` of an invitation lookup names, for
/// [`not_found_message`].
pub const RESOURCE_CONNECTION_INVITATION: &str = "Connection invitation";
/// The resource a `ResourceNotFoundException` of a connection lookup names.
pub const RESOURCE_CONNECTION: &str = "Connection";

/// The prefix of a `PartnerProfileId` (`^pprofile-[A-Za-z0-9]{13}$`).
pub const PARTNER_PROFILE_ID_PREFIX: &str = "pprofile-";

/// Characters in an `AwsAccountId` (`^[0-9]{12}$`).
const ACCOUNT_ID_LEN: usize = 12;

/// The digits an `EmailVerificationCode` carries: the shape is `@length(min: 6,
/// max: 6)` with `@pattern ^[0-9]+$` (`smithy_query(query="shape",
/// shape="EmailVerificationCode")`), so a code this service sends is exactly six
/// decimal digits and the operations that take one accept nothing else.
pub const VERIFICATION_CODE_DIGITS: usize = 6;

/// How long an email verification code stays held after it was sent.
///
/// CHOSEN, not cited: the model declares no lifetime for `EmailVerificationCode`
/// and no response member reports one. 30 minutes is this emulator's code
/// lifetime — long enough for a client to send a code and spend it in the same
/// session, while the removal the clock owes once that instant passes is still
/// materialised by
/// [`ComAmazonawsPartnercentralaccountPartnercentralaccountState::materialise_due_transitions`].
pub const EMAIL_VERIFICATION_CODE_TTL: Duration = Duration::from_secs(30 * 60);

/// How long a connection invitation stays answerable before it expires.
///
/// CHOSEN, not cited: the model declares `ExpiresAt` optional and documents it only
/// as "the timestamp when the connection invitation will expire if not responded
/// to", naming no span, and the request carries none either. 30 days is this
/// emulator's invitation lifetime — long enough that an invitation a client creates
/// stays `PENDING` for the whole of its session, while the `PENDING -> EXPIRED`
/// transition the clock owes is still materialised by
/// [`ComAmazonawsPartnercentralaccountPartnercentralaccountState::materialise_due_transitions`]
/// once that instant passes.
pub const INVITATION_TTL: Duration = Duration::from_secs(30 * 24 * 60 * 60);

/// The alphabet a minted resource id is drawn from: the modelled id patterns are
/// `^pacinv-[A-Za-z0-9]{13}$` and `^pac-[A-Za-z0-9]{13}$`.
const ID_ALPHABET: &[u8; 62] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";

/// Characters after the prefix in a minted id, from those same patterns.
const ID_BODY_LEN: usize = 13;

/// The prefix of a `ConnectionInvitationId`.
pub const INVITATION_ID_PREFIX: &str = "pacinv-";

/// The prefix of a `ConnectionId`.
pub const CONNECTION_ID_PREFIX: &str = "pac-";

/// The prefix a pagination cursor carries, so a token this service did not mint is
/// refused instead of being read as a resume point.
const PAGE_TOKEN_PREFIX: &str = "v1:";

/// One stored connection invitation: every value the service minted or persisted at
/// create time, so `Get`/`List` report exactly what `Create` returned instead of
/// recomputing anything from the later request.
#[derive(Debug, Clone)]
pub struct ConnectionInvitationRecord {
    pub catalog: String,
    pub id: String,
    pub arn: String,
    /// The connection this invitation produced, set when it is accepted.
    pub connection_id: Option<String>,
    pub connection_type: String,
    pub created_at: SystemTime,
    pub updated_at: SystemTime,
    pub expires_at: SystemTime,
    pub other_participant_identifier: String,
    pub participant_type: String,
    pub status: String,
    pub invitation_message: String,
    pub inviter_email: String,
    pub inviter_name: String,
}

impl ConnectionInvitationRecord {
    /// This record as a `ListConnectionInvitations` summary: the stored record's own
    /// fields, never request input.
    pub fn summary(&self) -> ConnectionInvitationSummary {
        ConnectionInvitationSummary {
            catalog: self.catalog.clone(),
            id: self.id.clone(),
            arn: self.arn.clone(),
            connection_id: self.connection_id.clone(),
            connection_type: self.connection_type.clone(),
            created_at: ::aws_smithy_types::DateTime::from(self.created_at),
            updated_at: ::aws_smithy_types::DateTime::from(self.updated_at),
            expires_at: Some(::aws_smithy_types::DateTime::from(self.expires_at)),
            other_participant_identifier: self.other_participant_identifier.clone(),
            participant_type: self.participant_type.clone(),
            status: self.status.clone(),
        }
    }
}

/// One connection type of a stored connection, with the cancellation the
/// `CancelConnection` call recorded for it.
#[derive(Debug, Clone)]
pub struct ConnectionTypeDetailRecord {
    pub created_at: SystemTime,
    pub inviter_email: String,
    pub inviter_name: String,
    pub status: String,
    pub canceled_at: Option<SystemTime>,
    pub canceled_by: Option<String>,
    /// The `Reason` the cancelling call gave. Persisted with the cancellation it
    /// explains; no modelled response member carries it back today.
    pub cancellation_reason: Option<String>,
    /// The counterparty's `ParticipantIdentifier`, carried over from the invitation
    /// this connection type came from and projected by [`participant_for`].
    pub other_participant_identifier: String,
}

impl ConnectionTypeDetailRecord {
    /// This record as the modelled `ConnectionTypeDetail`.
    pub fn detail(&self, other_participant_account_id: &str) -> ConnectionTypeDetail {
        ConnectionTypeDetail {
            created_at: ::aws_smithy_types::DateTime::from(self.created_at),
            inviter_email: self.inviter_email.clone(),
            inviter_name: self.inviter_name.clone(),
            status: self.status.clone(),
            canceled_at: self.canceled_at.map(::aws_smithy_types::DateTime::from),
            canceled_by: self.canceled_by.clone(),
            other_participant: participant_for(
                &self.other_participant_identifier,
                other_participant_account_id,
            ),
        }
    }
}

/// One stored connection: the resource `AcceptConnectionInvitation` created and
/// `GetConnection` / `CancelConnection` read and update.
#[derive(Debug, Clone)]
pub struct ConnectionRecord {
    pub catalog: String,
    pub id: String,
    pub arn: String,
    pub other_participant_account_id: String,
    pub updated_at: SystemTime,
    /// `ConnectionType` -> its detail. A `BTreeMap` so the map a response carries is
    /// built in one stable order.
    pub connection_types: BTreeMap<String, ConnectionTypeDetailRecord>,
}

impl ConnectionRecord {
    /// The stored connection types as the modelled `ConnectionTypeDetailMap`.
    pub fn connection_type_details(
        &self,
    ) -> ::std::collections::HashMap<String, ConnectionTypeDetail> {
        self.connection_types
            .iter()
            .map(|(connection_type, detail)| {
                (
                    connection_type.clone(),
                    detail.detail(&self.other_participant_account_id),
                )
            })
            .collect()
    }
}

/// The email verification code this service last sent to one address of one
/// catalog.
///
/// `SendEmailVerificationCode` returns nothing, so the code it sent lives only
/// here: the operations that spend one (`CreatePartner`,
/// `AssociateAwsTrainingCertificationEmailDomain`) take an `EmailVerificationCode`
/// and have to check it against the code the service actually sent. Never put
/// `code` in a response, a message or a log — `EmailVerificationCode` is
/// `@sensitive`.
#[derive(Debug, Clone)]
pub struct EmailVerificationCodeRecord {
    pub catalog: String,
    pub email: String,
    /// The six digits this service sent, from
    /// [`ComAmazonawsPartnercentralaccountPartnercentralaccountState::mint_verification_code`].
    pub code: String,
    pub sent_at: SystemTime,
    /// When the code falls due for removal ([`EMAIL_VERIFICATION_CODE_TTL`] after
    /// [`Self::sent_at`]).
    pub expires_at: SystemTime,
    /// How many codes have been sent to this address while one was held, so a
    /// resend is a resend rather than a first send.
    pub sends: u64,
}

/// What a completed, idempotency-token-bearing call recorded, so a replay of that
/// exact call answers with the resource it created rather than creating a second one.
#[derive(Debug, Clone)]
pub struct IdempotentCall {
    /// The id of the resource the first call created or updated.
    pub resource_id: String,
    /// A digest of the request the token arrived with, so the SAME token sent with a
    /// DIFFERENT request is refused instead of silently answering about the first.
    pub fingerprint: String,
}

/// The most `@idempotencyToken` records this service remembers, across every
/// operation that takes one.
///
/// Tokens are caller-supplied, so this map is caller-driven growth that holds no
/// resource of its own: beyond this many live records the oldest is evicted
/// ([`IdempotencyTokens::record`]). An evicted token is indistinguishable from one
/// whose idempotency window has passed — the next call carrying it is served as a
/// new request.
pub const MAX_IDEMPOTENCY_TOKENS: usize = 512;

/// A bounded `(operation, client token)` -> [`IdempotentCall`] store.
///
/// Insertion-ordered and capped at [`MAX_IDEMPOTENCY_TOKENS`]: recording the
/// (cap + 1)-th token forgets the oldest one rather than growing the map. A record
/// that is replayed keeps its original eviction position — the idempotency window of
/// a token starts when it was FIRST served, so replaying it does not extend how long
/// it is remembered.
#[derive(Debug, Default)]
pub struct IdempotencyTokens {
    by_key: HashMap<String, IdempotentCall>,
    oldest_first: VecDeque<String>,
}

/// The separator between the operation name and the client token in a record's key.
/// A unit separator, which neither an operation name nor a `ClientToken`
/// (`^[A-Za-z0-9-_]+$`) can contain, so two different pairs cannot compose one key.
const KEY_SEPARATOR: char = '\u{1f}';

impl IdempotencyTokens {
    /// The key a record is held under: composed once per call, so a lookup allocates
    /// one string rather than an owned tuple.
    fn key(operation: &str, token: &str) -> String {
        format!("{operation}{KEY_SEPARATOR}{token}")
    }

    /// What the earlier call carrying `token` on `operation` did, if it is still
    /// remembered.
    pub fn get(&self, operation: &str, token: &str) -> Option<&IdempotentCall> {
        self.by_key.get(&Self::key(operation, token))
    }

    /// Remember `call` under `(operation, token)`, evicting the oldest records to
    /// stay within [`MAX_IDEMPOTENCY_TOKENS`].
    pub fn record(&mut self, operation: &str, token: &str, call: IdempotentCall) {
        let key = Self::key(operation, token);
        if self.by_key.insert(key.clone(), call).is_none() {
            self.oldest_first.push_back(key);
        }
        while self.oldest_first.len() > MAX_IDEMPOTENCY_TOKENS {
            if let Some(evicted) = self.oldest_first.pop_front() {
                self.by_key.remove(&evicted);
            }
        }
    }

    /// How many token records are held right now.
    pub fn len(&self) -> usize {
        self.by_key.len()
    }

    /// Whether no token record is held.
    pub fn is_empty(&self) -> bool {
        self.by_key.is_empty()
    }
}

/// The most invitations this emulator stores before `CreateConnectionInvitation`
/// answers its modelled `ServiceQuotaExceededException`.
///
/// The real service's invitation quota is not in the model, so this ceiling is
/// CHOSEN, not cited: it is set far above any plausible client's invitation count,
/// so normal use never meets it, while the store a caller can grow stays bounded.
pub const MAX_INVITATIONS: usize = 1_000;

/// The most email addresses this emulator holds a verification code for before
/// `SendEmailVerificationCode` answers its modelled `ServiceQuotaExceededException`
/// with [`REASON_LIMIT_EXCEEDED_EMAIL`].
///
/// The real service's per-account email quota is not in the model, so this ceiling
/// is CHOSEN, not cited, for the same reason as [`MAX_INVITATIONS`]: it is far above
/// any plausible client's address count, so normal use never meets it, while the
/// store a caller can grow by sending codes stays bounded.
pub const MAX_VERIFICATION_EMAILS: usize = 100;

/// The most connections this emulator stores before `AcceptConnectionInvitation`
/// answers its modelled `ServiceQuotaExceededException`. CHOSEN for the same reason
/// as [`MAX_INVITATIONS`], and smaller because a connection outlives the invitation
/// that produced it.
pub const MAX_CONNECTIONS: usize = 500;

/// The largest page either list operation serves, from `MaxResults`' modelled
/// `@range` maximum of 100 — applied in the handler as well, so the cap holds even
/// if the request reaches it without the framework's range check.
pub const MAX_PAGE_SIZE: usize = 100;

/// The most identifiers a list operation's `OtherParticipantIdentifiers` filter may
/// carry.
///
/// CITED: `ListConnectionInvitationsRequest$OtherParticipantIdentifiers` declares
/// `@length(max: 100)` (`smithy_query(query="shape",
/// shape="ListConnectionInvitationsRequest")`). The trait sits on the member rather
/// than on `ParticipantIdentifierList`, so the handler enforces it itself: a filter
/// is matched against every stored invitation of the catalog, and bounding the list
/// is what keeps that product bounded.
pub const MAX_FILTER_IDENTIFIERS: usize = 100;

/// Per-service state for com-amazonaws-partnercentralaccount-partnercentralaccount. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsPartnercentralaccountPartnercentralaccountState {
    /// Every stored invitation, keyed by `(catalog, invitation id)`. A `BTreeMap`
    /// so one catalog's invitations are walked in a single stable order — minted ids
    /// ascend with the mint counter, so that order is creation order and a cursor
    /// taken on one page resumes exactly where it stopped. Bounded by
    /// [`MAX_INVITATIONS`], which `CreateConnectionInvitation` checks before it
    /// inserts.
    pub invitations: BTreeMap<(String, String), ConnectionInvitationRecord>,
    /// Every stored connection, keyed by `(catalog, connection id)`. Bounded by
    /// [`MAX_CONNECTIONS`], which `AcceptConnectionInvitation` checks before it
    /// inserts.
    pub connections: BTreeMap<(String, String), ConnectionRecord>,
    /// `(operation, client token)` -> what that call did, so a retry carrying an
    /// `@idempotencyToken` this service already served replays instead of
    /// duplicating. Bounded by [`MAX_IDEMPOTENCY_TOKENS`], which evicts its oldest
    /// record rather than growing.
    pub idempotency: IdempotencyTokens,
    /// The verification code last sent to each address, keyed by `(catalog,
    /// email)` — one record per address, since a newly sent code replaces the one
    /// it supersedes. Bounded by [`MAX_VERIFICATION_EMAILS`], which
    /// `SendEmailVerificationCode` checks before it inserts, and shortened by
    /// [`Self::materialise_due_transitions`] as codes fall due.
    pub email_verification_codes: BTreeMap<(String, String), EmailVerificationCodeRecord>,
    /// Monotonic counter behind every minted invitation id; never returned to a
    /// caller.
    next_invitation_seq: u64,
    /// Monotonic counter behind every minted connection id.
    next_connection_seq: u64,
    /// Monotonic counter behind every minted email verification code.
    next_verification_code_seq: u64,
}

impl ComAmazonawsPartnercentralaccountPartnercentralaccountState {
    /// Mint a `ConnectionInvitationId` matching `^pacinv-[A-Za-z0-9]{13}$`.
    ///
    /// The body is the mint counter in base 62, so every id is unique within this
    /// emulator and ids ascend in creation order.
    pub fn mint_invitation_id(&mut self) -> String {
        self.next_invitation_seq = self.next_invitation_seq.wrapping_add(1);
        format!(
            "{INVITATION_ID_PREFIX}{}",
            base62_body(self.next_invitation_seq)
        )
    }

    /// Mint a `ConnectionId` matching `^pac-[A-Za-z0-9]{13}$`.
    pub fn mint_connection_id(&mut self) -> String {
        self.next_connection_seq = self.next_connection_seq.wrapping_add(1);
        format!(
            "{CONNECTION_ID_PREFIX}{}",
            base62_body(self.next_connection_seq)
        )
    }

    /// Mint an `EmailVerificationCode` matching `^[0-9]{6}$`.
    ///
    /// The code is the mint counter rendered as [`VERIFICATION_CODE_DIGITS`]
    /// decimal digits, so a code comes from this service's own state rather than
    /// from the address it is sent to. The addresses holding a code are bounded by
    /// [`MAX_VERIFICATION_EMAILS`], far below the codes that many digits spell, so
    /// stepping the counter past the codes already held terminates and two live
    /// addresses never hold the same code — including after the counter has wrapped
    /// that space.
    pub fn mint_verification_code(&mut self) -> String {
        let mut code = self.next_verification_code();
        for _ in 0..MAX_VERIFICATION_EMAILS {
            if !self.verification_code_is_held(&code) {
                break;
            }
            code = self.next_verification_code();
        }
        code
    }

    /// The next code the mint counter spells.
    fn next_verification_code(&mut self) -> String {
        self.next_verification_code_seq = self.next_verification_code_seq.wrapping_add(1);
        let space = 10u64.pow(VERIFICATION_CODE_DIGITS as u32);
        format!(
            "{:0width$}",
            self.next_verification_code_seq % space,
            width = VERIFICATION_CODE_DIGITS
        )
    }

    /// Whether some address already holds `code`.
    fn verification_code_is_held(&self, code: &str) -> bool {
        self.email_verification_codes
            .values()
            .any(|record| record.code == code)
    }

    /// Apply every transition the CLOCK has already made due.
    ///
    /// An invitation nobody answered before its `ExpiresAt` is `EXPIRED` from that
    /// instant on, whether or not anyone called; `UpdatedAt` moves to the instant the
    /// expiry fell due, not to now, because that is when the invitation changed. An
    /// email verification code whose lifetime has passed is no longer held, for the
    /// same reason: it stopped being spendable at that instant even if nobody
    /// called.
    /// Called by the reporting operations as well as the mutating ones: a change that
    /// was owed at this instant anyway is the only write a read may perform.
    pub fn materialise_due_transitions(&mut self, now: SystemTime) {
        for invitation in self.invitations.values_mut() {
            if invitation.status == INVITATION_PENDING && now >= invitation.expires_at {
                invitation.status = INVITATION_EXPIRED.to_string();
                invitation.updated_at = invitation.expires_at;
            }
        }
        self.email_verification_codes
            .retain(|_, record| now < record.expires_at);
    }

    /// What the earlier call carrying `token` on `operation` did, or `None` when this
    /// service has not served that token for that operation (or no longer remembers
    /// it — see [`MAX_IDEMPOTENCY_TOKENS`]).
    pub fn replay(&self, operation: &str, token: &str) -> Option<&IdempotentCall> {
        self.idempotency.get(operation, token)
    }

    /// Record that `operation` served `token` by creating or updating `resource_id`,
    /// within the bound [`MAX_IDEMPOTENCY_TOKENS`] sets.
    pub fn record_idempotent_call(
        &mut self,
        operation: &str,
        token: &str,
        resource_id: &str,
        fingerprint: &str,
    ) {
        self.idempotency.record(
            operation,
            token,
            IdempotentCall {
                resource_id: resource_id.to_string(),
                fingerprint: fingerprint.to_string(),
            },
        );
    }
}

/// `value` rendered as [`ID_BODY_LEN`] base-62 characters from [`ID_ALPHABET`],
/// most significant first and left-padded, so ids sort in minting order.
fn base62_body(value: u64) -> String {
    let mut body = [b'0'; ID_BODY_LEN];
    let mut remaining = value;
    let mut index = ID_BODY_LEN;
    while index > 0 {
        index -= 1;
        body[index] = ID_ALPHABET[(remaining % 62) as usize];
        remaining /= 62;
    }
    String::from_utf8(body.to_vec()).expect("the id alphabet is ASCII")
}

/// Whether `value` has the shape of a minted id with `prefix`: the prefix followed
/// by exactly [`ID_BODY_LEN`] alphanumeric characters.
fn has_id_shape(value: &str, prefix: &str) -> bool {
    value.strip_prefix(prefix).is_some_and(|body| {
        body.len() == ID_BODY_LEN && body.bytes().all(|byte| byte.is_ascii_alphanumeric())
    })
}

/// Whether `value` is a `ConnectionInvitationId`.
pub fn is_invitation_id(value: &str) -> bool {
    has_id_shape(value, INVITATION_ID_PREFIX)
}

/// Whether `value` has the form of a `PartnerProfileId`
/// (`PartnerProfileSummary$Id`: `^pprofile-[A-Za-z0-9]{13}$`).
pub fn is_partner_profile_id(value: &str) -> bool {
    has_id_shape(value, PARTNER_PROFILE_ID_PREFIX)
}

/// Whether `value` has the form of an `AwsAccountId` (`^[0-9]{12}$`), the shape this
/// service's `CanceledBy` and `OtherParticipantAccountId` members carry.
pub fn is_aws_account_id(value: &str) -> bool {
    value.len() == ACCOUNT_ID_LEN && value.bytes().all(|byte| byte.is_ascii_digit())
}

/// Whether `receiver` names a participant an invitation of `connection_type` can be
/// addressed to.
///
/// A connection joins two participants of a particular KIND, and each kind has its own
/// identifier form in the model's `Participant` union: an
/// `OPPORTUNITY_COLLABORATION` joins two Partner Central partner profiles, which are
/// identified by a `PartnerProfileId` (`^pprofile-[A-Za-z0-9]{13}$`), while a
/// `SUBSIDIARY` connection joins the caller to a subsidiary's AWS account, identified
/// by an `AwsAccountId` (`^[0-9]{12}$`). `ParticipantIdentifier`'s own pattern
/// (`^[a-zA-Z0-9-]+$`) is wide enough to spell either and much else besides, so the
/// pair is what decides whether a receiver is addressable at all.
pub fn receiver_is_addressable(connection_type: &str, receiver: &str) -> bool {
    match connection_type {
        CONNECTION_TYPE_OPPORTUNITY_COLLABORATION => is_partner_profile_id(receiver),
        CONNECTION_TYPE_SUBSIDIARY => is_aws_account_id(receiver),
        // A `ConnectionType` the model does not declare never reaches a handler (the
        // boundary refuses it), and nothing is addressable under it.
        _ => false,
    }
}

/// The message a lookup that found nothing states: the kind of resource, then the
/// catalog and the identifier the request named it by.
///
/// One rule for every lookup in this service, so the four handlers that raise
/// `ResourceNotFoundException` cannot word the same condition differently.
pub fn not_found_message(resource: &str, catalog: &str, identifier: &str) -> String {
    format!("{resource} not found with catalog: {catalog} and identifier: {identifier}.")
}

/// The region of the request being served: the third field of its SigV4 credential
/// scope, falling back to [`DEFAULT_REGION`] for a request that is not signed or
/// whose scope names a region the ARN's region segment (`[a-z0-9\-]*`) cannot carry.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .filter(|region| {
            !region.is_empty()
                && region
                    .chars()
                    .all(|c| c == '-' || c.is_ascii_lowercase() || c.is_ascii_digit())
        })
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// Whether `catalog` can appear in this service's resource ARNs.
///
/// Both `ConnectionInvitationArn` and `ConnectionArn` spell the catalog segment
/// `catalog/[a-zA-Z]+`, while the `Catalog` member itself is `^[a-zA-Z0-9-]+$`: a
/// catalog carrying a digit or a dash therefore names no resource this service could
/// mint an ARN for, so a create in it is refused rather than answered with an ARN
/// that violates its own modelled pattern.
pub fn catalog_is_arn_safe(catalog: &str) -> bool {
    !catalog.is_empty() && catalog.chars().all(|c| c.is_ascii_alphabetic())
}

/// The ARN partition a region belongs to, as the modelled ARN pattern's
/// `arn:[a-zA-Z0-9-]+:` partition segment allows.
///
/// AWS's published region-to-partition prefixes: `cn-*` is `aws-cn`, `us-gov-*` is
/// `aws-us-gov`, the ISO regions carry their own partitions (`us-iso-*` ->
/// `aws-iso`, `us-isob-*` -> `aws-iso-b`, `eu-isoe-*` -> `aws-iso-e`, `us-isof-*` ->
/// `aws-iso-f`), and every commercial region is `aws`. Matched longest prefix first,
/// so `us-isob-east-1` is not read as a `us-iso-*` region.
pub fn partition_for(region: &str) -> &'static str {
    const PARTITIONS: &[(&str, &str)] = &[
        ("cn-", "aws-cn"),
        ("us-gov-", "aws-us-gov"),
        ("us-isob-", "aws-iso-b"),
        ("us-isof-", "aws-iso-f"),
        ("eu-isoe-", "aws-iso-e"),
        ("us-iso-", "aws-iso"),
    ];
    PARTITIONS
        .iter()
        .find(|(prefix, _)| region.starts_with(prefix))
        .map_or("aws", |(_, partition)| *partition)
}

/// The ARN of a connection invitation:
/// `arn:<partition>:partnercentral:<region>::catalog/<catalog>/connection-invitation/<id>`.
///
/// Built here rather than with `engine::emulator_arn`, which always writes the
/// emulator account id into the account segment and fixes the partition at `aws`:
/// this service's ARNs have an EMPTY account segment, as the modelled pattern
/// (`^arn:[a-zA-Z0-9-]+:partnercentral:[a-z0-9\-]*::catalog/…`) requires, and the
/// partition follows the request's region ([`partition_for`]).
pub fn invitation_arn(region: &str, catalog: &str, id: &str) -> String {
    format!(
        "arn:{}:{ARN_NAMESPACE}:{region}::catalog/{catalog}/connection-invitation/{id}",
        partition_for(region)
    )
}

/// The ARN of a connection:
/// `arn:<partition>:partnercentral:<region>::catalog/<catalog>/connection/<id>`,
/// built the same way as [`invitation_arn`].
pub fn connection_arn(region: &str, catalog: &str, id: &str) -> String {
    format!(
        "arn:{}:{ARN_NAMESPACE}:{region}::catalog/{catalog}/connection/{id}",
        partition_for(region)
    )
}

/// The other participant of a connection type, as the identity the invitation named
/// it by.
///
/// An invitation carries one datum about the counterparty — its
/// `ParticipantIdentifier` — and the model's own id constraints decide which
/// `Participant` variant can carry it:
///
/// * `PartnerProfileSummary$Id` is `^pprofile-[A-Za-z0-9]{13}$`, so an identifier of
///   that form names a partner profile;
/// * `AwsAccountId` is `^[0-9]{12}$`, so an identifier of that form names an AWS
///   account — the form a `SUBSIDIARY` receiver carries
///   ([`receiver_is_addressable`]), and the same value the connection's
///   `OtherParticipantAccountId` reports;
/// * `SellerProfileSummary$Id` is `^[a-zA-Z0-9-]+$` with length 1..50, which every
///   other identifier of at most 50 characters satisfies;
/// * a longer identifier fits neither profile id, so the counterparty is reported as
///   the AWS account the connection already names.
///
/// A profile's `Name` is the identifier itself: this service holds no display name
/// for a participant until the partner-profile operations are implemented, and
/// inventing one would report a value no call ever supplied.
pub fn participant_for(identifier: &str, other_participant_account_id: &str) -> Participant {
    if is_partner_profile_id(identifier) {
        return Participant::PartnerProfile(PartnerProfileSummary {
            id: identifier.to_string(),
            name: identifier.to_string(),
        });
    }
    if is_aws_account_id(identifier) {
        return Participant::Account(AccountSummary {
            name: other_participant_account_id.to_string(),
        });
    }
    if identifier.len() <= 50 {
        return Participant::SellerProfile(SellerProfileSummary {
            id: identifier.to_string(),
            name: identifier.to_string(),
        });
    }
    Participant::Account(AccountSummary {
        name: other_participant_account_id.to_string(),
    })
}

/// Encode the invitation id a `ListConnectionInvitations` page resumes at as an
/// opaque `NextToken`.
///
/// Hex of the id behind a version prefix, rather than the id itself, so a caller has
/// no structure to read into the token and nothing to build one from: a `NextToken`
/// is opaque and only ever travels back unchanged.
pub fn encode_page_token(invitation_id: &str) -> String {
    format!("{PAGE_TOKEN_PREFIX}{}", ::hex::encode(invitation_id))
}

/// The invitation id a `NextToken` resumes at, or `None` when the token is not one
/// this service minted (the caller answers that with a `ValidationException`).
pub fn decode_page_token(token: &str) -> Option<String> {
    let body = token.strip_prefix(PAGE_TOKEN_PREFIX)?;
    let id = String::from_utf8(::hex::decode(body).ok()?).ok()?;
    is_invitation_id(&id).then_some(id)
}

/// A digest of the values a request carried, used to tell a genuine retry (same
/// token, same request) from a token reused for a different request.
pub fn fingerprint(values: &[&str]) -> String {
    let mut hasher = DefaultHasher::new();
    for value in values {
        value.hash(&mut hasher);
        // Length-delimited, so ("a", "bc") and ("ab", "c") do not collide.
        value.len().hash(&mut hasher);
    }
    format!("{:016x}", hasher.finish())
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsPartnercentralaccountPartnercentralaccountState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsPartnercentralaccountPartnercentralaccountState>)
}
