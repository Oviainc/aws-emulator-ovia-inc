// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-sesv2-simpleemailservice-v2 service.
//!
//! Everything the implemented operations answer with is computed from the records
//! below: the email-identity table, the contact-list table, the outbound-message log,
//! the per-account send counters and the account-level attributes. Nothing here is
//! per-request scratch — the whole struct lives in the one shared engine (see
//! [`service_state`]).

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

/// Tenants the account may hold.
///
/// CHOSEN for the same reason as [`MAX_CONTACT_LISTS`]: one record per accepted
/// `CreateTenant`, so a long-lived emulator needs a ceiling. `CreateTenant`
/// refuses past it with the `LimitExceededException` its model binds.
pub const MAX_TENANTS: usize = 1_000;

/// The longest `TenantName` SES accepts: its documentation states the name "can
/// contain up to 64 alphanumeric characters, including letters, numbers, hyphens
/// (-) and underscores (_) only". The model itself binds only the lower bound
/// (length 1..), so the upper bound and the character set are enforced by the
/// handler.
pub const MAX_TENANT_NAME_CHARS: usize = 64;

/// The ARN namespace this service stamps on a resource ARN it mints: SES's ARN
/// namespace is `ses`.
pub const ARN_NAMESPACE: &str = "ses";

/// The region an ARN carries when the request carries no SigV4 credential scope
/// naming one.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The `SendingStatus` a freshly created tenant carries: sending is allowed for a
/// new tenant, and `ENABLED` is terminal — the clock owes no transition out of it.
pub const TENANT_SENDING_STATUS_CREATED: &str = "ENABLED";

/// The `SuppressionListScope` a tenant gets when the request names none:
/// `TenantSuppressionAttributes$SuppressionScope` documents that "if you don't
/// specify a suppression scope, the tenant defaults to ACCOUNT scope and uses the
/// account-level suppression list".
pub const DEFAULT_SUPPRESSION_SCOPE: &str = "ACCOUNT";

/// The region to stamp into the ARNs a request mints: the SigV4 credential
/// scope's region, else [`DEFAULT_REGION`].
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// `arn:aws:ses:<region>:<account>:tenant/<name>/<id>` — the ARN SES reports for a
/// tenant: the resource part names the tenant AND its id, so the ARN identifies
/// the tenant record even after a tenant of the same name is recreated. Minted
/// once at create and read back from the record afterwards.
pub fn tenant_arn(region: &str, tenant_name: &str, tenant_id: &str) -> String {
    ::emulator_server::emulator_arn(
        ARN_NAMESPACE,
        region,
        &format!("tenant/{tenant_name}/{tenant_id}"),
    )
}

/// Whether `tenant_name` satisfies the documented `TenantName` form: at most
/// [`MAX_TENANT_NAME_CHARS`] characters, each a letter, a digit, `-` or `_`.
pub fn tenant_name_is_well_formed(tenant_name: &str) -> bool {
    let length = tenant_name.chars().count();
    length > 0
        && length <= MAX_TENANT_NAME_CHARS
        && tenant_name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// How this service words `BadRequestException` for a `TenantName` that does not
/// satisfy the documented form, stated once for every operation that takes one.
pub fn invalid_tenant_name(tenant_name: &str) -> String {
    format!(
        "Invalid tenant name: {tenant_name}. The name can contain up to \
         {MAX_TENANT_NAME_CHARS} alphanumeric characters, including letters, \
         numbers, hyphens (-) and underscores (_) only."
    )
}

/// How this service words `NotFoundException` for a tenant it does not hold.
///
/// One wording for every operation that takes a `TenantName`, so a client sees the
/// same sentence whichever call first tells it the tenant is gone.
pub fn tenant_not_found(tenant_name: &str) -> String {
    format!("Tenant {tenant_name} does not exist.")
}

/// How this service words `AlreadyExistsException` for a tenant name already taken.
pub fn tenant_already_exists(tenant_name: &str) -> String {
    format!("Tenant with name: {tenant_name} already exists.")
}

/// Configuration sets the account may hold: SES's documented quota is 10,000
/// configuration sets per account per Region, and `CreateConfigurationSet` refuses
/// past it with the `LimitExceededException` its model binds.
pub const MAX_CONFIGURATION_SETS: usize = 10_000;

/// The longest a `ConfigurationSetName` may be, from the member's own
/// documentation: "The name can contain up to 64 alphanumeric characters, including
/// letters, numbers, hyphens (-) and underscores (_) only."
pub const MAX_CONFIGURATION_SET_NAME_LEN: usize = 64;

/// Per-service state for com-amazonaws-sesv2-simpleemailservice-v2. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsSesv2SimpleemailserviceV2State {
    /// Email identities, keyed by [`identity_key`] of the `EmailIdentity` they
    /// were created with. Ordered, so `ListEmailIdentities` enumerates and pages
    /// deterministically.
    pub identities: BTreeMap<String, EmailIdentityRecord>,
    /// Tenants, keyed by the `TenantName` they were created with — an opaque name
    /// the API echoes back as given, so it is keyed exactly as supplied. Ordered,
    /// so `ListTenants` enumerates and pages deterministically.
    pub tenants: BTreeMap<String, TenantRecord>,
    /// Monotonic counter feeding [`Self::next_tenant_id`], so two tenants created
    /// in the same millisecond still get different ids.
    pub tenant_sequence: u64,
    /// Contact lists, keyed by the `ContactListName` they were created with — an
    /// opaque name the API echoes back as given, so it is keyed exactly as supplied.
    /// Ordered, so `ListContactLists` enumerates and pages deterministically.
    pub contact_lists: BTreeMap<String, ContactListRecord>,
    /// Configuration sets, keyed by the `ConfigurationSetName` they were created
    /// with. The name is an identifier the API echoes back as given and compares
    /// exactly, so it is keyed exactly as supplied. Ordered, so
    /// `ListConfigurationSets` enumerates and pages deterministically.
    pub configuration_sets: BTreeMap<String, ConfigurationSetRecord>,
    /// The outbound-message log, oldest first: one entry per accepted `SendEmail`.
    pub sent_messages: Vec<SentMessage>,
    /// Per-account send counters, bumped by `SendEmail`.
    pub send_counters: SendQuotaState,
    /// The account-level attributes of this account in this Region: the sending
    /// and dedicated-IP switches, the reputation status, the suppression, VDM and
    /// pricing attributes and the account details, as the account operations left
    /// them.
    pub account: AccountState,
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

    /// Mint the next `TenantId`.
    ///
    /// `TenantId` is documented only as "a unique identifier for the tenant" and
    /// carries no modelled format, so the one guarantee this makes is that
    /// documented one: uniqueness is structural, because the monotonic sequence
    /// number is part of the id, so two tenants created in the same millisecond
    /// still differ. The id is minted once, at create, and read back from the
    /// record afterwards.
    pub fn next_tenant_id(&mut self, now: SystemTime) -> String {
        self.tenant_sequence += 1;
        let millis = now
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        let sequence = self.tenant_sequence;
        format!("{millis:016x}{sequence:016x}")
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

/// One tenant: the logical container SES groups resources, reputation and sending
/// status under.
#[derive(Debug, Clone)]
pub struct TenantRecord {
    /// The `TenantName` this record is keyed by, as the request spelled it.
    pub name: String,
    /// `TenantId`, minted once by [`ComAmazonawsSesv2SimpleemailserviceV2State::next_tenant_id`].
    pub tenant_id: String,
    /// `TenantArn`, minted once from the request's region by [`tenant_arn`].
    pub arn: String,
    /// When the tenant was created.
    pub created_at: SystemTime,
    /// Tags, in the order they were supplied.
    pub tags: Vec<(String, String)>,
    /// `SendingStatus`.
    pub sending_status: String,
    /// `TenantSuppressionAttributes$SuppressedReasons`: the reasons an address is
    /// added to this tenant's suppression list, in the order supplied.
    pub suppressed_reasons: Vec<String>,
    /// `TenantSuppressionAttributes$SuppressionScope`.
    pub suppression_scope: String,
}

impl TenantRecord {
    /// A tenant as `CreateTenant` stores it: sending enabled, and the
    /// account-level suppression list until a caller asks for a tenant-level one.
    pub fn new(name: &str, tenant_id: String, arn: String, now: SystemTime) -> Self {
        Self {
            name: name.to_string(),
            tenant_id,
            arn,
            created_at: now,
            tags: Vec::new(),
            sending_status: TENANT_SENDING_STATUS_CREATED.to_string(),
            suppressed_reasons: Vec::new(),
            suppression_scope: DEFAULT_SUPPRESSION_SCOPE.to_string(),
        }
    }

    /// The modelled `TenantSuppressionAttributes` view of this record, shared by
    /// every operation that reports a tenant's suppression preferences, so a write
    /// and the read after it cannot disagree.
    pub fn suppression_attributes(&self) -> crate::ops::types::TenantSuppressionAttributes {
        crate::ops::types::TenantSuppressionAttributes {
            // A tenant that adds nothing automatically has no reason set at all,
            // rather than an empty one.
            suppressed_reasons: (!self.suppressed_reasons.is_empty())
                .then(|| self.suppressed_reasons.clone()),
            suppression_scope: Some(self.suppression_scope.clone()),
        }
    }

    /// The modelled `Tenant` view of this record, as `GetTenant` reports it.
    pub fn view(&self) -> crate::ops::types::Tenant {
        crate::ops::types::Tenant {
            tenant_name: Some(self.name.clone()),
            tenant_id: Some(self.tenant_id.clone()),
            tenant_arn: Some(self.arn.clone()),
            created_timestamp: Some(::aws_smithy_types::DateTime::from(self.created_at)),
            tags: Some(self.tag_list()),
            sending_status: Some(self.sending_status.clone()),
            suppression_attributes: Some(self.suppression_attributes()),
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

/// One configuration set: the rules a send that names it is subject to.
///
/// Every member is the EFFECTIVE configuration, with the documented default already
/// resolved at create time, so a read reports what a send would be subject to rather
/// than re-deriving a default per reader.
#[derive(Debug, Clone)]
pub struct ConfigurationSetRecord {
    /// The `ConfigurationSetName` this record is keyed by, as the request spelled it.
    pub name: String,
    /// `TrackingOptions`: the custom open/click redirect domain, when the caller
    /// configured one. SES tracks through its own domain otherwise, so an
    /// unconfigured set holds nothing here.
    pub tracking_options: Option<crate::ops::types::TrackingOptions>,
    /// `DeliveryOptions$TlsPolicy`.
    pub tls_policy: String,
    /// `DeliveryOptions$MaxDeliverySeconds`, when the caller bounded delivery.
    pub max_delivery_seconds: Option<i64>,
    /// `ReputationOptions$ReputationMetricsEnabled`.
    pub reputation_metrics_enabled: bool,
    /// `SendingOptions$SendingEnabled`.
    pub sending_enabled: bool,
    /// `SuppressionOptions`, as supplied: a set that configures none falls back to
    /// the tenant or account suppression preferences, so nothing is stored.
    pub suppression_options: Option<crate::ops::types::SuppressionOptions>,
    /// `VdmOptions`, as supplied.
    pub vdm_options: Option<crate::ops::types::VdmOptions>,
    /// `MessageSecurityOptions`, as supplied.
    pub message_security_options: Option<crate::ops::types::MessageSecurityOptions>,
    /// Tags, in the order they were supplied.
    pub tags: Vec<(String, String)>,
    /// When the set was created.
    pub created_at: SystemTime,
    /// When the set's reputation metrics were last given a fresh start.
    ///
    /// `ReputationOptions$LastFreshStart` is "the date and time when the reputation
    /// metrics were last given a fresh start", and SES documents that where sending
    /// has never been disabled and re-enabled this is the time the configuration set
    /// was created \u2014 so it starts at the create instant and moves only when a fresh
    /// start happens.
    pub last_fresh_start: SystemTime,
}

impl ConfigurationSetRecord {
    /// A set as `CreateConfigurationSet` stores it when the request configures
    /// nothing: SES's own defaults.
    pub fn new(name: &str, now: SystemTime) -> Self {
        Self {
            name: name.to_string(),
            tracking_options: None,
            // `TlsPolicy`: "If the value is Require, messages are only delivered if
            // a TLS connection can be established. If the value is Optional,
            // messages can be delivered in plain text if a TLS connection can't be
            // established." SES delivers opportunistically unless a caller asks for
            // the stricter policy, so a set that configures none is Optional.
            tls_policy: TLS_POLICY_OPTIONAL.to_string(),
            max_delivery_seconds: None,
            // The member's modelled default.
            reputation_metrics_enabled: false,
            // Sending through a configuration set is something a caller PAUSES
            // (`PutConfigurationSetSendingOptions` "enable or disable email sending
            // for messages that use this configuration set"), so a newly created set
            // does not itself hold sending back.
            sending_enabled: true,
            suppression_options: None,
            vdm_options: None,
            message_security_options: None,
            tags: Vec::new(),
            created_at: now,
            last_fresh_start: now,
        }
    }

    /// The modelled `DeliveryOptions` view of this record.
    ///
    /// One projection shared by every operation that reports a set's delivery
    /// configuration, so a create and the read after it cannot disagree.
    pub fn delivery_options(&self) -> crate::ops::types::DeliveryOptions {
        crate::ops::types::DeliveryOptions {
            tls_policy: Some(self.tls_policy.clone()),
            // The dedicated IP pool a set sends from is a resource no implemented
            // operation of this service keeps, so no set holds one.
            sending_pool_name: None,
            max_delivery_seconds: self.max_delivery_seconds,
        }
    }

    /// The modelled `ReputationOptions` view of this record.
    pub fn reputation_options(&self) -> crate::ops::types::ReputationOptions {
        crate::ops::types::ReputationOptions {
            reputation_metrics_enabled: self.reputation_metrics_enabled,
            last_fresh_start: Some(::aws_smithy_types::DateTime::from(self.last_fresh_start)),
        }
    }

    /// The modelled `SendingOptions` view of this record.
    pub fn sending_options(&self) -> crate::ops::types::SendingOptions {
        crate::ops::types::SendingOptions {
            sending_enabled: self.sending_enabled,
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

/// The `TlsPolicy` value a configuration set holds when the caller configured none.
pub const TLS_POLICY_OPTIONAL: &str = "OPTIONAL";

/// Whether `name` is a `ConfigurationSetName` SES accepts.
///
/// From the member's documentation: "The name can contain up to 64 alphanumeric
/// characters, including letters, numbers, hyphens (-) and underscores (_) only" —
/// so a name is a non-empty run of at most [`MAX_CONFIGURATION_SET_NAME_LEN`] ASCII
/// letters, digits, hyphens and underscores.
pub fn configuration_set_name_is_valid(name: &str) -> bool {
    !name.is_empty()
        && name.chars().count() <= MAX_CONFIGURATION_SET_NAME_LEN
        && name
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
}

/// How this service words the refusal of a `ConfigurationSetName` outside the
/// documented character and length rule, stated once for every operation that takes
/// one.
pub fn configuration_set_name_invalid(configuration_set_name: &str) -> String {
    format!(
        "Configuration set name {configuration_set_name} is invalid. The name can contain up to \
         {MAX_CONFIGURATION_SET_NAME_LEN} alphanumeric characters, including letters, numbers, \
         hyphens (-) and underscores (_) only."
    )
}

/// How this service words `NotFoundException` for a configuration set it does not
/// hold.
///
/// One wording for every operation that takes a `ConfigurationSetName`, so a client
/// sees the same sentence whichever call first tells it the set is not there.
pub fn configuration_set_not_found(configuration_set_name: &str) -> String {
    format!("Configuration set {configuration_set_name} does not exist.")
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

impl SendQuotaState {
    /// The modelled `SendQuota` view of these counters, as the operations that
    /// report the account's sending limits answer it.
    pub fn send_quota(&self) -> crate::ops::types::SendQuota {
        crate::ops::types::SendQuota {
            max24_hour_send: self.max_24h,
            max_send_rate: self.max_send_rate,
            sent_last24_hours: self.sent_last_24h,
        }
    }
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

/// The reputation status of an account nothing has acted against.
///
/// `GetAccount`'s `EnforcementStatus` documents it as "HEALTHY – There are no
/// reputation-related issues that currently impact your account", which is the
/// status of an account whose sending nobody has reviewed or paused.
pub const ENFORCEMENT_STATUS_HEALTHY: &str = "HEALTHY";

/// `FeatureStatus`'s value for a feature that is on, as the model spells it (see
/// `crate::ops::types::FEATURE_STATUS_VALUES`).
pub const FEATURE_ENABLED: &str = "ENABLED";

/// `FeatureStatus`'s value for a feature that is off.
pub const FEATURE_DISABLED: &str = "DISABLED";

/// `PricingPlan`'s value for an account on no paid plan, one of the four values
/// `PutAccountPricingAttributesRequest$Plan` documents.
pub const PRICING_PLAN_NONE: &str = "NONE";

/// The account-level attributes of one SES account in one Region.
///
/// A single record rather than a table: every member of it is per account and
/// Region, and the emulator serves one account in one Region, so the account
/// operations all read and write this one record.
#[derive(Debug, Clone)]
pub struct AccountState {
    /// `SendingEnabled`: whether email sending is enabled for the account.
    pub sending_enabled: bool,
    /// `DedicatedIpAutoWarmupEnabled`: whether dedicated IPs associated with the
    /// account are warmed up automatically.
    pub dedicated_ip_auto_warmup_enabled: bool,
    /// `ProductionAccessEnabled`: false while the account is in the sandbox, where
    /// it may only send to verified identities.
    pub production_access_enabled: bool,
    /// `EnforcementStatus`: the account's reputation status.
    pub enforcement_status: String,
    /// `SuppressionAttributes$SuppressedReasons`: the reasons for which an address
    /// is added to the account-level suppression list automatically.
    pub suppressed_reasons: Vec<String>,
    /// `SuppressionAttributes$ValidationAttributes`, once a request has set them.
    pub suppression_validation: Option<crate::ops::types::SuppressionValidationAttributes>,
    /// `VdmAttributes$VdmEnabled`: the status of the account's VDM configuration.
    pub vdm_enabled: String,
    /// `DashboardAttributes$EngagementMetrics`, once a request has set it.
    pub vdm_engagement_metrics: Option<String>,
    /// `GuardianAttributes$OptimizedSharedDelivery`, once a request has set it.
    pub vdm_optimized_shared_delivery: Option<String>,
    /// `PricingAttributes$CurrentPlan`: the plan currently active on the account.
    pub pricing_plan: String,
    /// `PricingAttributes$NextPlan`: the plan a scheduled change will activate.
    /// `NextPlan` "is empty when no scheduled change is pending", so it is absent
    /// until a change is scheduled.
    pub next_pricing_plan: Option<String>,
    /// `Details`: the account details a request supplied, absent until one does.
    pub details: Option<crate::ops::types::AccountDetails>,
}

impl Default for AccountState {
    /// The attributes of an account nobody has configured yet.
    ///
    /// Email sending and dedicated-IP auto warm-up are on — SES turns both on for
    /// an account and the account operations are what switch them off — the account
    /// is in the sandbox (which is what the sandbox [`SendQuotaState::default`]
    /// limits go with), its reputation is unenforced, and the account-level
    /// suppression list is on for both of its documented reasons. VDM is off until
    /// a request enables it, no pricing plan is active, and no account details have
    /// been supplied.
    fn default() -> Self {
        Self {
            sending_enabled: true,
            dedicated_ip_auto_warmup_enabled: true,
            production_access_enabled: false,
            enforcement_status: ENFORCEMENT_STATUS_HEALTHY.to_string(),
            suppressed_reasons: crate::ops::types::SUPPRESSION_LIST_REASON_VALUES
                .iter()
                .map(|reason| (*reason).to_string())
                .collect(),
            suppression_validation: None,
            vdm_enabled: FEATURE_DISABLED.to_string(),
            vdm_engagement_metrics: None,
            vdm_optimized_shared_delivery: None,
            pricing_plan: PRICING_PLAN_NONE.to_string(),
            next_pricing_plan: None,
            details: None,
        }
    }
}

impl AccountState {
    /// The modelled `SuppressionAttributes` view of this record.
    ///
    /// One projection shared by every operation that reports the account's
    /// suppression preferences, so a write and the read after it cannot disagree.
    pub fn suppression_attributes(&self) -> crate::ops::types::SuppressionAttributes {
        crate::ops::types::SuppressionAttributes {
            suppressed_reasons: Some(self.suppressed_reasons.clone()),
            validation_attributes: self.suppression_validation.clone(),
        }
    }

    /// The modelled `VdmAttributes` view of this record.
    ///
    /// The nested dashboard and guardian attributes carry one feature status each,
    /// and an attribute nothing has set is reported as absent rather than as a
    /// status this account does not hold.
    pub fn vdm_attributes(&self) -> crate::ops::types::VdmAttributes {
        crate::ops::types::VdmAttributes {
            vdm_enabled: self.vdm_enabled.clone(),
            dashboard_attributes: self.vdm_engagement_metrics.clone().map(|engagement_metrics| {
                crate::ops::types::DashboardAttributes {
                    engagement_metrics: Some(engagement_metrics),
                }
            }),
            guardian_attributes: self.vdm_optimized_shared_delivery.clone().map(
                |optimized_shared_delivery| crate::ops::types::GuardianAttributes {
                    optimized_shared_delivery: Some(optimized_shared_delivery),
                },
            ),
        }
    }

    /// The modelled `PricingAttributes` view of this record.
    pub fn pricing_attributes(&self) -> crate::ops::types::PricingAttributes {
        crate::ops::types::PricingAttributes {
            current_plan: Some(self.pricing_plan.clone()),
            next_plan: self.next_pricing_plan.clone(),
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
