// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-ses-simpleemailservice service.
//!
//! One store per emulator: the identity list SES keys every send against (with
//! the sending-authorization policies each identity carries), the send log the
//! sending quota is derived from, the account's sending limits, and the
//! configuration sets a send may name.
//!
//! Nothing here is a counter that a reader bumps: `SentLast24Hours` is COMPUTED
//! from the send log (see [`ComAmazonawsSesSimpleemailserviceState::sent_last_24_hours`]),
//! so it falls again as messages age out of the window instead of drifting.
//!
//! Every caller-driven store is BOUNDED, because every request reaching this
//! emulator can grow one of them: the identity list at [`MAX_IDENTITIES`]
//! entries, each identity's sending-authorization policies at
//! [`MAX_IDENTITY_POLICIES`] documents of [`MAX_POLICY_BYTES`], and the send log
//! at the account's own 24-hour quota, with the window's expiry applied on
//! append ([`ComAmazonawsSesSimpleemailserviceState::log_sent_message`]) and
//! [`MAX_SENT_LOG_ENTRIES`] as the backstop. The text one log entry keeps is
//! capped at [`MAX_LOGGED_TEXT`]. The template store is bounded the same way, at
//! [`MAX_TEMPLATES`].
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

/// The entries the send log may hold at once.
///
/// The 24-hour quota is the real ceiling — a day's accepted sends are at most
/// `max_24_hour_send` recipients — and the window's expiry is applied on every
/// append, so this is the backstop that still bounds the store if the stored
/// quota is raised later.
pub const MAX_SENT_LOG_ENTRIES: usize = 1_000;

/// The email templates one account may hold, and so the ceiling on the template
/// store: SES documents a limit of 10,000 email templates per account, and a
/// creation past it is refused rather than letting caller input grow this store
/// without end.
pub const MAX_TEMPLATES: usize = 10_000;

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
}

impl Default for AccountSettings {
    fn default() -> Self {
        Self {
            max_24_hour_send: SANDBOX_MAX_24_HOUR_SEND,
            max_send_rate: SANDBOX_MAX_SEND_RATE,
            sending_enabled: true,
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
    /// The configuration sets, keyed by name.
    pub configuration_sets: BTreeMap<String, ConfigurationSet>,
    /// The email templates, keyed by name. A `BTreeMap` so every listing is in
    /// one stable (lexicographic) order, which is what makes `ListTemplates`'
    /// pagination resumable. Bounded at [`MAX_TEMPLATES`].
    pub templates: BTreeMap<String, EmailTemplate>,
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

    /// Append an accepted message to the send log, bounding it as it grows.
    ///
    /// Two bounds, applied here so every writer gets them: the entries the
    /// 24-hour window has already left are dropped — the quota computation
    /// treats them as gone, so this only materialises an expiry the clock has
    /// already made due — and, as a backstop, the oldest entries past
    /// [`MAX_SENT_LOG_ENTRIES`].
    pub fn log_sent_message(&mut self, message: SentMessage) {
        let now = message.sent_at;
        self.sent_messages.retain(|logged| {
            logged
                .sent_at
                .checked_add(SEND_QUOTA_WINDOW)
                .is_none_or(|leaves_window| leaves_window > now)
        });
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
            .filter(|message| {
                message
                    .sent_at
                    .checked_add(SEND_QUOTA_WINDOW)
                    .is_none_or(|leaves_window| leaves_window > now)
            })
            .map(|message| message.destinations.len())
            .sum();
        recipients as f64
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
}

/// The listing name stamped into a `ListTemplates` page token, so a cursor cut
/// from the template list cannot resume another listing of this service.
pub const LISTING_TEMPLATES: &str = "ListTemplates";

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

/// The key an identity is stored under, for a caller value that may name the
/// identity either way the policy operations document.
///
/// Their `Identity` member states both forms: "You can specify an identity by
/// using its name or by using its Amazon Resource Name (ARN). Examples:
/// user@example.com, example.com,
/// arn:aws:ses:us-east-1:123456789012:identity/example.com." So an ARN of this
/// service naming an `identity` resource is read down to the identity it names,
/// and anything else is the identity itself; either way the value is keyed by
/// its addr-spec, the way the writers key it ([`addr_spec`]).
///
/// The ARN is read through the ONE caller-supplied-ARN parse
/// (`service_common::arn`) rather than by splitting the value here, so this
/// service cannot disagree with another about what an ARN is.
pub fn identity_key(value: &str) -> &str {
    let trimmed = value.trim();
    if let Some(named) = ::service_common::arn::is_resource_arn_of(trimmed, SES_ARN_VENDOR)
        .then(|| ::service_common::arn::resource_id_of_type(trimmed, IDENTITY_RESOURCE_TYPE))
        .flatten()
    {
        return addr_spec(named);
    }
    addr_spec(trimmed)
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

/// Whether `value` is a domain name with at least two non-empty labels, the
/// `[SubDomain.]Domain.TopLevelDomain` form SES documents.
///
/// The shared reading again: a label is 7-bit ASCII letters, digits and hyphens,
/// and the top-level label must carry a letter, which admits an
/// internationalised (`xn--…`) TLD and refuses an all-numeric one.
pub fn is_domain_name(value: &str) -> bool {
    ::service_common::email_address::is_domain_name(value)
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
