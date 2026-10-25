// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-sesv2-simpleemailservice-v2 service.
//!
//! Everything the implemented operations answer with is computed from the records
//! below: the email-identity table, the contact-list table, the outbound-message log
//! and the per-account send counters. Nothing here is per-request scratch — the whole
//! struct lives in the one shared engine (see [`service_state`]).

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::time::{Duration, SystemTime};

/// How long after `CreateEmailIdentity` an identity's `PENDING` -> `SUCCESS`
/// verification transition becomes due.
///
/// Real SES completes verification only once it observes something it cannot
/// observe here (the recipient following the link in the verification email, or
/// the Easy DKIM CNAME records appearing in the domain's DNS), so the span is
/// CHOSEN, not documented: it is the emulator's convention for a transition the
/// clock owes, long enough that a caller which reads straight back still sees the
/// transient `PENDING` state and the state errors that go with it. The due instant
/// is stored on the record; nothing ever sleeps for it.
pub const VERIFICATION_SETTLE: Duration = Duration::from_millis(500);

/// Easy DKIM publishes three CNAME tokens for a domain identity.
///
/// From the `DkimAttributes$Tokens` documentation: "this object contains a set of
/// unique strings that you use to create a set of CNAME records that you add to the
/// DNS configuration for your domain".
const EASY_DKIM_TOKEN_COUNT: usize = 3;

/// Length of one Easy DKIM token, in characters.
const EASY_DKIM_TOKEN_LEN: usize = 32;

/// The window the account's sending quota is measured over: SES states the quota
/// as `Max24HourSend`, "the maximum number of emails you can send in a 24-hour
/// period".
pub const SEND_QUOTA_WINDOW: Duration = Duration::from_secs(24 * 60 * 60);

/// Verified identities an account may hold: SES's documented quota is 10,000
/// identities per account per Region.
pub const MAX_IDENTITIES: usize = 10_000;

/// Entries the outbound-message log retains, oldest evicted first.
///
/// A CHOSEN bound, not an AWS one: the log is what lets a caller read back what it
/// sent, and a long-lived emulator must not grow without limit while serving
/// requests. It is well above the number of messages the sending quota lets a
/// caller accept in one window.
pub const MAX_SENT_MESSAGES: usize = 1_000;

/// Characters of a message body the log retains, the rest truncated.
///
/// Also CHOSEN: the body is caller-sized (a raw MIME document has no modelled
/// length bound), so the retained log is bounded by entries times this.
pub const MAX_STORED_BODY_CHARS: usize = 8 * 1024;

/// Contact lists the account may hold.
///
/// A CHOSEN bound, not an AWS one: a contact list is created by a request, so a
/// long-lived emulator must not grow without limit while serving them. It is far
/// above what a client exercising the contact API needs, and `CreateContactList`
/// refuses past it with the `LimitExceededException` its model binds.
pub const MAX_CONTACT_LISTS: usize = 1_000;

/// Contacts one contact list may hold.
///
/// CHOSEN for the same reason, and for the same growth: one record per accepted
/// `CreateContact`.
pub const MAX_CONTACTS_PER_LIST: usize = 10_000;

/// Per-service state for com-amazonaws-sesv2-simpleemailservice-v2. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsSesv2SimpleemailserviceV2State {
    /// Email identities, keyed by [`identity_key`] of the `EmailIdentity` they
    /// were created with. Ordered, so `ListEmailIdentities` enumerates and pages
    /// deterministically.
    pub identities: BTreeMap<String, EmailIdentityRecord>,
    /// Contact lists, keyed by the `ContactListName` they were created with — an
    /// opaque name the API echoes back as given, so it is keyed exactly as supplied.
    /// Ordered, so `ListContactLists` enumerates and pages deterministically.
    pub contact_lists: BTreeMap<String, ContactListRecord>,
    /// The outbound-message log, oldest first: one entry per accepted `SendEmail`.
    pub sent_messages: Vec<SentMessage>,
    /// Per-account send counters, bumped by `SendEmail`.
    pub send_counters: SendQuotaState,
    /// Monotonic counter feeding [`Self::next_message_id`], so two sends in the
    /// same microsecond still get different message ids.
    pub message_sequence: u64,
}

impl ComAmazonawsSesv2SimpleemailserviceV2State {
    /// Materialise every verification transition the clock already owes at `now`.
    ///
    /// A reporting operation may call this: the transition was due at `now` whether
    /// or not anybody called, so applying it changes nothing a later caller would
    /// otherwise have seen.
    pub fn settle(&mut self, now: SystemTime) {
        for identity in self.identities.values_mut() {
            identity.settle(now);
        }
        // `sent_last_24h` is a view of the log over the trailing window, not an
        // independent tally: recompute it here so it cannot drift as the window
        // moves past entries that no longer count.
        let charged = self.recipients_in_quota_window(now);
        self.send_counters.sent_last_24h = charged;
    }

    /// The identity that authorises sending from `address`, if any.
    ///
    /// An address identity authorises itself; a domain identity authorises every
    /// address in that domain and in any of its subdomains. The comparison runs on
    /// [`identity_key`], so the domain part matches without regard to case.
    pub fn sending_identity(&self, address: &str) -> Option<&EmailIdentityRecord> {
        let key = identity_key(address);
        if let Some(exact) = self.identities.get(&key) {
            return Some(exact);
        }
        let (_, domain) = key.rsplit_once('@')?;
        // Walk the domain and each of its parent domains: `a.b.example.com` is
        // covered by an identity for `a.b.example.com`, `b.example.com` or
        // `example.com`.
        let mut candidate = Some(domain);
        while let Some(suffix) = candidate {
            if let Some(found) = self
                .identities
                .get(suffix)
                .filter(|record| record.identity_kind == IdentityKind::Domain)
            {
                return Some(found);
            }
            candidate = suffix.split_once('.').map(|(_, parent)| parent);
        }
        None
    }

    /// Recipients charged against the sending quota within the trailing
    /// [`SEND_QUOTA_WINDOW`], derived from the log rather than from a counter that
    /// only ever grows, so the quota frees up as the window moves.
    pub fn recipients_in_quota_window(&self, now: SystemTime) -> f64 {
        let cutoff = now
            .checked_sub(SEND_QUOTA_WINDOW)
            .unwrap_or(SystemTime::UNIX_EPOCH);
        self.sent_messages
            .iter()
            .filter(|message| message.sent_at > cutoff)
            .map(|message| message.recipients().count() as f64)
            .sum()
    }

    /// Append an accepted message to the log, keeping it within
    /// [`MAX_SENT_MESSAGES`] by evicting the oldest entries.
    pub fn record_sent(&mut self, message: SentMessage) {
        self.sent_messages.push(message);
        if self.sent_messages.len() > MAX_SENT_MESSAGES {
            let overflow = self.sent_messages.len() - MAX_SENT_MESSAGES;
            self.sent_messages.drain(..overflow);
        }
    }

    /// The portion of a message body the log retains: at most
    /// [`MAX_STORED_BODY_CHARS`] characters.
    pub fn retained_body(body: &str) -> String {
        body.chars().take(MAX_STORED_BODY_CHARS).collect()
    }

    /// Mint the next outbound message id.
    ///
    /// `OutboundMessageId` carries no documented format, so the shape mirrors what
    /// SES hands back (`0100` + a timestamp + a body + a trailing field) while the
    /// only guarantee this makes is the documented one: the id is unique per
    /// accepted message. Uniqueness is structural — the monotonic sequence number
    /// is part of the id — so two sends in the same millisecond still differ.
    pub fn next_message_id(&mut self, now: SystemTime) -> String {
        self.message_sequence += 1;
        let millis = now
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        let sequence = self.message_sequence;
        format!("0100{millis:012x}-{sequence:016x}-000000")
    }
}

/// Whether an `EmailIdentity` names an address or a domain.
///
/// The model's `IdentityType` also carries `MANAGED_DOMAIN`, which its own
/// documentation says is "not supported for email identity types", so nothing here
/// produces it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdentityKind {
    /// `IdentityType` = `EMAIL_ADDRESS`.
    EmailAddress,
    /// `IdentityType` = `DOMAIN`.
    Domain,
}

impl IdentityKind {
    /// An `EmailIdentity` that carries a local part (`@`) is an address; anything
    /// else is a domain.
    pub fn of(identity: &str) -> Self {
        if identity.contains('@') {
            Self::EmailAddress
        } else {
            Self::Domain
        }
    }

    /// The modelled `IdentityType` value.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::EmailAddress => "EMAIL_ADDRESS",
            Self::Domain => "DOMAIN",
        }
    }
}

/// The value set shared by the model's `VerificationStatus` and `DkimStatus`
/// enums. Only the states this emulator can reach are represented.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SesStatus {
    /// The process has not been initiated for this identity.
    NotStarted,
    /// The process was initiated and has not completed.
    Pending,
    /// The process completed successfully.
    Success,
}

impl SesStatus {
    /// The modelled enum value.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NotStarted => "NOT_STARTED",
            Self::Pending => "PENDING",
            Self::Success => "SUCCESS",
        }
    }
}

/// How DKIM was configured for an identity (`DkimSigningAttributesOrigin`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DkimOrigin {
    /// Easy DKIM: SES generated the key pair and published the tokens.
    AwsSes,
    /// Bring Your Own DKIM: the caller supplied the selector and private key.
    External,
}

impl DkimOrigin {
    /// The modelled enum value.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::AwsSes => "AWS_SES",
            Self::External => "EXTERNAL",
        }
    }
}

/// The DKIM configuration stored for one identity.
#[derive(Debug, Clone)]
pub struct DkimState {
    /// Whether messages from the identity are DKIM-signed.
    pub signing_enabled: bool,
    /// The DKIM authentication status.
    pub status: SesStatus,
    /// Easy DKIM CNAME tokens, or the BYODKIM selector.
    pub tokens: Vec<String>,
    /// How DKIM was configured.
    pub origin: DkimOrigin,
    /// `[Easy DKIM]` the key length of the key pair in use, when SES owns the key.
    pub signing_key_length: Option<String>,
}

impl DkimState {
    /// Easy DKIM for a domain: SES owns the key pair, publishes three CNAME tokens
    /// and waits for them to appear in the domain's DNS.
    pub fn easy_dkim(domain: &str, key_length: Option<String>) -> Self {
        Self {
            signing_enabled: true,
            status: SesStatus::Pending,
            tokens: easy_dkim_tokens(domain),
            origin: DkimOrigin::AwsSes,
            // `DkimSigningKeyLength` has two values; SES generates 2048-bit keys
            // unless the request asks for the other one.
            signing_key_length: Some(key_length.unwrap_or_else(|| "RSA_2048_BIT".to_string())),
        }
    }

    /// BYODKIM for a domain: the caller owns the key pair, so the stored token set
    /// is the selector that names its public key and SES reports no key length.
    pub fn byo_dkim(selector: &str) -> Self {
        Self {
            signing_enabled: true,
            status: SesStatus::Pending,
            tokens: vec![selector.to_string()],
            origin: DkimOrigin::External,
            signing_key_length: None,
        }
    }

    /// An address identity: DKIM signing applies to domains, so nothing is
    /// configured and the authentication process was never initiated.
    pub fn not_started() -> Self {
        Self {
            signing_enabled: false,
            status: SesStatus::NotStarted,
            tokens: Vec::new(),
            origin: DkimOrigin::AwsSes,
            signing_key_length: None,
        }
    }
}

/// One email identity.
#[derive(Debug, Clone)]
pub struct EmailIdentityRecord {
    /// The `EmailIdentity` this record is keyed by.
    pub name: String,
    /// Address or domain.
    pub identity_kind: IdentityKind,
    /// `VerificationStatus`.
    pub verification_status: SesStatus,
    /// `VerifiedForSendingStatus` / `SendingEnabled`.
    pub verified_for_sending: bool,
    /// `FeedbackForwardingStatus`.
    pub feedback_forwarding: bool,
    /// The identity's DKIM configuration.
    pub dkim: DkimState,
    /// The configuration set used by default when sending from this identity.
    pub configuration_set_name: Option<String>,
    /// Tags, in the order they were supplied.
    pub tags: Vec<(String, String)>,
    /// Sending-authorization policies, keyed by policy name.
    pub policies: BTreeMap<String, String>,
    /// When the identity was created.
    pub created_at: SystemTime,
    /// When the `PENDING` -> `SUCCESS` verification transition falls due.
    pub verification_due_at: SystemTime,
}

impl EmailIdentityRecord {
    /// Start verifying `name`, as `CreateEmailIdentity` does. `kind` is the
    /// classification the caller already computed from [`addr_spec`], so the record
    /// and its creator cannot disagree about it.
    pub fn new(name: &str, kind: IdentityKind, dkim: DkimState, now: SystemTime) -> Self {
        Self {
            name: name.to_string(),
            identity_kind: kind,
            // "Starts the process of verifying an email identity": the identity is
            // not verified yet, so it cannot send yet either.
            verification_status: SesStatus::Pending,
            verified_for_sending: false,
            // SES forwards bounces and complaints to the identity's owner unless
            // the caller turns it off with PutEmailIdentityFeedbackAttributes.
            feedback_forwarding: true,
            dkim,
            configuration_set_name: None,
            tags: Vec::new(),
            policies: BTreeMap::new(),
            created_at: now,
            verification_due_at: now.checked_add(VERIFICATION_SETTLE).unwrap_or(now),
        }
    }

    /// The modelled `DkimAttributes` view of this record.
    ///
    /// One projection shared by every operation that reports DKIM state, so a
    /// create and the read after it cannot disagree.
    pub fn dkim_attributes(&self) -> crate::ops::types::DkimAttributes {
        crate::ops::types::DkimAttributes {
            signing_enabled: self.dkim.signing_enabled,
            status: Some(self.dkim.status.as_str().to_string()),
            // An identity with no DKIM configuration has no token set at all,
            // rather than an empty one.
            tokens: (!self.dkim.tokens.is_empty()).then(|| self.dkim.tokens.clone()),
            // SES reports the publishing hosted zone only for identities whose
            // records it publishes itself; nothing here configures one.
            signing_hosted_zone: None,
            signing_attributes_origin: Some(self.dkim.origin.as_str().to_string()),
            // `NextSigningKeyLength` describes a rotation that has been requested
            // and not yet taken effect; `PutEmailIdentityDkimSigningAttributes`
            // owns that, and nothing here requests one.
            next_signing_key_length: None,
            current_signing_key_length: self.dkim.signing_key_length.clone(),
            // SES generated the Easy DKIM key pair when it created the identity;
            // with BYODKIM the caller owns the key, so SES generated none.
            last_key_generation_timestamp: (self.dkim.origin == DkimOrigin::AwsSes
                && self.dkim.signing_enabled)
                .then(|| ::aws_smithy_types::DateTime::from(self.created_at)),
        }
    }

    /// The modelled `TagList` view of this record's tags.
    pub fn tag_list(&self) -> Vec<crate::ops::types::Tag> {
        self.tags
            .iter()
            .map(|(key, value)| crate::ops::types::Tag {
                key: key.clone(),
                value: value.clone(),
            })
            .collect()
    }

    /// Apply the verification transition if it is due at `now`.
    pub fn settle(&mut self, now: SystemTime) {
        if self.verification_status == SesStatus::Pending && now >= self.verification_due_at {
            self.verification_status = SesStatus::Success;
            self.verified_for_sending = true;
            if self.dkim.status == SesStatus::Pending {
                self.dkim.status = SesStatus::Success;
            }
        }
    }
}

/// One contact list: the topics it publishes, its description and tags, and the
/// contacts subscribed to it.
///
/// The contacts live ON the list rather than in a table beside it: a contact is
/// identified by its list and its address together (every contact operation binds
/// both as path labels), and deleting the list takes its contacts with it.
#[derive(Debug, Clone)]
pub struct ContactListRecord {
    /// The `ContactListName` this record is keyed by, as the request spelled it.
    pub name: String,
    /// `Topics`: the interest groups the list publishes, in the order supplied.
    pub topics: Vec<crate::ops::types::Topic>,
    /// `Description`.
    pub description: Option<String>,
    /// Tags, in the order they were supplied.
    pub tags: Vec<(String, String)>,
    /// When the list was created.
    pub created_at: SystemTime,
    /// When the list was last changed.
    pub last_updated_at: SystemTime,
    /// The contacts on this list, keyed by [`identity_key`] of their address so a
    /// listing enumerates and pages deterministically.
    pub contacts: BTreeMap<String, ContactRecord>,
}

impl ContactListRecord {
    /// A list as `CreateContactList` stores it.
    pub fn new(name: &str, now: SystemTime) -> Self {
        Self {
            name: name.to_string(),
            topics: Vec::new(),
            description: None,
            tags: Vec::new(),
            created_at: now,
            last_updated_at: now,
            contacts: BTreeMap::new(),
        }
    }

    /// Whether the list publishes a topic called `topic_name`.
    pub fn publishes_topic(&self, topic_name: &str) -> bool {
        self.topics.iter().any(|topic| topic.topic_name == topic_name)
    }

    /// The default preference the list's own topics give a contact that has stated
    /// none: one `TopicPreference` per topic, carrying that topic's
    /// `DefaultSubscriptionStatus`.
    ///
    /// `Contact$TopicDefaultPreferences` is "the default topic preferences applied to
    /// the contact", which is exactly this projection of the list's topics, so every
    /// operation that reports it computes it here.
    pub fn topic_default_preferences(&self) -> Vec<crate::ops::types::TopicPreference> {
        self.topics
            .iter()
            .map(|topic| crate::ops::types::TopicPreference {
                topic_name: topic.topic_name.clone(),
                subscription_status: topic.default_subscription_status.clone(),
            })
            .collect()
    }

    /// The modelled `ContactList` view of this record, as `ListContactLists` reports
    /// it.
    pub fn summary(&self) -> crate::ops::types::ContactList {
        crate::ops::types::ContactList {
            contact_list_name: Some(self.name.clone()),
            last_updated_timestamp: Some(::aws_smithy_types::DateTime::from(self.last_updated_at)),
        }
    }

    /// The modelled `TagList` view of this record's tags.
    pub fn tag_list(&self) -> Vec<crate::ops::types::Tag> {
        self.tags
            .iter()
            .map(|(key, value)| crate::ops::types::Tag {
                key: key.clone(),
                value: value.clone(),
            })
            .collect()
    }
}

/// One contact on a contact list.
#[derive(Debug, Clone)]
pub struct ContactRecord {
    /// The `EmailAddress` this contact was created with, as the request spelled it.
    pub email_address: String,
    /// `TopicPreferences`: the preferences the contact has stated, in the order
    /// supplied. A topic the contact has stated no preference for is answered from
    /// the list's own default instead
    /// ([`ContactListRecord::topic_default_preferences`]).
    pub topic_preferences: Vec<crate::ops::types::TopicPreference>,
    /// `UnsubscribeAll`: the contact is unsubscribed from every topic of the list.
    pub unsubscribe_all: bool,
    /// `AttributesData`: the caller's own JSON document about this contact, kept as
    /// supplied.
    pub attributes_data: Option<String>,
    /// When the contact was created.
    pub created_at: SystemTime,
    /// When the contact was last changed.
    pub last_updated_at: SystemTime,
}

impl ContactRecord {
    /// A contact as `CreateContact` stores it.
    pub fn new(email_address: &str, now: SystemTime) -> Self {
        Self {
            email_address: email_address.to_string(),
            topic_preferences: Vec::new(),
            unsubscribe_all: false,
            attributes_data: None,
            created_at: now,
            last_updated_at: now,
        }
    }

    /// The contact's preference for `topic_name`, when it has stated one.
    pub fn stated_preference(&self, topic_name: &str) -> Option<&str> {
        self.topic_preferences
            .iter()
            .find(|preference| preference.topic_name == topic_name)
            .map(|preference| preference.subscription_status.as_str())
    }

    /// The modelled `Contact` view of this record, as `ListContacts` reports it.
    ///
    /// `list` is the list this contact is on, which supplies the default preferences.
    pub fn summary(&self, list: &ContactListRecord) -> crate::ops::types::Contact {
        crate::ops::types::Contact {
            email_address: Some(self.email_address.clone()),
            topic_preferences: Some(self.topic_preferences.clone()),
            topic_default_preferences: Some(list.topic_default_preferences()),
            unsubscribe_all: self.unsubscribe_all,
            last_updated_timestamp: Some(::aws_smithy_types::DateTime::from(self.last_updated_at)),
        }
    }
}

/// How this service words `NotFoundException` for a contact list it does not hold.
///
/// One wording for every operation that takes a `ContactListName` — the reads, the
/// writes and the contact operations nested under the list — so a client sees the same
/// sentence whichever call first tells it the list is gone.
pub fn contact_list_not_found(contact_list_name: &str) -> String {
    format!("List with name: {contact_list_name} doesn't exist.")
}

/// How this service words `NotFoundException` for a contact the named list does not
/// hold, stated once for every contact operation.
pub fn contact_not_found(contact_list_name: &str, email_address: &str) -> String {
    format!(
        "Contact with email address: {email_address} doesn't exist in list: \
         {contact_list_name}."
    )
}

/// One accepted outbound message.
#[derive(Debug, Clone)]
pub struct SentMessage {
    /// The `MessageId` returned to the caller.
    pub message_id: String,
    /// The `From` address the message was accepted for.
    pub from: Option<String>,
    /// `Destination$ToAddresses`.
    pub to: Vec<String>,
    /// `Destination$CcAddresses`.
    pub cc: Vec<String>,
    /// `Destination$BccAddresses`.
    pub bcc: Vec<String>,
    /// The subject, for a simple message.
    pub subject: Option<String>,
    /// The body: the text part of a simple message, or the raw MIME data.
    pub body: Option<String>,
    /// The HTML part of a simple message, when the request carried one. A simple
    /// message may carry both parts, and the log records what was accepted.
    pub body_html: Option<String>,
    /// The configuration set the message was sent with.
    pub configuration_set_name: Option<String>,
    /// `ReplyToAddresses`.
    pub reply_to: Vec<String>,
    /// `FeedbackForwardingEmailAddress`: where the caller asked for bounce and
    /// complaint notifications. Nothing here delivers a bounce, so it is recorded
    /// as part of what was accepted rather than acted on.
    pub feedback_forwarding: Option<String>,
    /// `EmailTags`, in the order they were supplied.
    pub email_tags: Vec<(String, String)>,
    /// When the message was accepted.
    pub sent_at: SystemTime,
}

impl SentMessage {
    /// Every recipient of the message, in `To`, `Cc`, `Bcc` order. SES counts one
    /// recipient as one message against the account's sending quota.
    pub fn recipients(&self) -> impl Iterator<Item = &String> {
        self.to.iter().chain(self.cc.iter()).chain(self.bcc.iter())
    }
}

/// Per-account send counters.
#[derive(Debug, Clone, Copy)]
pub struct SendQuotaState {
    /// Recipients accepted within the trailing [`SEND_QUOTA_WINDOW`]. A view of
    /// the state's outbound-message log, recomputed by
    /// [`ComAmazonawsSesv2SimpleemailserviceV2State::settle`] rather than tallied,
    /// so it frees up as the window moves.
    pub sent_last_24h: f64,
    /// The account's 24-hour sending quota.
    pub max_24h: f64,
    /// The account's maximum send rate, in messages per second. Recorded as part
    /// of the account's limits for the reporting operations that answer it
    /// (`GetAccount`); no per-second throttle is applied anywhere in this slice.
    pub max_send_rate: f64,
}

impl Default for SendQuotaState {
    /// The limits a new account has in the SES sandbox: 200 messages per 24-hour
    /// period at 1 message per second.
    fn default() -> Self {
        Self {
            sent_last_24h: 0.0,
            max_24h: 200.0,
            max_send_rate: 1.0,
        }
    }
}

/// The addr-spec of a `From`-style address.
///
/// An email address member may carry RFC 5322's display-name form, `Sender Name
/// <sender@example.com>`, which names the same mailbox as the bare address: the
/// mailbox is what the identity table is keyed by, so the angle-bracketed
/// addr-spec is what a lookup compares. A value with no angle brackets is already
/// an addr-spec.
pub fn addr_spec(address: &str) -> &str {
    match (address.rfind('<'), address.rfind('>')) {
        (Some(open), Some(close)) if open < close => address[open + 1..close].trim(),
        _ => address.trim(),
    }
}

/// The key an identity is stored and looked up under.
///
/// A domain name is case-insensitive — DNS compares labels without regard to case
/// — so `Example.COM` and `example.com` name one identity, and an address is
/// authorised by the identity for its domain whatever case the request spells the
/// domain in. The local part of an address belongs to the receiving mailbox, so it
/// is kept exactly as supplied, as is the whole value on the record, which is what
/// every response reports.
pub fn identity_key(identity: &str) -> String {
    let identity = addr_spec(identity);
    match identity.rsplit_once('@') {
        Some((local, domain)) => format!("{local}@{}", domain.to_lowercase()),
        None => identity.to_lowercase(),
    }
}

/// The Easy DKIM CNAME tokens SES publishes for `domain`.
///
/// `DkimAttributes$Tokens` is documented as "a set of unique strings" the caller
/// turns into CNAME records, with no format of its own, so each token here is a
/// digest of the domain and the token's index: distinct per domain and per index,
/// and stable for as long as the identity holds it (the set is computed once, at
/// create, and then stored), which is what a client adding CNAME records depends
/// on. Two 64-bit digests make up the [`EASY_DKIM_TOKEN_LEN`] hex characters.
///
/// The digest is `std`'s, whose output is explicitly NOT guaranteed to be the same
/// across Rust releases — nothing here relies on that: a token set is computed once,
/// at create, and read back from the record afterwards, so no caller and no test may
/// assert a literal token value.
pub fn easy_dkim_tokens(domain: &str) -> Vec<String> {
    (0..EASY_DKIM_TOKEN_COUNT)
        .map(|index| {
            let mut token = String::with_capacity(EASY_DKIM_TOKEN_LEN);
            for salt in [0_u64, u64::from_be_bytes(*b"EasyDKIM")] {
                let mut hasher = DefaultHasher::new();
                salt.hash(&mut hasher);
                domain.hash(&mut hasher);
                index.hash(&mut hasher);
                write!(token, "{:016x}", hasher.finish())
                    .expect("writing to a String cannot fail");
            }
            token
        })
        .collect()
}

/// What a `NextToken` of this service's own making starts with, so a token from
/// anywhere else — including one that happens to be valid base64 — is detectably
/// not a position in this listing.
const PAGE_TOKEN_TAG: &str = "sesv2:email-identities:";

/// The tag `ListContactLists`'s own tokens carry.
pub const CONTACT_LISTS_PAGE_TOKEN_TAG: &str = "sesv2:contact-lists:";

/// The tag `ListContacts`'s own tokens carry.
pub const CONTACTS_PAGE_TOKEN_TAG: &str = "sesv2:contacts:";

/// Encode a `NextToken` that resumes the listing tagged `tag` after `last_returned`.
///
/// The token is opaque to the caller: it is the listing's tag and the last key
/// returned, base64'd — decodable by [`decode_tagged_page_token`] under the same tag
/// and by nothing else, so one listing's token is not a position in another's.
pub fn encode_tagged_page_token(tag: &str, last_returned: &str) -> String {
    ::aws_smithy_types::base64::encode(format!("{tag}{last_returned}"))
}

/// Decode a `NextToken` produced by [`encode_tagged_page_token`] under `tag`. `None`
/// when the caller made the token up, or when it belongs to another listing.
pub fn decode_tagged_page_token(tag: &str, token: &str) -> Option<String> {
    let decoded = String::from_utf8(::aws_smithy_types::base64::decode(token).ok()?).ok()?;
    decoded.strip_prefix(tag).map(ToString::to_string)
}

/// Encode a `ListEmailIdentities` `NextToken` that resumes after `last_returned`.
pub fn encode_page_token(last_returned: &str) -> String {
    encode_tagged_page_token(PAGE_TOKEN_TAG, last_returned)
}

/// Decode a `NextToken` produced by [`encode_page_token`]. `None` when the caller
/// made the token up.
pub fn decode_page_token(token: &str) -> Option<String> {
    decode_tagged_page_token(PAGE_TOKEN_TAG, token)
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// `None` means the engine is not the shared in-memory engine, so this service has
/// no store at all; a handler answers that with the scaffold's 501, never with a
/// fresh engine and never with a modelled error that would read as a real service
/// state (a throttle an SDK retries, say).
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsSesv2SimpleemailserviceV2State>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsSesv2SimpleemailserviceV2State>)
}
