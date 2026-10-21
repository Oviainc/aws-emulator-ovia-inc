// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-pinpointemail-amazonpinpointemailservice service.
//!
//! One process-wide store behind the engine's mutex, holding the three collections the
//! implemented slice of this service computes its answers from: verified email
//! identities, configuration sets, and the log of messages `SendEmail` accepted.
//!
//! Both maps are [`BTreeMap`]s, so iteration order is the stored keys' order rather
//! than a hash order that varies per process: `ListEmailIdentities` pages over a stable
//! sequence, which is what makes its `NextToken` mean the same thing on the next call.

use std::collections::{BTreeMap, VecDeque};
use std::time::{Duration, SystemTime};

use crate::ops::types::{
    DeliveryOptions, DkimAttributes, EventDestinationDefinition, MailFromAttributes,
    ReputationOptions, SendingOptions, Tag, TrackingOptions,
};

use sha2::{Digest, Sha256};

/// What a handler says when the engine it was constructed with does not carry this
/// service's store. The router always builds the emulator on `engine::InMemoryEngine`,
/// so this is unreachable there; a handler must still answer a modelled error rather
/// than construct a private engine, whose writes nobody else would ever see.
pub const STATE_UNAVAILABLE: &str =
    "the Pinpoint Email emulator's state store is not available on this engine";

/// `IdentityType` for an identity that is an email address (`IdentityType` enum,
/// `shared_types::IDENTITY_TYPE_VALUES`).
pub const IDENTITY_TYPE_EMAIL_ADDRESS: &str = "EMAIL_ADDRESS";
/// `IdentityType` for an identity that is a domain.
pub const IDENTITY_TYPE_DOMAIN: &str = "DOMAIN";

/// `DkimStatus` for a domain whose CNAME records have not been detected yet
/// (`DkimStatus` enum, `shared_types::DKIM_STATUS_VALUES`).
pub const DKIM_STATUS_PENDING: &str = "PENDING";
/// `DkimStatus` for a domain whose CNAME records have been detected and are correct.
pub const DKIM_STATUS_SUCCESS: &str = "SUCCESS";

/// How long after `CreateEmailIdentity` an identity's verification is due.
///
/// A new identity is reported in a non-terminal state — not verified for sending, DKIM
/// `PENDING` — and real verification finishes out of band: a domain when its DKIM CNAME
/// records appear in DNS (AWS documents "around 72 hours"), an address when its owner
/// follows the link in the verification email. Neither instant is one the emulator can
/// observe, so it owes the transition on its own clock, at a chosen span: long enough
/// that a caller reading straight after the create still sees the pending identity, short
/// enough that a client script never waits on it. The instant is stored per record and
/// materialised when a later call reads it; nothing sleeps.
pub const VERIFICATION_DUE_AFTER: Duration = Duration::from_millis(500);

/// How many DKIM tokens a domain identity gets. `DkimAttributes$Tokens` is documented as
/// "a set of unique strings that you use to create a set of CNAME records"; AWS hands out
/// three of them for a domain.
const DKIM_TOKEN_COUNT: usize = 3;

/// The most email identities this store holds. AWS caps how many identities an account
/// may have in a Region, and the emulator needs a cap of its own anyway: without one a
/// caller could grow the store until the process runs out of memory. A create past the cap
/// is refused with the modelled `LimitExceededException` ("There are too many instances of
/// the specified resource type"), never served.
pub const MAX_EMAIL_IDENTITIES: usize = 10_000;

/// The most configuration sets this store holds — see [`MAX_EMAIL_IDENTITIES`] for why the
/// cap exists and how a create past it is refused.
pub const MAX_CONFIGURATION_SETS: usize = 10_000;

/// How many accepted messages the sent-message log retains.
///
/// The log is an emulator-side record rather than an API resource: no operation reads an
/// individual entry, so it is a ring buffer and the oldest entry is dropped once the cap
/// is reached, which bounds the memory a sequence of sends can occupy. `MessageId`
/// uniqueness does not depend on the log, because `minted_message_ids` is monotone and is
/// never reset.
pub const SENT_MESSAGE_LOG_LIMIT: usize = 1_000;

/// How much of a caller-supplied address or resource name a log entry retains: the longest
/// an email address can be — a 64-character local part, `@`, and a 255-character domain.
const RETAINED_ADDRESS_CHARS: usize = 320;

/// How much of a message's subject a log entry retains.
const RETAINED_SUBJECT_CHARS: usize = 256;

/// A log entry's copy of a caller-supplied address or resource name.
pub fn retained_address(value: &str) -> String {
    value.chars().take(RETAINED_ADDRESS_CHARS).collect()
}

/// A log entry's copy of a caller-supplied subject.
pub fn retained_subject(value: &str) -> String {
    value.chars().take(RETAINED_SUBJECT_CHARS).collect()
}

/// One stored email identity — what `CreateEmailIdentity` persists and
/// `GetEmailIdentity` / `ListEmailIdentities` report.
#[derive(Debug, Clone)]
pub struct EmailIdentityRecord {
    /// The address or domain this identity was created under; also its key in the store.
    pub identity_name: String,
    /// One of `IDENTITY_TYPE_EMAIL_ADDRESS` / `IDENTITY_TYPE_DOMAIN`, classified from the
    /// identity's own form by [`classify_identity_type`]. `MANAGED_DOMAIN` is a domain AWS
    /// itself manages, so no `CreateEmailIdentity` call produces it.
    pub identity_type: String,
    /// Whether the identity finished verification. A newly created identity has not: it
    /// becomes verified when the transition [`VERIFICATION_DUE_AFTER`] describes comes
    /// due.
    pub verified_for_sending: bool,
    /// Whether bounce and complaint feedback is forwarded by email. AWS enables feedback
    /// forwarding for a new identity; `PutEmailIdentityFeedbackAttributes` changes it.
    pub feedback_forwarding_enabled: bool,
    /// Whether email may be sent from the identity (`IdentityInfo$SendingEnabled`), which
    /// follows verification.
    pub sending_enabled: bool,
    /// DKIM configuration, present for a domain identity and absent for an address.
    pub dkim: Option<DkimAttributes>,
    /// A custom MAIL FROM domain, set by `PutEmailIdentityMailFromAttributes`.
    pub mail_from: Option<MailFromAttributes>,
    /// The tags the create call carried.
    pub tags: Vec<Tag>,
    /// When the identity was created.
    pub created_at: SystemTime,
    /// When this identity's verification comes due, or `None` once it has been applied.
    pub verification_due_at: Option<SystemTime>,
}

impl EmailIdentityRecord {
    /// A freshly created, not-yet-verified identity for `identity_name`.
    pub fn new(identity_name: &str, created_at: SystemTime) -> Self {
        let identity_type = classify_identity_type(identity_name);
        let dkim = (identity_type == IDENTITY_TYPE_DOMAIN).then(|| DkimAttributes {
            signing_enabled: true,
            status: Some(DKIM_STATUS_PENDING.to_string()),
            tokens: Some(dkim_tokens(identity_name)),
        });
        Self {
            identity_name: identity_name.to_string(),
            identity_type: identity_type.to_string(),
            verified_for_sending: false,
            feedback_forwarding_enabled: true,
            sending_enabled: false,
            dkim,
            mail_from: None,
            tags: Vec::new(),
            created_at,
            verification_due_at: Some(created_at + VERIFICATION_DUE_AFTER),
        }
    }

    /// Apply the verification the clock has already made due.
    ///
    /// Safe for a reporting operation to call before it answers: the transition was owed
    /// at `verification_due_at` whether or not anybody called, so materialising it on a
    /// read shows no caller anything it would not have seen anyway. It is the only write
    /// a read makes.
    pub fn materialize_due_verification(&mut self, now: SystemTime) {
        let Some(due) = self.verification_due_at else {
            return;
        };
        if now < due {
            return;
        }
        self.verification_due_at = None;
        self.verified_for_sending = true;
        self.sending_enabled = true;
        if let Some(dkim) = &mut self.dkim {
            dkim.status = Some(DKIM_STATUS_SUCCESS.to_string());
        }
    }
}

/// Classify an identity from its own form: an email address carries a local part and a
/// domain separated by `@`, anything else names a domain.
pub fn classify_identity_type(identity_name: &str) -> &'static str {
    if identity_name.contains('@') {
        IDENTITY_TYPE_EMAIL_ADDRESS
    } else {
        IDENTITY_TYPE_DOMAIN
    }
}

/// The DKIM tokens for a domain identity.
///
/// The tokens are opaque strings a customer copies into DNS; nothing in the API derives
/// meaning from them, so the emulator derives them from the domain itself with SHA-256
/// (truncated to 16 bytes and hex-encoded, giving the 32-character lowercase
/// alphanumeric token shape AWS hands out). That makes them unique per domain and stable
/// for as long as the record lives, without any per-call randomness.
fn dkim_tokens(domain: &str) -> Vec<String> {
    (0..DKIM_TOKEN_COUNT)
        .map(|index| {
            let mut hasher = Sha256::new();
            hasher.update(domain.as_bytes());
            hasher.update(b"#dkim#");
            hasher.update(index.to_string().as_bytes());
            hex::encode(&hasher.finalize()[..16])
        })
        .collect()
}

/// One stored configuration set — what `CreateConfigurationSet` persists and
/// `GetConfigurationSet` reports.
#[derive(Debug, Clone, Default)]
pub struct ConfigurationSetRecord {
    /// The set's name; also its key in the store.
    pub name: String,
    pub tracking_options: Option<TrackingOptions>,
    pub delivery_options: Option<DeliveryOptions>,
    pub reputation_options: Option<ReputationOptions>,
    pub sending_options: Option<SendingOptions>,
    pub tags: Vec<Tag>,
    /// Event destinations attached to this set, keyed by `EventDestinationName`. The
    /// `*ConfigurationSetEventDestination` operations that fill it are still scaffolded.
    pub event_destinations: BTreeMap<String, EventDestinationDefinition>,
}

/// One message `SendEmail` accepted. The log is what makes a minted `MessageId` the id of
/// something that exists, and backs the account-level send accounting `GetAccount` will
/// report once it is implemented.
///
/// Every field is either emulator-minted or a BOUNDED summary of the request: the
/// recipients, tags and body are kept as counts and sizes, and the addresses and subject
/// as a bounded prefix ([`retained_address`], [`retained_subject`]), so one request cannot
/// decide how much memory its entry occupies. Nothing delivers mail here, so no caller can
/// read a message back and the full payload has no reader.
#[derive(Debug, Clone)]
pub struct SentMessageRecord {
    pub message_id: String,
    pub from_email_address: Option<String>,
    /// How many `ToAddresses` + `CcAddresses` + `BccAddresses` recipients the send named.
    pub recipient_count: usize,
    /// Which `EmailContent` member the send carried: `SIMPLE_TEXT`, `SIMPLE_HTML`,
    /// `SIMPLE` (a subject with no body part), `RAW` or `TEMPLATE`.
    pub content_form: String,
    pub content_subject: Option<String>,
    /// Size in bytes of the body the request carried; the body itself is not retained.
    pub content_body_bytes: usize,
    pub reply_to_address_count: usize,
    pub email_tag_count: usize,
    pub configuration_set_name: Option<String>,
    pub sent_at: SystemTime,
}

/// Per-service state for com-amazonaws-pinpointemail-amazonpinpointemailservice. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsPinpointemailAmazonpinpointemailserviceState {
    /// Email identities, keyed by `EmailIdentity`. At most [`MAX_EMAIL_IDENTITIES`].
    pub identities: BTreeMap<String, EmailIdentityRecord>,
    /// Configuration sets, keyed by `ConfigurationSetName`. At most
    /// [`MAX_CONFIGURATION_SETS`].
    pub configuration_sets: BTreeMap<String, ConfigurationSetRecord>,
    /// The most recent accepted messages, oldest first, at most
    /// [`SENT_MESSAGE_LOG_LIMIT`] of them.
    pub sent_messages: VecDeque<SentMessageRecord>,
    /// How many message ids have been minted. Monotone and never reset, so two sends can
    /// never be handed the same `MessageId` even though the log drops its oldest entries.
    pub minted_message_ids: u64,
}

impl ComAmazonawsPinpointemailAmazonpinpointemailserviceState {
    /// Apply every identity verification the clock has made due. A reporting operation
    /// over the whole collection calls this before it reads — see
    /// [`EmailIdentityRecord::materialize_due_verification`].
    pub fn materialize_due_verifications(&mut self, now: SystemTime) {
        for record in self.identities.values_mut() {
            record.materialize_due_verification(now);
        }
    }

    /// Append one accepted message to the log, dropping the oldest entry once the log
    /// holds [`SENT_MESSAGE_LOG_LIMIT`] of them.
    pub fn record_sent_message(&mut self, record: SentMessageRecord) {
        self.sent_messages.push_back(record);
        while self.sent_messages.len() > SENT_MESSAGE_LOG_LIMIT {
            self.sent_messages.pop_front();
        }
    }

    /// Mint the next `MessageId`: an opaque, never-repeated id.
    ///
    /// `OutboundMessageId` is an unconstrained string whose value AWS does not document,
    /// so the only requirements are opacity and uniqueness. Uniqueness comes from the
    /// monotone counter, which is advanced here; the timestamp prefix only makes the id
    /// sort by send time the way a real one does.
    pub fn mint_message_id(&mut self, sent_at: SystemTime) -> String {
        self.minted_message_ids += 1;
        let micros = sent_at
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_micros();
        format!("0100{micros:012x}-{:012x}-000000", self.minted_message_ids)
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsPinpointemailAmazonpinpointemailserviceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsPinpointemailAmazonpinpointemailserviceState>)
}
