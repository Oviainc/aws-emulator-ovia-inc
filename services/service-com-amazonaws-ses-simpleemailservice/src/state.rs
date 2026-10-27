// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-ses-simpleemailservice service.
//!
//! One store per emulator: the identity list SES keys every send against — each
//! identity carrying the sending-authorization policies it holds and where its
//! notifications go ([`NotificationSettings`]) — the send log the sending quota
//! is derived from, the account's sending limits, and the configuration sets a
//! send may name.
//!
//! Nothing here is a counter that a reader bumps: `SentLast24Hours` is COMPUTED
//! from the send log (see [`ComAmazonawsSesSimpleemailserviceState::sent_last_24_hours`]),
//! so it falls again as messages age out of the window instead of drifting.
//!
//! Every caller-driven store is BOUNDED, because every request reaching this
//! emulator can grow one of them: the identity list at [`MAX_IDENTITIES`]
//! entries, each identity's sending-authorization policies at
//! [`MAX_IDENTITY_POLICIES`] documents of [`MAX_POLICY_BYTES`], the configuration
//! sets at [`MAX_CONFIGURATION_SETS`], the receipt rule sets at
//! [`MAX_RECEIPT_RULE_SETS`], the receipt IP address filters at
//! [`MAX_RECEIPT_FILTERS`], and the send log
//! at the activity its longest report covers, with that retention's expiry
//! applied on append ([`ComAmazonawsSesSimpleemailserviceState::log_sent_message`])
//! and [`MAX_SENT_LOG_ENTRIES`] as the backstop. The text one log entry keeps is
//! capped at [`MAX_LOGGED_TEXT`]. The template store is bounded the same way, at
//! [`MAX_TEMPLATES`], and the custom verification email templates at
//! [`MAX_CUSTOM_VERIFICATION_EMAIL_TEMPLATES`].
//!
//! A listing this store serves a page at a time hands the caller an OPAQUE,
//! self-verifying cursor ([`encode_page_token`]), so a token this service never
//! issued is refused instead of being read as some arbitrary position.

use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// The trailing window the sending quota is measured over.
///
/// The model documents the window: `GetSendQuotaResponse$Max24HourSend` is "The
/// maximum number of emails the user is allowed to send in a 24-hour interval"
/// and `SentLast24Hours` is "The number of emails sent during the previous 24
/// hours".
pub const SEND_QUOTA_WINDOW: Duration = Duration::from_secs(24 * 60 * 60);

/// The trailing window the sending statistics are reported over.
///
/// `GetSendStatistics` documents it: "The result is a list of data points,
/// representing the last two weeks of sending activity."
pub const SEND_STATISTICS_WINDOW: Duration = Duration::from_secs(14 * 24 * 60 * 60);

/// How long an accepted message stays in the send log: the LONGEST window any
/// reader of the log reports over, which is the sending statistics' two weeks
/// ([`SEND_STATISTICS_WINDOW`]) rather than the sending quota's 24 hours. An
/// entry the quota has stopped counting is still part of the statistics, so the
/// log cannot drop it any earlier without making a documented report impossible
/// to serve.
pub const SEND_LOG_RETENTION: Duration = SEND_STATISTICS_WINDOW;

/// The period one sending-statistics data point aggregates.
///
/// `GetSendStatistics` documents it: "Each data point in the list contains
/// statistics for a 15-minute period of time", and `SendDataPoint` repeats it.
pub const SEND_STATISTICS_PERIOD: Duration = Duration::from_secs(15 * 60);

/// The account's daily sending quota in the Amazon SES sandbox, which is where a
/// fresh account (and therefore this emulator's account) starts.
///
/// Held in [`AccountSettings`] rather than returned as a literal, so later work
/// that raises the quota changes the stored account record and every reader
/// follows.
pub const SANDBOX_MAX_24_HOUR_SEND: f64 = 200.0;

/// The account's maximum send rate (messages per second) in the Amazon SES
/// sandbox. Stored, for the same reason as [`SANDBOX_MAX_24_HOUR_SEND`].
pub const SANDBOX_MAX_SEND_RATE: f64 = 1.0;

/// The identities one account may hold, and so the ceiling on the identity
/// store: SES documents a limit of 10,000 verified identities per account per
/// Region, and a verification past it is refused rather than letting caller
/// input grow this store without end.
pub const MAX_IDENTITIES: usize = 10_000;

/// The configuration sets one account may hold, and so the ceiling on the
/// configuration-set store: SES documents a limit of 10,000 configuration sets
/// per account per Region, and a create past it is refused rather than letting
/// caller input grow this store without end.
pub const MAX_CONFIGURATION_SETS: usize = 10_000;

/// The characters a configuration set name may be built from, and the longest
/// such name.
///
/// `ConfigurationSet$Name` states the rule: "Contain only letters (a-z, A-Z),
/// numbers (0-9), underscores (_), or dashes (-)" and "Contain 64 characters or
/// fewer".
pub const MAX_CONFIGURATION_SET_NAME: usize = 64;

/// The entries the send log may hold at once.
///
/// The account's own quota is the real ceiling — a day's accepted sends are at
/// most `max_24_hour_send` recipients, and [`SEND_LOG_RETENTION`] bounds how
/// many days of them the log keeps — and that retention's expiry is applied on
/// every append, so this is the backstop that still bounds the store if the
/// stored quota is raised later.
pub const MAX_SENT_LOG_ENTRIES: usize = 1_000;

/// The receipt rule sets one account may hold, and so the ceiling on the rule-set
/// store: SES documents a limit of 20 receipt rule sets per AWS account, and a
/// create or clone past it is refused with the modelled `LimitExceeded` rather
/// than letting caller input grow this store without end.
pub const MAX_RECEIPT_RULE_SETS: usize = 20;

/// The characters a receipt rule set name may carry.
///
/// From the model's own `ReceiptRuleSetName` documentation (e.g.
/// `CreateReceiptRuleSetRequest$RuleSetName`): the name must "Contain 64
/// characters or fewer".
pub const MAX_RECEIPT_RULE_SET_NAME: usize = 64;

/// The receipt IP address filters one account may hold, and so the ceiling on
/// the filter store: SES documents a limit of 100 receipt filters per AWS
/// account, and a create past it is refused with the modelled `LimitExceeded`
/// rather than letting caller input grow this store without end.
pub const MAX_RECEIPT_FILTERS: usize = 100;

/// The longest name a receipt IP address filter may carry.
///
/// `ReceiptFilter$Name` documents the same ceiling the rule and rule set names
/// carry ("Contain 64 characters or fewer"), so this is that constant rather
/// than a second copy of the number.
pub const MAX_RECEIPT_FILTER_NAME: usize = MAX_RECEIPT_RULE_SET_NAME;

/// The email templates one account may hold, and so the ceiling on the template
/// store: SES documents a limit of 10,000 email templates per account, and a
/// creation past it is refused rather than letting caller input grow this store
/// without end.
pub const MAX_TEMPLATES: usize = 10_000;

/// The custom verification email templates one account may hold, and so the
/// ceiling on that store: SES documents a limit of 50 custom verification email
/// templates per account per Region, and a creation past it is refused rather
/// than letting caller input grow this store without end.
pub const MAX_CUSTOM_VERIFICATION_EMAIL_TEMPLATES: usize = 50;

/// Whether the account a freshly booted emulator serves has been granted
/// production access.
///
/// A new Amazon SES account is in the sandbox, which is the same account state
/// the stored sandbox quota ([`SANDBOX_MAX_24_HOUR_SEND`]) already describes. No
/// operation of this service's model moves an account out of it — leaving the
/// sandbox is a separate, unmodelled account-review request — so nothing a
/// caller does changes this, and an operation that requires production access is
/// refused while [`AccountSettings::production_access_granted`] is false.
pub const PRODUCTION_ACCESS_AT_ACCOUNT_START: bool = false;

/// The bytes one sending-authorization policy document may carry.
///
/// `PutIdentityPolicyRequest$Policy` states the ceiling: "The text of the policy
/// in JSON format. The policy cannot exceed 4 KB." The model carries only the
/// member's lower bound (`length 1..`), so the documented ceiling is the
/// handler's to enforce — and it is also what bounds one entry of the policy
/// store.
pub const MAX_POLICY_BYTES: usize = 4 * 1024;

/// The sending-authorization policies ONE identity may hold at once, and so the
/// ceiling on an identity's policy map.
///
/// `GetIdentityPoliciesRequest$PolicyNames` is documented as "a list of the names
/// of policies to be retrieved. You can retrieve a maximum of 20 policies at a
/// time", so 20 policies is the whole set a caller can read back in one call —
/// the documented size of an identity's policy collection. A put past it is
/// refused rather than letting caller input grow this store without end.
pub const MAX_IDENTITY_POLICIES: usize = 20;

/// The characters of a message's subject and body the log keeps.
///
/// The log records THAT a message was accepted, with enough of it to identify
/// it; it is not a mailbox, and no operation serves the stored text back. A
/// message may be 10 MB, so a bounded prefix is kept rather than a whole body.
pub const MAX_LOGGED_TEXT: usize = 1024;

/// The class of an identity, as SES's `IdentityType` enum names it.
///
/// The wire spellings are the model's own enum values, published as
/// `crate::ops::types::IDENTITY_TYPE_VALUES`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdentityType {
    /// A single mailbox, e.g. the address `VerifyEmailIdentity` records.
    EmailAddress,
    /// A domain, which also covers the mailboxes under it and its subdomains.
    Domain,
}

impl IdentityType {
    /// The wire value for this class.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::EmailAddress => "EmailAddress",
            Self::Domain => "Domain",
        }
    }

    /// Parse a wire `IdentityType`; `None` for a value the model does not declare.
    pub fn from_wire(value: &str) -> Option<Self> {
        match value {
            "EmailAddress" => Some(Self::EmailAddress),
            "Domain" => Some(Self::Domain),
            _ => None,
        }
    }
}

/// The verification state of an identity, as SES's `VerificationStatus` enum
/// names it (wire spellings published as
/// `crate::ops::types::VERIFICATION_STATUS_VALUES`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VerificationStatus {
    /// Verification has started and has not completed.
    Pending,
    /// The identity is verified; SES accepts it as a sender.
    Success,
    /// Verification failed.
    Failed,
    /// Verification failed for a reason that may clear on its own.
    TemporaryFailure,
    /// Verification has not been attempted.
    NotStarted,
}

impl VerificationStatus {
    /// The wire value for this status.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Pending => "Pending",
            Self::Success => "Success",
            Self::Failed => "Failed",
            Self::TemporaryFailure => "TemporaryFailure",
            Self::NotStarted => "NotStarted",
        }
    }
}

/// The class of notification an identity publishes, as SES's `NotificationType`
/// enum names it (wire spellings published as
/// `crate::ops::types::NOTIFICATION_TYPE_VALUES`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotificationType {
    /// A message that could not be delivered.
    Bounce,
    /// A recipient's complaint about a delivered message.
    Complaint,
    /// A successful delivery.
    Delivery,
}

impl NotificationType {
    /// The wire value for this class.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Bounce => "Bounce",
            Self::Complaint => "Complaint",
            Self::Delivery => "Delivery",
        }
    }

    /// Parse a wire `NotificationType`; `None` for a value the model does not
    /// declare.
    ///
    /// The value set this accepts is pinned against the model's own published
    /// one: `tests/all/identity_notification_topics.rs` drives EVERY entry of
    /// `crate::ops::types::NOTIFICATION_TYPE_VALUES` through an operation that
    /// parses it here, and refuses values outside it, so a model that gained or
    /// renamed a value fails there rather than diverging quietly.
    pub fn from_wire(value: &str) -> Option<Self> {
        match value {
            "Bounce" => Some(Self::Bounce),
            "Complaint" => Some(Self::Complaint),
            "Delivery" => Some(Self::Delivery),
            _ => None,
        }
    }
}

/// Where one identity's notifications go, as
/// `GetIdentityNotificationAttributes` reports it.
///
/// A topic is `None` when none is set, which is what SES calls publishing
/// DISABLED for that class: `SetIdentityNotificationTopicRequest$SnsTopic` is
/// documented as "If the parameter is omitted from the request or a null value is
/// passed, `SnsTopic` is cleared and publishing is disabled."
#[derive(Debug, Clone)]
pub struct NotificationSettings {
    /// The topic bounce notifications are published to.
    pub bounce_topic: Option<String>,
    /// The topic complaint notifications are published to.
    pub complaint_topic: Option<String>,
    /// The topic delivery notifications are published to.
    pub delivery_topic: Option<String>,
    /// Whether bounce and complaint notifications are also forwarded as email
    /// (`SetIdentityFeedbackForwardingEnabled`).
    pub forwarding_enabled: bool,
    /// Whether the original email headers travel with a bounce notification
    /// (`SetIdentityHeadersInNotificationsEnabled`).
    pub headers_in_bounce_notifications_enabled: bool,
    /// The same, for a complaint notification.
    pub headers_in_complaint_notifications_enabled: bool,
    /// The same, for a delivery notification.
    pub headers_in_delivery_notifications_enabled: bool,
}

impl Default for NotificationSettings {
    /// A freshly recorded identity publishes nowhere and forwards feedback as
    /// email.
    ///
    /// `ForwardingEnabled` starts TRUE because SES cannot be asked for anything
    /// else yet: `SetIdentityFeedbackForwardingEnabledRequest$ForwardingEnabled`
    /// is documented as "This value can only be set to `false` when Amazon SNS
    /// topics are set for both `Bounce` and `Complaint` notification types", and
    /// an identity with no topics has neither. The header flags start false —
    /// the model's own `@default` for each of them.
    fn default() -> Self {
        Self {
            bounce_topic: None,
            complaint_topic: None,
            delivery_topic: None,
            forwarding_enabled: true,
            headers_in_bounce_notifications_enabled: false,
            headers_in_complaint_notifications_enabled: false,
            headers_in_delivery_notifications_enabled: false,
        }
    }
}

impl NotificationSettings {
    /// The topic `kind` notifications are published to, when one is set.
    pub fn topic(&self, kind: NotificationType) -> Option<&str> {
        match kind {
            NotificationType::Bounce => self.bounce_topic.as_deref(),
            NotificationType::Complaint => self.complaint_topic.as_deref(),
            NotificationType::Delivery => self.delivery_topic.as_deref(),
        }
    }

    /// Publish `kind` notifications to `topic`, or clear the topic (and so
    /// disable publishing for that class) with `None`.
    pub fn set_topic(&mut self, kind: NotificationType, topic: Option<String>) {
        let slot = match kind {
            NotificationType::Bounce => &mut self.bounce_topic,
            NotificationType::Complaint => &mut self.complaint_topic,
            NotificationType::Delivery => &mut self.delivery_topic,
        };
        *slot = topic;
    }

    /// Whether the original email headers travel with a `kind` notification.
    pub fn headers_enabled(&self, kind: NotificationType) -> bool {
        match kind {
            NotificationType::Bounce => self.headers_in_bounce_notifications_enabled,
            NotificationType::Complaint => self.headers_in_complaint_notifications_enabled,
            NotificationType::Delivery => self.headers_in_delivery_notifications_enabled,
        }
    }

    /// Include (or stop including) the original email headers in a `kind`
    /// notification.
    pub fn set_headers_enabled(&mut self, kind: NotificationType, enabled: bool) {
        let slot = match kind {
            NotificationType::Bounce => &mut self.headers_in_bounce_notifications_enabled,
            NotificationType::Complaint => &mut self.headers_in_complaint_notifications_enabled,
            NotificationType::Delivery => &mut self.headers_in_delivery_notifications_enabled,
        };
        *slot = enabled;
    }

    /// Whether a topic is set for both `Bounce` and `Complaint`, which is the
    /// condition `SetIdentityFeedbackForwardingEnabled` documents for turning
    /// feedback forwarding OFF: "This value can only be set to `false` when
    /// Amazon SNS topics are set for both `Bounce` and `Complaint` notification
    /// types."
    pub fn publishes_bounces_and_complaints(&self) -> bool {
        self.topic(NotificationType::Bounce).is_some()
            && self.topic(NotificationType::Complaint).is_some()
    }
}

/// One identity in the account's identity list.
#[derive(Debug, Clone)]
pub struct Identity {
    /// The identity itself: an email address, or a domain.
    pub name: String,
    /// Which class of identity this is.
    pub identity_type: IdentityType,
    /// This identity's verification state.
    pub verification_status: VerificationStatus,
    /// The domain-verification TXT token. `Some` only for a domain identity:
    /// `IdentityVerificationAttributes$VerificationToken` is documented as
    /// "The verification token for a domain identity. Null for email address
    /// identities."
    pub verification_token: Option<String>,
    /// The Easy DKIM tokens, set by `VerifyDomainDkim` (still a stub, so empty).
    pub dkim_tokens: Vec<String>,
    /// Where this identity's notifications go, as
    /// `SetIdentityNotificationTopic` writes it and
    /// `GetIdentityNotificationAttributes` reports it.
    pub notifications: NotificationSettings,
    /// The sending-authorization policies attached to this identity, keyed by
    /// policy name.
    ///
    /// Held ON the identity record, because a policy exists only for an identity
    /// the account owns: nothing can attach one to an identity that is not in
    /// the list, and removing the identity removes its policies with it. A
    /// `BTreeMap` so `ListIdentityPolicies` answers in one stable
    /// (lexicographic) order, and bounded at [`MAX_IDENTITY_POLICIES`] entries
    /// of [`MAX_POLICY_BYTES`] each.
    ///
    /// The document is kept VERBATIM as the caller submitted it, so
    /// `GetIdentityPolicies` reports back the text that was stored rather than a
    /// re-rendering of it.
    pub policies: BTreeMap<String, String>,
    /// When the identity was added to the list.
    pub verified_at: SystemTime,
}

/// One entry of the append-only send log.
#[derive(Debug, Clone)]
pub struct SentMessage {
    /// The id `SendEmail` answered with.
    pub message_id: String,
    /// The `Source` the message was sent from.
    pub source: String,
    /// Every recipient, To: + Cc: + Bcc: flattened in that order.
    pub destinations: Vec<String>,
    /// The message subject, when the message carried one.
    pub subject: Option<String>,
    /// The message body (text part if present, else the HTML part).
    pub body: Option<String>,
    /// The configuration set the send named, when it named one.
    pub configuration_set: Option<String>,
    /// The message tags, in request order.
    pub tags: Vec<(String, String)>,
    /// When the message was accepted.
    pub sent_at: SystemTime,
}

/// The account-level sending settings every send is checked against.
#[derive(Debug, Clone)]
pub struct AccountSettings {
    /// The daily sending quota, reported as `GetSendQuota$Max24HourSend`.
    pub max_24_hour_send: f64,
    /// The maximum send rate, reported as `GetSendQuota$MaxSendRate`.
    pub max_send_rate: f64,
    /// Whether sending is enabled for the whole account
    /// (`UpdateAccountSendingEnabled`); a send is refused with
    /// `AccountSendingPausedException` while it is false.
    pub sending_enabled: bool,
    /// Whether this account has been granted production access, which is the
    /// opposite of being in the sandbox. An operation that SES serves only to an
    /// account out of the sandbox — `SendCustomVerificationEmail`, whose
    /// `ProductionAccessNotGrantedException` reports exactly this — is refused
    /// while it is false. Starts at [`PRODUCTION_ACCESS_AT_ACCOUNT_START`].
    pub production_access_granted: bool,
}

impl Default for AccountSettings {
    fn default() -> Self {
        Self {
            max_24_hour_send: SANDBOX_MAX_24_HOUR_SEND,
            max_send_rate: SANDBOX_MAX_SEND_RATE,
            sending_enabled: true,
            production_access_granted: PRODUCTION_ACCESS_AT_ACCOUNT_START,
        }
    }
}

/// One email template in the account's template store.
///
/// The members are the model's own `Template` shape: a name, and the subject,
/// text and HTML parts a send renders. `created_at` is the instant the template
/// was recorded, which `ListTemplates` reports as
/// `TemplateMetadata$CreatedTimestamp`.
#[derive(Debug, Clone)]
pub struct EmailTemplate {
    /// The template's name, which is its key in the store.
    pub name: String,
    /// `Template$SubjectPart` — "The subject line of the email."
    pub subject_part: Option<String>,
    /// `Template$TextPart` — the body shown to clients that display no HTML.
    pub text_part: Option<String>,
    /// `Template$HtmlPart` — "The HTML body of the email."
    pub html_part: Option<String>,
    /// When the template was recorded.
    pub created_at: SystemTime,
}

/// A configuration set a send may name.
#[derive(Debug, Clone, Default)]
pub struct ConfigurationSet {
    /// The set's name, which is its key in the store.
    pub name: String,
    /// Whether sending is enabled for this set
    /// (`UpdateConfigurationSetSendingEnabled`); a send that names it is refused
    /// with `ConfigurationSetSendingPausedException` while it is false.
    pub sending_enabled: bool,
    /// Whether Amazon SES publishes reputation metrics (bounce and complaint
    /// rates) to Amazon CloudWatch for this set
    /// (`UpdateConfigurationSetReputationMetricsEnabled`), reported as
    /// `ReputationOptions$ReputationMetricsEnabled`. Per set: the member
    /// documents "The default value is false", and one set's flag says nothing
    /// about another's.
    pub reputation_metrics_enabled: bool,
    /// The custom open-and-click redirect domain of this set's `TrackingOptions`
    /// sub-resource.
    ///
    /// The sub-resource EXISTS exactly while this is `Some`: that is what
    /// `CreateConfigurationSetTrackingOptions` adds, what
    /// `UpdateConfigurationSetTrackingOptions` replaces (and what its
    /// `TrackingOptionsDoesNotExistException` reports the absence of), and what
    /// `DescribeConfigurationSet` reports as `TrackingOptions`.
    pub custom_redirect_domain: Option<String>,
    /// The `TlsPolicy` of this set's `DeliveryOptions` sub-resource: whether a
    /// message that uses the set is only delivered over a TLS connection
    /// (`Require`) or may fall back to plain text (`Optional`).
    ///
    /// The sub-resource EXISTS exactly while this is `Some`: that is what
    /// `PutConfigurationSetDeliveryOptions` writes and what
    /// `DescribeConfigurationSet` reports as `DeliveryOptions`. The model
    /// documents no default for `DeliveryOptions$TlsPolicy`, so a set nobody has
    /// written delivery options for carries none rather than a policy the
    /// service never stored.
    pub tls_policy: Option<String>,
    /// When this set's reputation metrics were last reset — a *fresh start* —
    /// and `None` while sending for the set "has never been disabled and later
    /// re-enabled", which is what `ReputationOptions$LastFreshStart` documents
    /// and what `DescribeConfigurationSet` reports.
    ///
    /// Written only by [`Self::set_sending_enabled`], which is where that rule
    /// lives.
    pub last_fresh_start: Option<SystemTime>,
}

impl ConfigurationSet {
    /// Enable or disable email sending for this set, as of `at`.
    ///
    /// Re-enabling a set whose sending was disabled resets the set's reputation
    /// metrics — `ReputationOptions$LastFreshStart`: "When you disable email
    /// sending for a configuration set [...] and later re-enable it, the
    /// reputation metrics for the configuration set (but not for the entire
    /// Amazon SES account) are reset." So the fresh start is stamped on the
    /// disabled-to-enabled transition only: a call that sets the state the set
    /// already holds is idempotent and resets nothing, and disabling never
    /// stamps one.
    pub fn set_sending_enabled(&mut self, enabled: bool, at: SystemTime) {
        if enabled && !self.sending_enabled {
            self.last_fresh_start = Some(at);
        }
        self.sending_enabled = enabled;
    }
}

/// A receipt rule set: a named, ordered collection of receipt rules.
///
/// The rules are kept as the modelled [`crate::ops::types::ReceiptRule`] shape,
/// so a rule a caller wrote is reported back unchanged by every reader instead
/// of being projected through a second copy of the same structure.
#[derive(Debug, Clone)]
pub struct ReceiptRuleSet {
    /// The set's name, which is its key in the store.
    pub name: String,
    /// When the set was created, reported as
    /// `ReceiptRuleSetMetadata$CreatedTimestamp`.
    pub created_at: SystemTime,
    /// The set's rules, in the order SES applies them.
    pub rules: Vec<crate::ops::types::ReceiptRule>,
}

/// Per-service state for com-amazonaws-ses-simpleemailservice. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsSesSimpleemailserviceState {
    /// The identity list, keyed by the identity string. A `BTreeMap` so every
    /// listing is in one stable (lexicographic) order, which is what makes
    /// `ListIdentities`' pagination resumable. Bounded at [`MAX_IDENTITIES`].
    pub identities: BTreeMap<String, Identity>,
    /// The send log, oldest first. Appended through
    /// [`Self::log_sent_message`], which is where its bounds are applied.
    pub sent_messages: Vec<SentMessage>,
    /// The account's sending settings.
    pub account: AccountSettings,
    /// The configuration sets, keyed by name. Grown ONLY through
    /// [`Self::add_configuration_set`], which is where its ceiling
    /// ([`MAX_CONFIGURATION_SETS`]) is applied; a handler that inserted here
    /// directly would let caller input grow this store without end.
    pub configuration_sets: BTreeMap<String, ConfigurationSet>,
    /// The receipt rule sets, keyed by name. A `BTreeMap` so every listing is in
    /// one stable (lexicographic) order. Bounded at [`MAX_RECEIPT_RULE_SETS`].
    pub receipt_rule_sets: BTreeMap<String, ReceiptRuleSet>,
    /// The NAME of the receipt rule set email receiving is currently served by,
    /// and `None` while no set is active — which is where an account starts, and
    /// what disables email receiving completely.
    ///
    /// A name rather than a copy of the set, so the active set's metadata and
    /// rules are the ones its entry in [`Self::receipt_rule_sets`] holds at the
    /// moment of the read ([`Self::active_rule_set`]) instead of a second,
    /// stale copy of them. This is a pointer into that store, so it grows by at
    /// most one name and needs no ceiling of its own.
    pub active_receipt_rule_set: Option<String>,
    /// The receipt IP address filters, keyed by name, held as the modelled
    /// [`crate::ops::types::ReceiptFilter`] shape so a filter a caller wrote is
    /// reported back unchanged instead of being projected through a second copy
    /// of the same structure. A `BTreeMap` so every listing is in one stable
    /// (lexicographic) order. Grown ONLY through [`Self::add_receipt_filter`],
    /// which is where its ceiling ([`MAX_RECEIPT_FILTERS`]) is applied.
    pub receipt_filters: BTreeMap<String, crate::ops::types::ReceiptFilter>,
    /// The email templates, keyed by name. A `BTreeMap` so every listing is in
    /// one stable (lexicographic) order, which is what makes `ListTemplates`'
    /// pagination resumable. Bounded at [`MAX_TEMPLATES`].
    pub templates: BTreeMap<String, EmailTemplate>,
    /// The custom verification email templates a `SendCustomVerificationEmail`
    /// may name, keyed by template name and held as the modelled
    /// [`crate::ops::types::CustomVerificationEmailTemplate`] shape so a
    /// template a caller wrote is reported back unchanged instead of being
    /// projected through a second copy of the same structure. A `BTreeMap` so
    /// every listing is in one stable (lexicographic) order, which is what makes
    /// `ListCustomVerificationEmailTemplates`' pagination resumable. Grown ONLY
    /// through [`Self::put_custom_verification_email_template`], which is where
    /// its ceiling ([`MAX_CUSTOM_VERIFICATION_EMAIL_TEMPLATES`]) is applied.
    pub custom_verification_email_templates:
        BTreeMap<String, crate::ops::types::CustomVerificationEmailTemplate>,
    /// How many opaque ids this store has minted, which is what makes the next
    /// one UNIQUE rather than merely unlikely to repeat: every id digests this
    /// counter, and the counter is never reused. Written only by
    /// [`Self::mint_token`].
    minted_tokens: u64,
}

impl ComAmazonawsSesSimpleemailserviceState {
    /// Add `name` to the identity list as an identity of `identity_type`, and
    /// hand back the stored record.
    ///
    /// Idempotent: re-verifying an identity leaves the existing record — and the
    /// verification token a domain identity carries — in place, so the list never
    /// holds a duplicate and a token stays stable across reads.
    ///
    /// The record is stored [`VerificationStatus::Success`]: a real verification
    /// completes only when someone follows the link in the message SES sends to
    /// the address (or publishes the domain's TXT record), and this emulator
    /// delivers no mail and reads no DNS, so a record left `Pending` could never
    /// advance and every later send from it would be refused. Recording it
    /// verified is the one choice that is consistent and observable: the next
    /// `GetIdentityVerificationAttributes` reports `Success`, and `SendEmail`
    /// accepts the identity as a `Source`.
    ///
    /// `None` when the list already holds [`MAX_IDENTITIES`] identities and this
    /// is a new one: the caller refuses the request rather than growing the store
    /// past the account's limit. Re-verifying an identity already in the list
    /// always succeeds, since it adds nothing.
    pub fn verify_identity(
        &mut self,
        name: &str,
        identity_type: IdentityType,
    ) -> Option<&Identity> {
        let is_new = !self.identities.contains_key(name);
        if is_new && self.identities.len() >= MAX_IDENTITIES {
            return None;
        }
        // Minted BEFORE the entry is taken, and only for an identity that is
        // about to be recorded: re-verification must not mint a second token for
        // a domain that already carries one.
        let verification_token = match (is_new, identity_type) {
            (true, IdentityType::Domain) => Some(self.new_verification_token()),
            _ => None,
        };
        Some(
            self.identities
                .entry(name.to_string())
                .or_insert_with(|| Identity {
                    name: name.to_string(),
                    identity_type,
                    verification_status: VerificationStatus::Success,
                    verification_token,
                    dkim_tokens: Vec::new(),
                    notifications: NotificationSettings::default(),
                    policies: BTreeMap::new(),
                    verified_at: SystemTime::now(),
                }),
        )
    }

    /// Record `template` under its own name, replacing any template already
    /// stored under that name, and hand back the stored record.
    ///
    /// `None` when the store already holds [`MAX_TEMPLATES`] templates and this
    /// is a new one: the caller refuses the request rather than growing the store
    /// past the account's limit. Replacing a template already in the store always
    /// succeeds, since it adds nothing.
    pub fn put_template(&mut self, template: EmailTemplate) -> Option<&EmailTemplate> {
        let name = template.name.clone();
        if !self.templates.contains_key(&name) && self.templates.len() >= MAX_TEMPLATES {
            return None;
        }
        self.templates.insert(name.clone(), template);
        self.templates.get(&name)
    }

    /// Record `template` under the template name it carries, replacing any
    /// custom verification email template already stored under that name, and
    /// hand back the stored record.
    ///
    /// `None` when the store already holds
    /// [`MAX_CUSTOM_VERIFICATION_EMAIL_TEMPLATES`] templates and this is a new
    /// one: the caller refuses the request rather than growing the store past the
    /// account's limit. Replacing a template already in the store always
    /// succeeds, since it adds nothing. A template carrying no name is refused
    /// the same way, since the name is its key.
    ///
    /// `CreateCustomVerificationEmailTemplate` is the intended caller — it is
    /// still a scaffold stub, carried by its own issue — so until it lands the
    /// only writers are the suites that seed a template to send.
    pub fn put_custom_verification_email_template(
        &mut self,
        template: crate::ops::types::CustomVerificationEmailTemplate,
    ) -> Option<&crate::ops::types::CustomVerificationEmailTemplate> {
        let name = template.template_name.clone()?;
        if !self.custom_verification_email_templates.contains_key(&name)
            && self.custom_verification_email_templates.len()
                >= MAX_CUSTOM_VERIFICATION_EMAIL_TEMPLATES
        {
            return None;
        }
        self.custom_verification_email_templates
            .insert(name.clone(), template);
        self.custom_verification_email_templates.get(&name)
    }

    /// Append an accepted message to the send log, bounding it as it grows.
    ///
    /// Two bounds, applied here so every writer gets them: the entries that have
    /// left [`SEND_LOG_RETENTION`] are dropped — no reader counts them any more,
    /// so this only materialises an expiry the clock has already made due — and,
    /// as a backstop, the oldest entries past [`MAX_SENT_LOG_ENTRIES`].
    ///
    /// The expiry is the LONGEST window a reader reports over, not the shortest:
    /// an entry the sending quota has stopped counting is still inside the
    /// window the sending statistics report, so dropping it at the quota's
    /// window would leave the statistics unable to report the two weeks they
    /// document.
    pub fn log_sent_message(&mut self, message: SentMessage) {
        let now = message.sent_at;
        self.sent_messages
            .retain(|logged| within_window(logged.sent_at, now, SEND_LOG_RETENTION));
        self.sent_messages.push(message);
        if let Some(excess) = self
            .sent_messages
            .len()
            .checked_sub(MAX_SENT_LOG_ENTRIES)
            .filter(|excess| *excess > 0)
        {
            self.sent_messages.drain(..excess);
        }
    }

    /// Whether SES would accept `address` as a `Source`.
    ///
    /// `SendEmail`: "The message must be sent from a verified email address or
    /// domain." So the addr-spec itself may be a verified identity, or any
    /// domain suffix of it may be — a verified domain covers the mailboxes in it
    /// and in its subdomains.
    pub fn is_verified_sender(&self, address: &str) -> bool {
        let addr = addr_spec(address);
        if self.is_verified(addr) {
            return true;
        }
        let Some((_, domain)) = addr.split_once('@') else {
            return false;
        };
        self.covers_verified_domain(domain)
    }

    /// Whether `domain` is itself a verified domain identity, or sits under one.
    ///
    /// A verified domain covers its subdomains, which is the same reading
    /// [`Self::is_verified_sender`] applies to a sender's domain; it is what lets
    /// a custom tracking subdomain of a verified domain be used.
    pub fn covers_verified_domain(&self, domain: &str) -> bool {
        domain_suffixes(domain).any(|suffix| self.is_verified(suffix))
    }

    /// The sending-authorization policies attached to `identity`, or `None` when
    /// this account holds no such identity.
    ///
    /// `None` is what the policy operations refuse on: they are "for the identity
    /// owner only", so an identity this account has not verified is not an
    /// identity with no policies.
    pub fn identity_policies(&self, identity: &str) -> Option<&BTreeMap<String, String>> {
        self.identities
            .get(identity_key(identity))
            .map(|stored| &stored.policies)
    }

    /// [`Self::identity_policies`] for a writer: the same map, keyed the same
    /// way, so a policy is put on and read back from one record.
    pub fn identity_policies_mut(
        &mut self,
        identity: &str,
    ) -> Option<&mut BTreeMap<String, String>> {
        self.identities
            .get_mut(identity_key(identity))
            .map(|stored| &mut stored.policies)
    }

    /// Whether `name` is in the identity list with a successful verification.
    pub fn is_verified(&self, name: &str) -> bool {
        self.identities
            .get(name)
            .is_some_and(|identity| identity.verification_status == VerificationStatus::Success)
    }

    /// The custom redirect domain a tracking-options request may be stored with,
    /// or the message Amazon SES refuses it as invalid tracking options with.
    ///
    /// Both refusals are the ones `InvalidTrackingOptionsException` documents,
    /// in the words it documents them with: the error "appears most often in the
    /// following situations: When the tracking domain you specified is not
    /// verified in Amazon SES. When the tracking domain you specified is not a
    /// valid domain or subdomain." Neither sentence carries a value, so neither
    /// message interpolates one. A tracking subdomain is covered by the verified
    /// domain above it ([`Self::covers_verified_domain`]), which is how a custom
    /// open-and-click domain is set up.
    ///
    /// Only a single trailing dot is taken off — the root label of a fully
    /// qualified name, which names the same domain. Nothing else is normalised:
    /// a padded or doubly dotted value is not a domain name and is refused as
    /// one.
    ///
    /// Shared by `CreateConfigurationSetTrackingOptions` and
    /// `UpdateConfigurationSetTrackingOptions`, which read one rule between
    /// them; each wraps the message in its own modelled error.
    pub fn tracking_domain(&self, custom_redirect_domain: Option<&str>) -> Result<String, String> {
        let given = custom_redirect_domain.unwrap_or_default();
        let domain = given.strip_suffix('.').unwrap_or(given);
        if !is_domain_name(domain) {
            return Err(
                "The tracking domain you specified is not a valid domain or subdomain."
                    .to_string(),
            );
        }
        if !self.covers_verified_domain(domain) {
            return Err(
                "The tracking domain you specified is not verified in Amazon SES.".to_string(),
            );
        }
        Ok(domain.to_string())
    }

    /// The number of emails sent in the trailing [`SEND_QUOTA_WINDOW`], as
    /// `GetSendQuota$SentLast24Hours` reports it.
    ///
    /// DERIVED from the send log on every read rather than kept as a counter, so
    /// it falls again as messages age out of the window.
    ///
    /// Every RECIPIENT counts, not every message, because that is what SES
    /// counts: "For every message that you send, the total number of recipients
    /// (including each recipient in the To:, CC: and BCC: fields) is counted
    /// against the maximum number of emails you can send in a 24-hour period
    /// (your sending quota)" (`SendEmail`'s own documentation).
    pub fn sent_last_24_hours(&self, now: SystemTime) -> f64 {
        // Counted as an integer and converted once: an `f64` sum would answer
        // `-0` for an empty log, because `-0.0` is the identity std uses for
        // floating-point `Sum`, and `-0` is not what SES reports for "nothing
        // sent".
        let recipients: usize = self
            .sent_messages
            .iter()
            .filter(|message| within_window(message.sent_at, now, SEND_QUOTA_WINDOW))
            .map(|message| message.destinations.len())
            .sum();
        recipients as f64
    }

    /// The sending activity of the trailing [`SEND_STATISTICS_WINDOW`], bucketed
    /// into the [`SEND_STATISTICS_PERIOD`] data points `GetSendStatistics`
    /// reports: `(the period's start, the deliveries attempted in that period)`,
    /// oldest period first.
    ///
    /// DERIVED from the send log on every read, like [`Self::sent_last_24_hours`],
    /// so a period's numbers never drift from the messages the log holds and the
    /// list shortens again as activity ages out of the window.
    ///
    /// Every RECIPIENT is one delivery attempt, for the reason
    /// [`Self::sent_last_24_hours`] counts recipients: SES counts each recipient
    /// of a message as an email sent. A period the log has no message in is NOT a
    /// data point — the statistics report the periods there was activity in, not
    /// one entry per period of the window.
    pub fn send_data_points(&self, now: SystemTime) -> Vec<(SystemTime, u64)> {
        // Keyed by the period's start so the buckets come out in one stable,
        // chronological order however the log is ordered.
        let mut periods: BTreeMap<u64, u64> = BTreeMap::new();
        for message in self
            .sent_messages
            .iter()
            .filter(|message| within_window(message.sent_at, now, SEND_STATISTICS_WINDOW))
        {
            let Some(period) = statistics_period_start(message.sent_at) else {
                continue;
            };
            let deliveries = periods.entry(period).or_default();
            *deliveries = deliveries.saturating_add(message.destinations.len() as u64);
        }
        periods
            .into_iter()
            .filter_map(|(period, deliveries)| {
                let start = UNIX_EPOCH.checked_add(Duration::from_secs(period))?;
                Some((start, deliveries))
            })
            .collect()
    }

    /// The verified EMAIL ADDRESS identities of this account, in one stable
    /// (lexicographic) order — the view `ListVerifiedEmailAddresses` reports.
    ///
    /// A domain identity is not an email address, so it is not in this list;
    /// `ListIdentities` is the operation that reports both classes, which is what
    /// the deprecated listing's documentation points a caller at.
    pub fn verified_email_addresses(&self) -> Vec<String> {
        self.identities
            .values()
            .filter(|identity| {
                identity.identity_type == IdentityType::EmailAddress
                    && identity.verification_status == VerificationStatus::Success
            })
            .map(|identity| identity.name.clone())
            .collect()
    }

    /// A fresh opaque token for this store, as 64 hex characters.
    ///
    /// Unique rather than merely improbable: every token digests
    /// [`Self::minted_tokens`], which this method advances and never reuses, so
    /// no two tokens from one store can collide. Opaque because the digest also
    /// covers entropy the system seeded ([`system_entropy`]) and the minting
    /// instant, so a caller cannot derive the next token from one it holds, and
    /// NOTHING from the request goes in: a token identifies what the service
    /// stored, it is never computed from what the caller sent.
    ///
    /// Hashed with the `sha2` crate rather than a hand-written digest.
    fn mint_token(&mut self) -> String {
        self.minted_tokens = self.minted_tokens.wrapping_add(1);
        let minted_at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |since_epoch| since_epoch.as_nanos());
        let mut digest = Sha256::new();
        digest.update(self.minted_tokens.to_be_bytes());
        digest.update(system_entropy().to_be_bytes());
        digest.update(minted_at.to_be_bytes());
        hex::encode(digest.finalize())
    }

    /// A fresh `MessageId` in the shape SES answers `SendEmail` with:
    /// `0100<12 hex>-<8-4-4-4-12 hex>-000000`.
    ///
    /// Server-generated (see [`Self::mint_token`]) and stored on the
    /// [`SentMessage`] it identifies: the value the operation answers with is the
    /// one it wrote.
    pub fn new_message_id(&mut self) -> String {
        let token = self.mint_token();
        format!(
            "0100{}-{}-{}-{}-{}-{}-000000",
            &token[..12],
            &token[12..20],
            &token[20..24],
            &token[24..28],
            &token[28..32],
            &token[32..44]
        )
    }

    /// A fresh domain-verification token, the value a domain identity publishes
    /// as a TXT record: 44 opaque characters, the length SES's own token
    /// carries. Opaque to the caller; only its stability across reads matters,
    /// which the stored [`Identity`] record is what provides.
    pub fn new_verification_token(&mut self) -> String {
        self.mint_token()[..44].to_string()
    }

    /// Record a configuration set named `name`, and hand back the stored record.
    ///
    /// Idempotent: a name already in the store keeps the record (and the sending
    /// state) it already carries, so the caller can tell a conflict from a
    /// create by checking the store first.
    ///
    /// `None` when the store already holds [`MAX_CONFIGURATION_SETS`] sets and
    /// this is a new one: the caller refuses the request with the modelled
    /// `LimitExceeded` rather than growing the store past the account's limit.
    pub fn add_configuration_set(&mut self, name: &str) -> Option<&ConfigurationSet> {
        if !self.configuration_sets.contains_key(name)
            && self.configuration_sets.len() >= MAX_CONFIGURATION_SETS
        {
            return None;
        }
        Some(
            self.configuration_sets
                .entry(name.to_string())
                .or_insert_with(|| ConfigurationSet {
                    name: name.to_string(),
                    // A set SES has just created is not paused: `SendEmail`
                    // refuses a send naming it only after
                    // `UpdateConfigurationSetSendingEnabled` disables it.
                    sending_enabled: true,
                    // And it publishes no reputation metrics:
                    // `ReputationOptions$ReputationMetricsEnabled` documents
                    // "The default value is false", so a set starts off until
                    // `UpdateConfigurationSetReputationMetricsEnabled` enables
                    // it.
                    reputation_metrics_enabled: false,
                    // And it carries no `TrackingOptions` sub-resource:
                    // `CreateConfigurationSetTrackingOptions` is what adds one.
                    custom_redirect_domain: None,
                    // And no `DeliveryOptions` sub-resource either:
                    // `PutConfigurationSetDeliveryOptions` is what writes one.
                    tls_policy: None,
                    // A set whose sending has never been disabled and later
                    // re-enabled has had no fresh start.
                    last_fresh_start: None,
                }),
        )
    }

    /// Record a receipt rule set named `name` holding `rules`, and hand back the
    /// stored record.
    ///
    /// The caller has already decided that `name` is free — a name already in
    /// the store is a naming conflict, which every creating operation answers
    /// with its own modelled `AlreadyExists` — so this only applies the
    /// account's own ceiling: `None` when the store already holds
    /// [`MAX_RECEIPT_RULE_SETS`] sets, which the caller refuses with the
    /// modelled `LimitExceeded` rather than growing the store past the limit.
    ///
    /// `created_at` is taken here, so the timestamp a reader reports is the one
    /// this write stored.
    pub fn add_receipt_rule_set(
        &mut self,
        name: &str,
        rules: Vec<crate::ops::types::ReceiptRule>,
    ) -> Option<&ReceiptRuleSet> {
        if !self.receipt_rule_sets.contains_key(name)
            && self.receipt_rule_sets.len() >= MAX_RECEIPT_RULE_SETS
        {
            return None;
        }
        Some(
            self.receipt_rule_sets
                .entry(name.to_string())
                .or_insert_with(|| ReceiptRuleSet {
                    name: name.to_string(),
                    created_at: SystemTime::now(),
                    rules,
                }),
        )
    }

    /// The receipt rule set that is currently active, and `None` while no set
    /// is active — the two cases a reader of the active set reports.
    ///
    /// Resolved through [`Self::receipt_rule_sets`] on every call rather than
    /// stored alongside the name, so a rule added to or removed from the active
    /// set after it was made active is part of what this hands back.
    pub fn active_rule_set(&self) -> Option<&ReceiptRuleSet> {
        self.active_receipt_rule_set
            .as_deref()
            .and_then(|name| self.receipt_rule_sets.get(name))
    }

    /// Record the receipt IP address filter `filter` under its own name.
    ///
    /// The caller has already decided that the name is free — a name already in
    /// the store is a naming conflict, which the creating operation answers with
    /// its modelled `AlreadyExists` — so this only applies the account's own
    /// ceiling: `false`, having stored nothing, when the store already holds
    /// [`MAX_RECEIPT_FILTERS`] filters, which the caller refuses with the
    /// modelled `LimitExceeded` rather than growing the store past the limit.
    pub fn add_receipt_filter(&mut self, filter: crate::ops::types::ReceiptFilter) -> bool {
        // A name the store already holds would be replaced rather than added, so
        // it does not grow the store and the ceiling does not apply to it.
        let held = self.receipt_filters.contains_key(&filter.name);
        if !held && self.receipt_filters.len() >= MAX_RECEIPT_FILTERS {
            return false;
        }
        self.receipt_filters.insert(filter.name.clone(), filter);
        true
    }
}

/// Whether a message accepted at `sent_at` is still inside the trailing `window`
/// at `now`.
///
/// The one rule every window in this store is read by — the sending quota's and
/// the sending statistics' — so a message is in or out of a window in exactly
/// one place. A message whose window end cannot be represented is still inside
/// it: it cannot have left a window that does not end.
fn within_window(sent_at: SystemTime, now: SystemTime, window: Duration) -> bool {
    sent_at
        .checked_add(window)
        .is_none_or(|leaves_window| leaves_window > now)
}

/// The start of the [`SEND_STATISTICS_PERIOD`] bucket `at` falls in, as whole
/// seconds since the epoch; `None` for an instant before the epoch, which no
/// accepted message carries.
///
/// The buckets are aligned to the epoch rather than to the time of the call, so
/// one message always falls in the same period however often the statistics are
/// read.
fn statistics_period_start(at: SystemTime) -> Option<u64> {
    let since_epoch = at.duration_since(UNIX_EPOCH).ok()?.as_secs();
    let period = SEND_STATISTICS_PERIOD.as_secs();
    Some(since_epoch - since_epoch % period)
}

/// Whether `value` is a receipt rule or rule set name in the form SES documents.
///
/// The model states the rule on every `ReceiptRuleSetName` and `ReceiptRuleName`
/// member (e.g. `CloneReceiptRuleSetRequest$RuleSetName`): the name must
/// "Contain only ASCII letters (a-z, A-Z), numbers (0-9), underscores (_), or
/// dashes (-)", "Start and end with a letter or number", and "Contain 64
/// characters or fewer" ([`MAX_RECEIPT_RULE_SET_NAME`]).
pub fn is_receipt_rule_set_name(value: &str) -> bool {
    let mut characters = value.chars();
    let Some(first) = characters.next() else {
        return false;
    };
    let last = characters.next_back().unwrap_or(first);
    value.len() <= MAX_RECEIPT_RULE_SET_NAME
        && value
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '_' | '-'))
        && first.is_ascii_alphanumeric()
        && last.is_ascii_alphanumeric()
}

/// Whether `value` is a receipt IP address filter name in the form SES
/// documents.
///
/// `ReceiptFilter$Name` states the same three clauses the receipt rule and rule
/// set names carry — ASCII letters, numbers, underscores or dashes only, a
/// letter or number at each end, and [`MAX_RECEIPT_FILTER_NAME`] characters or
/// fewer — so the value is read through [`is_receipt_rule_set_name`] instead of
/// repeating them.
pub fn is_receipt_filter_name(value: &str) -> bool {
    is_receipt_rule_set_name(value)
}

/// The IPv4 address or range `value` names, or `None` when it is neither.
///
/// `ReceiptIpFilter$Cidr` documents the form: "A single IP address or a range of
/// IP addresses to block or allow, specified in Classless Inter-Domain Routing
/// (CIDR) notation. An example of a single email address is 10.0.0.1. An example
/// of a range of IP addresses is 10.0.0.1/24." A bare address is the range that
/// holds only it, so it reads as a `/32`; a range carries its own prefix length,
/// which `ipnet` accepts only within the address family's width. Both examples
/// the member gives are IPv4, and an IPv4 address is the only form it documents,
/// so an address of another family is one of the values this reads as neither.
pub fn parse_receipt_filter_cidr(value: &str) -> Option<::ipnet::Ipv4Net> {
    if let Ok(address) = value.parse::<std::net::Ipv4Addr>() {
        return Some(::ipnet::Ipv4Net::from(address));
    }
    value.parse::<::ipnet::Ipv4Net>().ok()
}

/// The listing name stamped into a `ListTemplates` page token, so a cursor cut
/// from the template list cannot resume another listing of this service.
pub const LISTING_TEMPLATES: &str = "ListTemplates";

/// The listing name stamped into a `ListReceiptRuleSets` page token, for the same
/// reason: a cursor cut from the receipt rule set list resumes only that listing.
pub const LISTING_RECEIPT_RULE_SETS: &str = "ListReceiptRuleSets";

/// The listing name stamped into a `ListConfigurationSets` page token, for the
/// same reason: a cursor cut from the configuration set list resumes only that
/// listing.
pub const LISTING_CONFIGURATION_SETS: &str = "ListConfigurationSets";

/// The bytes every page token of this service digests before the listing and the
/// key, so a token of this service is not a token of any other.
const PAGE_TOKEN_DOMAIN: &[u8] = b"com.amazonaws.ses#page-token\0";

/// The leading bytes of the digest a page token carries as its integrity tag.
/// Eight bytes is enough that a value the caller made up does not carry a
/// matching tag, while keeping the token short.
const PAGE_TOKEN_TAG_LEN: usize = 8;

/// The integrity tag of `key` for `listing`: the leading [`PAGE_TOKEN_TAG_LEN`]
/// bytes of SHA-256 over this service's domain separator, the listing, and the
/// key. Hashed with the `sha2` crate rather than a hand-written digest.
fn page_token_tag(listing: &str, key: &str) -> Vec<u8> {
    let mut digest = Sha256::new();
    digest.update(PAGE_TOKEN_DOMAIN);
    digest.update(listing.as_bytes());
    digest.update(b"\0");
    digest.update(key.as_bytes());
    digest.finalize()[..PAGE_TOKEN_TAG_LEN].to_vec()
}

/// A page token naming the `key` the NEXT page of `listing` starts at: the
/// integrity tag followed by the key, hex-encoded.
///
/// Opaque — a caller cannot read a position out of it and hands it back verbatim
/// — and self-verifying, so a token this listing never issued is refused by
/// [`decode_page_token`] instead of being read as an arbitrary position in it.
pub fn encode_page_token(listing: &str, key: &str) -> String {
    let mut bytes = page_token_tag(listing, key);
    bytes.extend_from_slice(key.as_bytes());
    ::hex::encode(bytes)
}

/// The key a token of `listing` resumes at, or `None` when `token` is not one
/// this service issued for that listing: not hex, too short to carry a tag, not
/// UTF-8, carrying no key, or carrying a tag that does not match the listing and
/// position it claims. The caller refuses such a token rather than restarting
/// the listing at some other position.
pub fn decode_page_token(listing: &str, token: &str) -> Option<String> {
    let bytes = ::hex::decode(token).ok()?;
    if bytes.len() <= PAGE_TOKEN_TAG_LEN {
        return None;
    }
    let (tag, key) = bytes.split_at(PAGE_TOKEN_TAG_LEN);
    let key = String::from_utf8(key.to_vec()).ok()?;
    (page_token_tag(listing, &key) == tag).then_some(key)
}

/// The addr-spec of a `Source`/recipient value, which may arrive either bare
/// (`user@example.com`) or with a display name (`"Me" <user@example.com>`).
///
/// A value with no angle-addr is returned trimmed and unchanged, so a malformed
/// value stays malformed for the caller's own check.
pub fn addr_spec(value: &str) -> &str {
    let trimmed = value.trim();
    match (trimmed.rfind('<'), trimmed.rfind('>')) {
        (Some(open), Some(close)) if open + 1 < close => trimmed[open + 1..close].trim(),
        _ => trimmed,
    }
}

/// The identity an ARN of this service names, when it names one: the ARN's
/// `identity` resource, read down to the identity's own name and keyed by its
/// addr-spec. `None` for a value that is not an ARN of this service, names
/// another resource of it, or names no identity at all.
///
/// The ONE reading of the identity-ARN form in this service, so
/// [`identity_key`] and [`resolve_identity`] cannot disagree about what an
/// identity ARN names; the ARN itself is read by `service_common::arn`, the one
/// caller-supplied-ARN parse in this tree.
fn identity_named_by_arn(value: &str) -> Option<&str> {
    ::service_common::arn::is_resource_arn_of(value, SES_ARN_VENDOR)
        .then(|| ::service_common::arn::resource_id_of_type(value, IDENTITY_RESOURCE_TYPE))
        .flatten()
        .filter(|name| !name.is_empty())
        .map(addr_spec)
}

/// The key an identity is stored under, for a caller value that may name the
/// identity either way the policy operations document.
///
/// Their `Identity` member states both forms: "You can specify an identity by
/// using its name or by using its Amazon Resource Name (ARN). Examples:
/// user@example.com, example.com,
/// arn:aws:ses:us-east-1:123456789012:identity/example.com." So an ARN of this
/// service naming an `identity` resource is read down to the identity it names
/// ([`identity_named_by_arn`]), and anything else is the identity itself; either
/// way the value is keyed by its addr-spec, the way the writers key it
/// ([`addr_spec`]).
pub fn identity_key(value: &str) -> &str {
    let trimmed = value.trim();
    identity_named_by_arn(trimmed).unwrap_or_else(|| addr_spec(trimmed))
}

/// The vendor field of this service's own ARNs, which is its signing name.
const SES_ARN_VENDOR: &str = "ses";

/// The resource type an SES identity ARN addresses
/// (`arn:aws:ses:<region>:<account>:identity/<name>`).
const IDENTITY_RESOURCE_TYPE: &str = "identity";

/// The domain itself and every parent domain of it, longest first
/// (`mail.corp.example.com` -> `mail.corp.example.com`, `corp.example.com`,
/// `example.com`), stopping before the bare top-level label.
fn domain_suffixes(domain: &str) -> impl Iterator<Item = &str> {
    let labels = domain.match_indices('.').map(|(at, _)| at + 1);
    std::iter::once(0)
        .chain(labels)
        .filter(move |start| domain[*start..].contains('.'))
        .map(move |start| &domain[start..])
}

/// Whether `value` is an email address in the form SES requires.
///
/// `SendEmail` states the form — "not in the format
/// UserName@[SubDomain.]Domain.TopLevelDomain" is what makes an address invalid
/// — and `SendEmail$Source` states the character set: "Amazon SES does not
/// support the SMTPUTF8 extension, as described in RFC6531. For this reason, the
/// email address string must be 7-bit ASCII."
///
/// That is the form every service whose model documents a member as an email
/// address reads, so it is read through the ONE shared reading of one
/// (`service_common::email_address`) rather than stated again here: a dot-atom
/// local part of 7-bit-ASCII atom characters — so a leading, trailing or doubled
/// dot, which would make an EMPTY atom, is not an address — an `@`, and a domain
/// name ([`is_domain_name`]).
pub fn is_email_address(value: &str) -> bool {
    ::service_common::email_address::is_email_address(value)
}

/// The vendor field of an Amazon SNS ARN (`aws.api#service$arnNamespace`): the
/// third field of a topic ARN.
const SNS_ARN_VENDOR: &str = "sns";

/// The partition every ARN this emulator mints and serves carries
/// (`engine::emulator_arn` writes `arn:aws:…`), so an ARN of another partition
/// names a resource in a partition this emulator does not serve.
const ARN_PARTITION: &str = "aws";

/// The Region an operation serves a request that carries no SigV4 credential
/// scope in, which is the Region AWS's own `email` endpoint resolves to without
/// one.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The longest Region LABEL this service reads: every `*Arn` pattern in the AWS
/// models bounds the Region field at `[a-z0-9-]{1,20}`, so longer text is no
/// Region any AWS endpoint has.
const MAX_REGION_LENGTH: usize = 20;

/// Whether `value` is a Region LABEL, under the grammar those patterns state.
///
/// Read on the way IN ([`region_from_request`]) so the Region every later
/// comparison and message uses is bounded, whatever a caller's credential scope
/// carries.
fn is_region_label(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_REGION_LENGTH
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

/// The Region the caller's request is scoped to: the Region of its SigV4
/// credential scope, or [`DEFAULT_REGION`] when it carries none — or carries text
/// that is no Region label ([`is_region_label`]).
///
/// The scope is parsed by `emulator_server::sigv4_credential_scope`, the single
/// source of SigV4-scope parsing, so a header that is not a well-formed SigV4
/// `Authorization` yields the default rather than a bogus Region.
pub fn region_from_request(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .filter(|region| is_region_label(region))
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The identities ONE call may ask about, shared by the two operations that
/// report per-identity attributes: each is documented as being able to "only
/// get … attributes for up to 100 identities at a time"
/// (`GetIdentityNotificationAttributes` and
/// `GetIdentityVerificationAttributes`). The model carries no `@length` for
/// either list, so the boundary validator passes a longer one and the operation
/// refuses it itself.
pub const MAX_IDENTITIES_PER_CALL: usize = 100;

/// The characters an Amazon SNS topic name is made of, and the length it may
/// reach: "Topic names must be made up of only uppercase and lowercase ASCII
/// letters, numbers, underscores, and hyphens, and must be between 1 and 256
/// characters long" (the Amazon SNS documentation of a topic name, which is the
/// resource field of a topic ARN).
const MAX_TOPIC_NAME_LENGTH: usize = 256;

/// Whether `value` is an Amazon SNS topic NAME, as Amazon SNS documents one (see
/// [`MAX_TOPIC_NAME_LENGTH`]).
fn is_sns_topic_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_TOPIC_NAME_LENGTH
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
}

/// Whether the ARN `value`, of `vendor`, names a resource of the one account this
/// emulator serves, in the partition it serves and in the Region the caller's
/// request is scoped to.
///
/// An ARN names ONE account's resource in ONE Region, and this emulator serves a
/// single account ([`::engine::EMULATOR_ACCOUNT_ID`]) — so an ARN of another
/// account, partition or Region names nothing it holds. The six fields come from
/// ONE `arn::parse_of`; the Region needs no second reading, because the Region it
/// is compared against is already a label ([`region_from_request`]).
fn names_a_resource_here(value: &str, vendor: &str, region: &str) -> bool {
    ::service_common::arn::parse_of(value, vendor).is_some_and(|arn| {
        arn.partition == ARN_PARTITION
            && arn.account == ::engine::EMULATOR_ACCOUNT_ID
            && arn.region == region
    })
}

/// The Amazon SNS topic ARN a request names, or the reason the value names no
/// topic SES could publish to for this caller.
///
/// What a member the model documents as "the Amazon Resource Name (ARN) of the
/// Amazon SNS topic" has to carry. The six fields come from
/// `service_common::arn`, the one caller-supplied-ARN parse in this tree; what is
/// decided here is the rest of the published form of a topic ARN,
/// `arn:<partition>:sns:<region>:<account-id>:<topic-name>` — the vendor is
/// Amazon SNS's, the resource field is the topic NAME alone
/// ([`is_sns_topic_name`]), since a topic ARN names no subresource while a
/// subscription ARN carries a further `:<subscription-id>` segment — and that the
/// topic is one this caller could have: in its own account, partition and Region
/// ([`names_a_resource_here`]), as a notification topic of an identity is.
///
/// Read on the ARN alone: whether the topic EXISTS is Amazon SNS's own state,
/// which this service does not hold.
pub fn resolve_sns_topic<'value>(value: &'value str, region: &str) -> Result<&'value str, String> {
    let trimmed = value.trim();
    let names_a_topic = names_a_resource_here(trimmed, SNS_ARN_VENDOR, region)
        && ::service_common::arn::resource(trimmed).is_some_and(is_sns_topic_name);
    if names_a_topic {
        Ok(trimmed)
    } else {
        Err(format!(
            "Invalid SnsTopic {trimmed:?}: SnsTopic must be the ARN of an Amazon SNS topic of \
             account {} in {region}",
            ::engine::EMULATOR_ACCOUNT_ID
        ))
    }
}

/// The identity-list key for an identity a request names, or the reason the
/// value names no identity this emulator holds.
///
/// [`identity_key`] reads the two forms the `Identity` member documents — the
/// identity's own name, and an ARN of this service naming an `identity`
/// resource. This COMPOSES with that one reading ([`identity_named_by_arn`]) and
/// adds what an ARN has to satisfy to name an identity HERE: an ARN names one
/// account's resource in one Region, and this emulator serves a single account
/// ([`names_a_resource_here`]), so an ARN of another account, partition or
/// Region names nothing — as does an ARN that names no identity at all (another
/// vendor's resource, or another resource of this service). The operations that
/// also read an Amazon SNS topic ARN ([`resolve_sns_topic`]) read BOTH their ARN
/// members this way, so one request is not held to two different readings of
/// what an ARN names; a value that is not an ARN at all is keyed exactly as
/// [`identity_key`] keys it.
///
/// `Err` carries the reason, for a caller that refuses the request with it.
pub fn resolve_identity<'value>(value: &'value str, region: &str) -> Result<&'value str, String> {
    let trimmed = value.trim();
    // A value that is not an ARN at all is the identity's own name.
    if ::service_common::arn::parse(trimmed).is_none() {
        return Ok(identity_key(trimmed));
    }
    match identity_named_by_arn(trimmed)
        .filter(|_| names_a_resource_here(trimmed, SES_ARN_VENDOR, region))
    {
        Some(name) => Ok(name),
        None => Err(format!(
            "Invalid Identity {trimmed:?}: an identity ARN must name an identity of account {} \
             in {region}",
            ::engine::EMULATOR_ACCOUNT_ID
        )),
    }
}

/// Whether `value` is a domain name with at least two non-empty labels, the
/// `[SubDomain.]Domain.TopLevelDomain` form SES documents.
///
/// The shared reading again: a label is 7-bit ASCII letters, digits and hyphens,
/// and the top-level label must carry a letter, which admits an
/// internationalised (`xn--…`) TLD and refuses an all-numeric one.
pub fn is_domain_name(value: &str) -> bool {
    ::service_common::email_address::is_domain_name(value)
}

/// Whether `name` is a configuration set name of the form the model documents.
///
/// `ConfigurationSet$Name` states both halves of the rule: a name must "contain
/// only letters (a-z, A-Z), numbers (0-9), underscores (_), or dashes (-)" and
/// "contain 64 characters or fewer" ([`MAX_CONFIGURATION_SET_NAME`]). A name
/// built from no characters at all names nothing, so it is not one either.
pub fn is_configuration_set_name(name: &str) -> bool {
    !name.is_empty()
        && name.chars().count() <= MAX_CONFIGURATION_SET_NAME
        && name
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '_' | '-'))
}

/// `text` as a log entry keeps it: at most [`MAX_LOGGED_TEXT`] characters, and
/// `None` for a text that is not there.
///
/// Truncated on a CHARACTER boundary, so a multi-byte character is never split.
pub fn logged_text(text: Option<&str>) -> Option<String> {
    text.map(|text| match text.char_indices().nth(MAX_LOGGED_TEXT) {
        Some((cut, _)) => text[..cut].to_string(),
        None => text.to_string(),
    })
}

/// How a send refuses an address that is not an address, for every operation
/// that takes one.
///
/// `SendEmail` states the rule and the consequence: a recipient address that "is
/// not in the format UserName@[SubDomain.]Domain.TopLevelDomain" makes the whole
/// message rejected. One wording here, so the operations that share that reading
/// ([`is_email_address`]) also share how they report it.
pub fn illegal_address_message(value: &str) -> String {
    format!(
        "Illegal address: {value:?} is not of the form \
         UserName@[SubDomain.]Domain.TopLevelDomain"
    )
}

/// How a send refuses a message that would take the account past its 24-hour
/// sending quota, for every operation that counts against it.
///
/// `GetSendQuota$Max24HourSend` is "the maximum number of emails the user is
/// allowed to send in a 24-hour interval" and `SentLast24Hours` what has been
/// sent in it, so the refusal names both. One wording, for the same reason as
/// [`illegal_address_message`].
pub fn daily_quota_exceeded_message(quota: f64, already_sent: f64) -> String {
    format!(
        "Daily message quota exceeded: this account may send {quota} messages per 24 hours \
         and has sent {already_sent}"
    )
}

/// 64 bits of entropy this process did not choose, salting a minted id so it is
/// opaque as well as unique.
///
/// `std::collections::hash_map::RandomState::new` "is initialized with random
/// keys" (its own documentation) — the system seeds them — and a fresh one per
/// call carries fresh keys, so finishing its hasher yields a value a caller
/// cannot derive from the request or from an earlier id. It salts an identifier
/// and nothing else: it is never a key, a secret or a signature, and the
/// UNIQUENESS of an id comes from the stored counter it digests, not from here.
fn system_entropy() -> u64 {
    use std::collections::hash_map::RandomState;
    use std::hash::{BuildHasher, Hasher};
    RandomState::new().build_hasher().finish()
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsSesSimpleemailserviceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsSesSimpleemailserviceState>)
}
