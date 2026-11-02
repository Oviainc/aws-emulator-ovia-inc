// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-socialmessaging-socialmessaging service.
//!
//! One store holds the WhatsApp Business Account (WABA) control plane: the linked
//! accounts, the phone numbers registered under them, the signups that are still
//! in progress, and the messages that were sent. Every handler reads and writes
//! THIS store through [`service_state`], so a write made by one request is what the
//! next request reads.

use std::collections::{HashMap, VecDeque};
use std::time::{Duration, SystemTime};

use sha2::{Digest, Sha256};

use crate::ops::types::{
    AccessDeniedException, LinkedWhatsAppBusinessAccount, LinkedWhatsAppBusinessAccountIdMetaData,
    LinkedWhatsAppBusinessAccountSummary, Tag, WhatsAppBusinessAccountEventDestination,
    WhatsAppCallSettings, WhatsAppPhoneNumberDetail, WhatsAppPhoneNumberSummary,
};

/// This service's ARN namespace and SigV4 signing name (`OPERATIONS.md`: "sigv4
/// signing name: social-messaging", "ARN namespace: social-messaging").
pub const ARN_SERVICE: &str = "social-messaging";

/// The region an ARN carries when the request is not SigV4-signed, so an
/// unsigned call still mints a well-formed ARN.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The `waba-` prefix the model's `LinkedWhatsAppBusinessAccountId` pattern
/// `(^waba-.*$)|(^arn:.*:waba/[0-9a-zA-Z]+$)` requires of the short form.
pub const LINKED_ACCOUNT_ID_PREFIX: &str = "waba-";

/// The `phone-number-id-` prefix the model's `WhatsAppPhoneNumberId` pattern
/// `(^phone-number-id-.*$)|(^arn:.*:phone-number-id/[0-9a-zA-Z]+$)` requires.
pub const PHONE_NUMBER_ID_PREFIX: &str = "phone-number-id-";

/// Members of the model's `RegistrationStatus` enum (see
/// `crate::ops::types::REGISTRATION_STATUS_VALUES`, `["COMPLETE", "INCOMPLETE"]`).
pub const REGISTRATION_STATUS_COMPLETE: &str = "COMPLETE";
/// See [`REGISTRATION_STATUS_COMPLETE`].
pub const REGISTRATION_STATUS_INCOMPLETE: &str = "INCOMPLETE";

/// What Meta reports for a phone number that has no messaging history yet, so no
/// quality signal exists to rate it. Fits `WhatsAppPhoneNumberQualityRating`'s
/// `len ..10`.
pub const QUALITY_RATING_UNKNOWN: &str = "UNKNOWN";

/// The page size `ListLinkedWhatsAppBusinessAccounts` uses when the caller sends
/// no `maxResults`: the maximum the model's `MaxResults` range (`1..100`) allows.
pub const MAX_RESULTS_LIMIT: i32 = 100;

/// How many linked accounts this emulator holds. Emulator-chosen (AWS publishes no
/// WABA quota through this API) and reported with `AssociateWhatsAppBusinessAccount`'s
/// own modelled `LimitExceededException`, which says exactly this: "The request was
/// denied because it would exceed one or more service quotas or limits." Every
/// account is created by a request, so without this bound a client could grow the
/// process without limit.
pub const MAX_LINKED_ACCOUNTS: usize = 128;

/// How many phone numbers one linked account holds. Emulator-chosen for the same
/// reason, and refused with the same modelled error. With [`MAX_LINKED_ACCOUNTS`]
/// it bounds `phone_numbers` and `arn_index` too.
pub const MAX_PHONE_NUMBERS_PER_ACCOUNT: usize = 32;

/// How many signups may be in progress at once. Emulator-chosen, refused with the
/// same modelled error, so a stream of callback-only requests cannot grow the store.
pub const MAX_PENDING_SIGNUPS: usize = 64;

/// How long a signup stays finalizable. Emulator-chosen (AWS publishes no lifetime
/// for `associateInProgressToken`): a signup nobody finalized within this span is
/// dropped, so an abandoned one does not occupy the store forever.
pub const PENDING_SIGNUP_TTL: Duration = Duration::from_secs(15 * 60);

/// How many accepted sends the store retains, oldest evicted first. Emulator-chosen:
/// `SendWhatsAppMessage` appends a caller-sized blob per call and models no quota
/// error, so the record is trimmed rather than the call refused.
pub const MAX_RETAINED_SENT_MESSAGES: usize = 128;

/// The bytes of retained message bodies, which with [`MAX_RETAINED_SENT_MESSAGES`]
/// bounds the send record whatever size the modelled `1..2048000` blob arrives at.
/// The newest send is always retained, even on its own.
pub const MAX_RETAINED_SENT_MESSAGE_BYTES: usize = 8 * 1024 * 1024;

/// How many accepted conversion events the store retains, oldest evicted first.
/// Emulator-chosen for the same reason as [`MAX_RETAINED_SENT_MESSAGES`]:
/// `SendWhatsAppConversionEvent` appends a caller-sized blob per call and models no quota
/// error, so the record is trimmed rather than the call refused.
pub const MAX_RETAINED_CONVERSION_EVENTS: usize = 128;

/// The bytes of retained conversion-event payloads, which with
/// [`MAX_RETAINED_CONVERSION_EVENTS`] bounds the record whatever size the modelled
/// `1..1024000` blob arrives at. The newest event is always retained, even on its own.
pub const MAX_RETAINED_CONVERSION_EVENT_BYTES: usize = 8 * 1024 * 1024;

/// How many digits a minted dataset id carries.
///
/// The model's `WhatsAppDatasetId` is `len 10..20, pattern ^[0-9]+$` and documents a
/// Meta-generated "numeric string of 10 to 20 digits"; 15 sits inside that range at both
/// ends, so a minted id satisfies the constraint with room on either side.
const DATASET_ID_DIGITS: u32 = 15;

/// The message this service's `ResourceNotFoundException` carries.
///
/// The text is CONSTANT: nothing of the request reaches it, so a client's match on it does
/// not depend on what it asked for and the lookup key never reaches an error the service
/// keeps opaque. The model documents the error only as "The resource was not found."; this
/// text is the one the service's READS were recorded answering, identically for two
/// accounts named by 36- and 115-character ids, which is what shows it is not derived from
/// the request. Every not-found path takes a constant; the Conversions API's own is
/// [`DATASET_NOT_FOUND_MESSAGE`] and the phone-number lookups' is
/// [`ORIGINATION_PHONE_NUMBER_NOT_FOUND_MESSAGE`].
pub const RESOURCE_NOT_FOUND_MESSAGE: &str = "Requested resource not found.";

/// The message the PHONE-NUMBER lookup paths' `ResourceNotFoundException` carries.
///
/// A constant too, and the one that lookup was recorded answering for a phone number this
/// account has not registered: it names the KIND of key that resolved to no resource — an
/// origination phone number id — and nothing of the value itself, so it does not vary
/// with the request. The rule is the one [`RESOURCE_NOT_FOUND_MESSAGE`] states — a
/// not-found message is a constant, not a formatted one — with each lookup stating the
/// constant it was recorded with; every operation that resolves a phone-number id (the
/// phone-number read, the send) states this one.
pub const ORIGINATION_PHONE_NUMBER_NOT_FOUND_MESSAGE: &str =
    "Origination phone number id not found for account.";

/// The message the Conversions API's `ResourceNotFoundException` carries.
///
/// A constant too, and the one the Conversions API paths were recorded answering for a
/// `id` that names no linked account: the dataset create and the conversion event both
/// state it. Those paths state their own text rather than the one the account reads state
/// ([`RESOURCE_NOT_FOUND_MESSAGE`]), so the rule — a not-found message is a constant, not
/// a formatted one — holds with each path stating the constant it was recorded with.
pub const DATASET_NOT_FOUND_MESSAGE: &str = "Id not found for account.";

/// The message the DISASSOCIATION path's `ResourceNotFoundException` carries.
///
/// A constant too, and the one that path was recorded answering for an `id` that names no
/// linked account: it says the resource asked to be disassociated does not exist and
/// nothing of the id itself, so it does not vary with the request. Its text is NOT the one
/// the account reads state ([`RESOURCE_NOT_FOUND_MESSAGE`]) — the same lookup miss is
/// worded per operation — which is the distinction a caller reads to tell which lookup
/// failed.
pub const DISASSOCIATION_NOT_FOUND_MESSAGE: &str =
    "Resource requested for disassociation does not exist.";

/// The header the console's WhatsApp sign-up flow presents on the sign-up callback it
/// posts.
///
/// The sign-up operation is reached through that flow alone — its model doc says it "is
/// only used through the Amazon Web Services console during sign-up" — and a console
/// request carries the flow's own CSRF token in this header. A request made with anything
/// else does not carry it, which is the difference [`require_console_request`] decides on.
pub const CONSOLE_CSRF_HEADER: &str = "x-csrf-token";

/// The message a console-only operation refuses a request made outside the console with.
///
/// Constant, like every other error text this service states: the refusal is decided from
/// the absence of [`CONSOLE_CSRF_HEADER`] alone, so nothing of the request — no id, no
/// name, no value — reaches it.
pub const CSRF_HEADER_MISSING_MESSAGE: &str = "CSRF header does not exist";

/// Admit only the requests that arrive through the console flow a console-only operation
/// is served through.
///
/// A request carrying no [`CONSOLE_CSRF_HEADER`] — or carrying it empty, which is the same
/// absence as far as the flow is concerned — did not come through that flow, and is
/// refused with the 403 access-denied envelope that refusal is answered in
/// ([`crate::scaffolded::ConsoleAccessDenied`]). The decision is made from the REQUEST
/// alone, so an operation asks here BEFORE it reads or writes anything: a refused call
/// leaves the store exactly as it found it, records no signup and mints no token.
pub fn require_console_request(
    ctx: &smithy_server_runtime::OperationContext<'_>,
) -> Result<(), crate::scaffolded::ConsoleAccessDenied> {
    let through_the_console = ctx
        .parts()
        .headers
        .get(CONSOLE_CSRF_HEADER)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|token| !token.trim().is_empty());
    if through_the_console {
        return Ok(());
    }
    Err(crate::scaffolded::ConsoleAccessDenied::new(
        CSRF_HEADER_MISSING_MESSAGE,
    ))
}

/// The message this service refuses a request that hands it a role of ANOTHER account
/// with.
///
/// Constant, like every other error text this service states: neither the role, the
/// destination it would write to, nor the account the request named is formatted into it,
/// so the refusal says only that passing a role across an account boundary is not
/// permitted — which is the whole of the decision.
pub const CROSS_ACCOUNT_PASS_ROLE_MESSAGE: &str = "Cross-account pass role is not allowed.";

/// Refuse a request that hands this service an IAM role belonging to another account.
///
/// A role an operation accepts is one the SERVICE will assume on the caller's behalf, so
/// handing one over requires `iam:PassRole` on it and a principal can hold that only for a
/// role in its own account: a role named in another account is refused outright, with the
/// 403 `AccessDeniedException` the service models, whatever the rest of the request says.
/// The decision is made from the REQUEST alone — the account the role ARN names against
/// the calling account — so every operation that takes a role asks here BEFORE it resolves
/// any resource the request references and before it writes anything, and a caller that
/// may not pass the role never learns from the answer whether the resource exists.
///
/// `Ok` means only that no entry names another account: a destination with no role, and a
/// role whose account field cannot be read at all, are both left to the rest of the
/// handler, which is what refuses them.
///
/// The calling account is `engine::emulator_account_id()`: this emulator presents ONE
/// account to every caller, so the account a request is made in is that one and nothing
/// has to be read off the request's credentials. A later change that gave the emulator
/// more than one account would supply the caller's account here instead.
pub fn authorize_passed_roles(
    destinations: &[WhatsAppBusinessAccountEventDestination],
) -> Result<(), AccessDeniedException> {
    for destination in destinations {
        let Some(role_arn) = destination.role_arn.as_deref() else {
            continue;
        };
        // The tree's one ARN parse, so this service cannot disagree with another about
        // which account an ARN names.
        let account = ::service_common::arn::parse(role_arn)
            .map(|parsed| parsed.account)
            .filter(|account| !account.is_empty());
        let Some(account) = account else {
            continue;
        };
        if account != ::engine::emulator_account_id() {
            return Err(AccessDeniedException {
                message: Some(CROSS_ACCOUNT_PASS_ROLE_MESSAGE.to_string()),
            });
        }
    }
    Ok(())
}

/// A WhatsApp Business Account linked to this account, as
/// `AssociateWhatsAppBusinessAccount` stored it.
#[derive(Debug, Clone)]
pub struct LinkedAccount {
    /// The service-minted id, `waba-<32 hex>`.
    pub id: String,
    /// The service-minted ARN, `arn:aws:social-messaging:<region>:<account>:waba/<32 hex>`.
    pub arn: String,
    /// The WhatsApp Business Account id at Meta.
    pub waba_id: String,
    /// The account's display name.
    pub waba_name: String,
    /// One of [`REGISTRATION_STATUS_COMPLETE`] / [`REGISTRATION_STATUS_INCOMPLETE`].
    pub registration_status: String,
    /// When the account was linked.
    pub link_date: SystemTime,
    /// As last written by `PutWhatsAppBusinessAccountEventDestinations` (or at
    /// association time).
    pub event_destinations: Vec<WhatsAppBusinessAccountEventDestination>,
    /// The phone numbers registered under this account, in registration order.
    pub phone_number_ids: Vec<String>,
    /// The Meta Conversions API dataset, once one has been created.
    pub dataset_id: Option<String>,
    /// Meta's Marketing Messages onboarding status, when Meta reports one.
    pub marketing_messages_onboarding_status: Option<String>,
    /// Tags supplied at association time.
    pub tags: HashMap<String, String>,
}

/// A phone number registered under a linked account.
#[derive(Debug, Clone)]
pub struct PhoneNumberRecord {
    /// The service-minted id, `phone-number-id-<32 hex>`.
    pub phone_number_id: String,
    /// The service-minted ARN, `arn:aws:social-messaging:<region>:<account>:phone-number-id/<32 hex>`.
    pub arn: String,
    /// The [`LinkedAccount::id`] this number belongs to.
    pub linked_account_id: String,
    /// The E.164 number messages are sent from.
    pub phone_number: String,
    /// The phone number id at Meta, as the setup finalization named it.
    pub meta_phone_number_id: String,
    /// The number as it appears in a recipient's display.
    pub display_phone_number: String,
    /// The display name for this number.
    pub display_phone_number_name: String,
    /// Meta's quality rating for the number.
    pub quality_rating: String,
    /// The two-letter ISO region Meta stores this number's data in.
    pub data_localization_region: Option<String>,
    /// Call settings, once `UpdateLinkedWhatsAppBusinessAccountPhoneNumber` wrote some.
    pub call_settings: Option<WhatsAppCallSettings>,
    /// Tags supplied at registration time.
    pub tags: HashMap<String, String>,
}

/// A signup started by a `signupCallback` and not yet finalized.
#[derive(Debug, Clone)]
pub struct PendingSignup {
    /// The Meta-issued access token the callback carried.
    pub access_token: String,
    /// Where WhatsApp sends callback notifications for this signup.
    pub callback_url: Option<String>,
    /// When the callback arrived.
    pub created_at: SystemTime,
}

/// One accepted `SendWhatsAppMessage` call.
#[derive(Debug, Clone)]
pub struct SentMessage {
    /// The minted, per-call message id returned to the caller.
    pub message_id: String,
    /// The [`PhoneNumberRecord::phone_number_id`] the message was sent from.
    pub origination_phone_number_id: String,
    /// The message body exactly as the caller supplied it.
    pub message: Vec<u8>,
    /// The Meta Graph API version the caller asked for.
    pub meta_api_version: String,
    /// When the call was accepted.
    pub sent_at: SystemTime,
}

/// One accepted `SendWhatsAppConversionEvent` call.
#[derive(Debug, Clone)]
pub struct ConversionEvent {
    /// The minted, per-call `requestId` returned to the caller.
    pub request_id: String,
    /// The [`LinkedAccount::id`] the event was sent for.
    pub linked_account_id: String,
    /// The [`LinkedAccount::dataset_id`] the event was sent to.
    pub dataset_id: String,
    /// The Conversions API payload exactly as the caller supplied it.
    pub event_data: Vec<u8>,
    /// When the call was accepted.
    pub sent_at: SystemTime,
}

/// Whether a fresh store reports WhatsApp setup as enabled for the account.
///
/// `setupEnabled` is an account-level fact `ListLinkedWhatsAppBusinessAccounts` reports
/// beside the listing, and the recorded listing reports it `true` for an account with no
/// linked WhatsApp Business Account at all, so it is not derived from what the account
/// holds. No operation this service models enables or disables WhatsApp setup — it is
/// settled for the account before any of them is called — so the store starts from this
/// value and [`ComAmazonawsSocialmessagingSocialmessagingState::setup_enabled`] is what
/// the listing reads.
pub const SETUP_ENABLED_DEFAULT: bool = true;

/// Per-service state for com-amazonaws-socialmessaging-socialmessaging. A handler reaches it through [`service_state`].
#[derive(Debug)]
pub struct ComAmazonawsSocialmessagingSocialmessagingState {
    /// Linked accounts by [`LinkedAccount::id`].
    pub linked_accounts: HashMap<String, LinkedAccount>,
    /// Phone numbers by [`PhoneNumberRecord::phone_number_id`].
    pub phone_numbers: HashMap<String, PhoneNumberRecord>,
    /// Pending signups by their `associateInProgressToken`.
    pub pending_signups: HashMap<String, PendingSignup>,
    /// Every accepted send, oldest first. Bounded by [`MAX_RETAINED_SENT_MESSAGES`]
    /// and [`MAX_RETAINED_SENT_MESSAGE_BYTES`]; see [`Self::record_sent_message`].
    pub sent_messages: VecDeque<SentMessage>,
    /// The bytes of the message bodies `sent_messages` currently holds.
    pub retained_message_bytes: usize,
    /// Every accepted conversion event, oldest first. Bounded by
    /// [`MAX_RETAINED_CONVERSION_EVENTS`] and [`MAX_RETAINED_CONVERSION_EVENT_BYTES`];
    /// see [`Self::record_conversion_event`].
    pub conversion_events: VecDeque<ConversionEvent>,
    /// The bytes of the payloads `conversion_events` currently holds.
    pub retained_conversion_event_bytes: usize,
    /// ARN -> resource id, so an ARN-form `id` input resolves to the same record
    /// as the short form.
    pub arn_index: HashMap<String, String>,
    /// Whether WhatsApp setup is enabled for this account, as the listing reports it
    /// beside `linkedAccounts`. Starts at [`SETUP_ENABLED_DEFAULT`].
    pub setup_enabled: bool,
    /// Monotonic counter behind every minted id, so each one is unique.
    pub id_seq: u64,
}

/// A fresh store holds no resources and reports WhatsApp setup as
/// [`SETUP_ENABLED_DEFAULT`], which is the one field whose empty value is not its
/// type's default — hence the hand-written impl rather than a derive.
impl Default for ComAmazonawsSocialmessagingSocialmessagingState {
    fn default() -> Self {
        Self {
            linked_accounts: HashMap::new(),
            phone_numbers: HashMap::new(),
            pending_signups: HashMap::new(),
            sent_messages: VecDeque::new(),
            retained_message_bytes: 0,
            conversion_events: VecDeque::new(),
            retained_conversion_event_bytes: 0,
            arn_index: HashMap::new(),
            setup_enabled: SETUP_ENABLED_DEFAULT,
            id_seq: 0,
        }
    }
}

impl ComAmazonawsSocialmessagingSocialmessagingState {
    /// Whether the store already holds [`MAX_LINKED_ACCOUNTS`] accounts, so a
    /// further association must be refused instead of growing it.
    pub fn linked_accounts_full(&self) -> bool {
        self.linked_accounts.len() >= MAX_LINKED_ACCOUNTS
    }

    /// Drop every signup nobody finalized within [`PENDING_SIGNUP_TTL`]. This
    /// materialises what the clock already made due: each entry was owed its
    /// removal at the same instant whether or not anyone called.
    pub fn expire_pending_signups(&mut self, now: SystemTime) {
        self.pending_signups
            .retain(|_, signup| match now.duration_since(signup.created_at) {
                Ok(age) => age < PENDING_SIGNUP_TTL,
                // A `created_at` in the future (clock skew) is not yet expired.
                Err(_) => true,
            });
    }

    /// Whether [`MAX_PENDING_SIGNUPS`] signups are already in progress. Call
    /// [`Self::expire_pending_signups`] first, so only live ones count.
    pub fn pending_signups_full(&self) -> bool {
        self.pending_signups.len() >= MAX_PENDING_SIGNUPS
    }

    /// Record an accepted send, evicting the oldest records until the retained set
    /// is back inside [`MAX_RETAINED_SENT_MESSAGES`] and
    /// [`MAX_RETAINED_SENT_MESSAGE_BYTES`]. The send just accepted is always kept.
    pub fn record_sent_message(&mut self, message: SentMessage) {
        self.retained_message_bytes += message.message.len();
        self.sent_messages.push_back(message);
        while self.sent_messages.len() > MAX_RETAINED_SENT_MESSAGES
            || (self.retained_message_bytes > MAX_RETAINED_SENT_MESSAGE_BYTES
                && self.sent_messages.len() > 1)
        {
            match self.sent_messages.pop_front() {
                Some(evicted) => {
                    self.retained_message_bytes = self
                        .retained_message_bytes
                        .saturating_sub(evicted.message.len());
                }
                None => break,
            }
        }
    }

    /// Record an accepted conversion event, evicting the oldest records until the
    /// retained set is back inside [`MAX_RETAINED_CONVERSION_EVENTS`] and
    /// [`MAX_RETAINED_CONVERSION_EVENT_BYTES`]. The event just accepted is always kept.
    pub fn record_conversion_event(&mut self, event: ConversionEvent) {
        self.retained_conversion_event_bytes += event.event_data.len();
        self.conversion_events.push_back(event);
        while self.conversion_events.len() > MAX_RETAINED_CONVERSION_EVENTS
            || (self.retained_conversion_event_bytes > MAX_RETAINED_CONVERSION_EVENT_BYTES
                && self.conversion_events.len() > 1)
        {
            match self.conversion_events.pop_front() {
                Some(evicted) => {
                    self.retained_conversion_event_bytes = self
                        .retained_conversion_event_bytes
                        .saturating_sub(evicted.event_data.len());
                }
                None => break,
            }
        }
    }

    /// 32 lowercase hex characters, unique per call: the digest of `kind` and this
    /// store's monotonic sequence number. Distinct `kind`s never collide, so a
    /// linked-account suffix is never also a phone-number suffix.
    pub fn next_hex32(&mut self, kind: &str) -> String {
        self.id_seq += 1;
        let mut hasher = Sha256::new();
        hasher.update(kind.as_bytes());
        hasher.update(b":");
        hasher.update(self.id_seq.to_be_bytes());
        hex::encode(&hasher.finalize()[..16])
    }

    /// Mint the `(id, arn)` pair for a new linked account.
    pub fn mint_linked_account_id(&mut self, region: &str) -> (String, String) {
        let suffix = self.next_hex32("waba");
        let arn = ::engine::emulator_arn(ARN_SERVICE, region, &format!("waba/{suffix}"));
        (format!("{LINKED_ACCOUNT_ID_PREFIX}{suffix}"), arn)
    }

    /// Mint the `(id, arn)` pair for a newly registered phone number.
    pub fn mint_phone_number_id(&mut self, region: &str) -> (String, String) {
        let suffix = self.next_hex32("phone-number-id");
        let arn =
            ::engine::emulator_arn(ARN_SERVICE, region, &format!("phone-number-id/{suffix}"));
        (format!("{PHONE_NUMBER_ID_PREFIX}{suffix}"), arn)
    }

    /// Mint an `associateInProgressToken` (the model caps it at 50 characters).
    pub fn mint_signup_token(&mut self) -> String {
        format!("signup-{}", self.next_hex32("signup"))
    }

    /// Mint the message id `SendWhatsAppMessage` returns. Meta message ids carry
    /// the `wamid.` prefix.
    pub fn mint_message_id(&mut self) -> String {
        format!("wamid.{}", self.next_hex32("message"))
    }

    /// Mint the Meta Conversions API dataset id `CreateWhatsAppDataset` returns:
    /// [`DATASET_ID_DIGITS`] decimal digits with a non-zero lead, read off this store's
    /// own unique digest, so it satisfies `WhatsAppDatasetId` and two datasets never get
    /// the same id.
    pub fn mint_dataset_id(&mut self) -> String {
        let digest = self.next_hex32("dataset");
        // 15 hex characters are 60 bits, which a u64 holds.
        let value = u64::from_str_radix(&digest[..15], 16).unwrap_or(self.id_seq);
        let lead = 10_u64.pow(DATASET_ID_DIGITS - 1);
        (lead + value % (9 * lead)).to_string()
    }

    /// Mint the `requestId` `SendWhatsAppConversionEvent` returns: the 8-4-4-4-12 form a
    /// request id takes, over this store's own unique digest, so each accepted call
    /// reports its own id.
    pub fn mint_request_id(&mut self) -> String {
        let hex = self.next_hex32("conversion-event");
        format!(
            "{}-{}-{}-{}-{}",
            &hex[0..8],
            &hex[8..12],
            &hex[12..16],
            &hex[16..20],
            &hex[20..32]
        )
    }

    /// The stored linked account an `id` input names, accepting both the
    /// `waba-...` short form and the account ARN.
    pub fn resolve_linked_account(&self, id: &str) -> Option<&LinkedAccount> {
        self.linked_account_id(id)
            .and_then(|resolved| self.linked_accounts.get(&resolved))
    }

    /// The key under which the linked account an `id` input names is stored.
    pub fn linked_account_id(&self, id: &str) -> Option<String> {
        if self.linked_accounts.contains_key(id) {
            return Some(id.to_string());
        }
        self.arn_index
            .get(id)
            .filter(|resolved| self.linked_accounts.contains_key(resolved.as_str()))
            .cloned()
    }

    /// The stored phone number an `id` input names, accepting both the
    /// `phone-number-id-...` short form and the number's ARN.
    pub fn resolve_phone_number(&self, id: &str) -> Option<&PhoneNumberRecord> {
        self.phone_number_id(id)
            .and_then(|resolved| self.phone_numbers.get(&resolved))
    }

    /// The key under which the phone number an `id` input names is stored.
    pub fn phone_number_id(&self, id: &str) -> Option<String> {
        if self.phone_numbers.contains_key(id) {
            return Some(id.to_string());
        }
        self.arn_index
            .get(id)
            .filter(|resolved| self.phone_numbers.contains_key(resolved.as_str()))
            .cloned()
    }

    /// The tags of the resource `arn` names, or `None` when this store holds no
    /// resource under that ARN.
    ///
    /// Both resource kinds this service tags — a linked account and a registered
    /// phone number — keep their tags on their own record and are reachable through
    /// [`Self::arn_index`], so a tag read is over the SAME map the resource was
    /// created with and reports nothing a write did not put there.
    pub fn tags_for_arn(&self, arn: &str) -> Option<Vec<Tag>> {
        let id = self.arn_index.get(arn)?;
        let tags = self
            .linked_accounts
            .get(id)
            .map(|account| &account.tags)
            .or_else(|| self.phone_numbers.get(id).map(|number| &number.tags))?;
        Some(tag_list(tags))
    }

    /// Every linked account whose setup is not finished, keyed by its Meta
    /// account id — what `signupCallback` reports back to the console.
    pub fn accounts_with_incomplete_setup(
        &self,
    ) -> HashMap<String, LinkedWhatsAppBusinessAccountIdMetaData> {
        self.linked_accounts
            .values()
            .filter(|account| account.registration_status == REGISTRATION_STATUS_INCOMPLETE)
            .map(|account| {
                (
                    account.waba_id.clone(),
                    LinkedWhatsAppBusinessAccountIdMetaData {
                        account_name: Some(account.waba_name.clone()),
                        registration_status: Some(account.registration_status.clone()),
                        // An account with an unfinished setup has no registered
                        // number, so there is none to report here.
                        unregistered_whats_app_phone_numbers: None,
                        waba_id: Some(account.id.clone()),
                    },
                )
            })
            .collect()
    }

    /// The summaries of the numbers registered under `account`, in registration order.
    pub fn phone_number_summaries(
        &self,
        account: &LinkedAccount,
    ) -> Vec<WhatsAppPhoneNumberSummary> {
        account
            .phone_number_ids
            .iter()
            .filter_map(|id| self.phone_numbers.get(id))
            .map(PhoneNumberRecord::to_summary)
            .collect()
    }
}

/// An account's registration status: a linked account with no registered phone
/// number has not finished setting up.
pub fn registration_status_for(phone_number_count: usize) -> &'static str {
    if phone_number_count == 0 {
        REGISTRATION_STATUS_INCOMPLETE
    } else {
        REGISTRATION_STATUS_COMPLETE
    }
}

/// The `(E.164, display)` number pair for a Meta phone-number id.
///
/// The real service reports the number Meta holds against the id. This emulator
/// has no Meta, so it derives one in the North American range reserved for
/// fictional use — `+1 <area> 555-0100` through `555-0199` — from the id itself,
/// so every later read of the same registration reports the same number and two
/// different registrations get different numbers.
pub fn derive_phone_number(meta_phone_number_id: &str) -> (String, String) {
    let digest = Sha256::digest(meta_phone_number_id.as_bytes());
    // A NANP area code is 3 digits whose first is 2-9.
    let area = 200 + ((u32::from(digest[0]) << 3) | (u32::from(digest[1]) >> 5)) % 800;
    let line = u32::from(digest[2]) % 100;
    (
        format!("+1{area}55501{line:02}"),
        format!("+1 {area} 555-01{line:02}"),
    )
}

/// A modelled `TagList` as the key/value store a resource keeps.
///
/// A tag with no value is stored as an empty string, which is how a tag whose
/// value member is absent reads back.
pub fn tag_map(tags: Vec<Tag>) -> HashMap<String, String> {
    tags.into_iter()
        .map(|tag| (tag.key, tag.value.unwrap_or_default()))
        .collect()
}

/// A resource's stored tags as the modelled `TagList` a read reports.
///
/// Ordered by key, because a `HashMap` has no order of its own and two reads of one
/// unchanged resource report the same list.
pub fn tag_list(tags: &HashMap<String, String>) -> Vec<Tag> {
    let mut list: Vec<Tag> = tags
        .iter()
        .map(|(key, value)| Tag {
            key: key.clone(),
            value: Some(value.clone()),
        })
        .collect();
    list.sort_by(|left, right| left.key.cmp(&right.key));
    list
}

/// A stored instant as the wire timestamp the model's `timestamp` members carry.
pub fn to_date_time(at: SystemTime) -> ::aws_smithy_types::DateTime {
    let since_epoch = at
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    ::aws_smithy_types::DateTime::from_secs_f64(since_epoch.as_secs_f64())
}

/// The region this request was signed for, so a resource's ARN carries the region
/// the caller addressed. Falls back to [`DEFAULT_REGION`] for an unsigned request.
pub fn request_region(ctx: &smithy_server_runtime::OperationContext<'_>) -> String {
    ::emulator_server::sigv4_credential_scope(&ctx.parts().headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

impl PhoneNumberRecord {
    /// This record as the `WhatsAppPhoneNumberSummary` a linked account reports.
    pub fn to_summary(&self) -> WhatsAppPhoneNumberSummary {
        WhatsAppPhoneNumberSummary {
            arn: self.arn.clone(),
            phone_number: self.phone_number.clone(),
            phone_number_id: self.phone_number_id.clone(),
            meta_phone_number_id: self.meta_phone_number_id.clone(),
            display_phone_number_name: self.display_phone_number_name.clone(),
            display_phone_number: self.display_phone_number.clone(),
            quality_rating: self.quality_rating.clone(),
            data_localization_region: self.data_localization_region.clone(),
        }
    }

    /// This record as the `WhatsAppPhoneNumberDetail` the phone-number read returns.
    pub fn to_detail(&self) -> WhatsAppPhoneNumberDetail {
        WhatsAppPhoneNumberDetail {
            arn: self.arn.clone(),
            phone_number: self.phone_number.clone(),
            phone_number_id: self.phone_number_id.clone(),
            meta_phone_number_id: self.meta_phone_number_id.clone(),
            display_phone_number_name: self.display_phone_number_name.clone(),
            display_phone_number: self.display_phone_number.clone(),
            quality_rating: self.quality_rating.clone(),
            data_localization_region: self.data_localization_region.clone(),
        }
    }
}

impl LinkedAccount {
    /// This account as the summary the list operation returns.
    pub fn to_summary(&self) -> LinkedWhatsAppBusinessAccountSummary {
        LinkedWhatsAppBusinessAccountSummary {
            arn: self.arn.clone(),
            id: self.id.clone(),
            waba_id: self.waba_id.clone(),
            registration_status: self.registration_status.clone(),
            link_date: to_date_time(self.link_date),
            waba_name: self.waba_name.clone(),
            event_destinations: self.event_destinations.clone(),
            marketing_messages_onboarding_status: self
                .marketing_messages_onboarding_status
                .clone(),
            // No Direct Send enforcement is modelled, and the model omits these
            // members when none is active.
            direct_send_enforcement_status: None,
            direct_send_enforcement_status_expires_at: None,
            dataset_id: self.dataset_id.clone(),
        }
    }

    /// This account as the full `LinkedWhatsAppBusinessAccount` the read returns.
    pub fn to_account(
        &self,
        phone_numbers: Vec<WhatsAppPhoneNumberSummary>,
    ) -> LinkedWhatsAppBusinessAccount {
        LinkedWhatsAppBusinessAccount {
            arn: self.arn.clone(),
            id: self.id.clone(),
            waba_id: self.waba_id.clone(),
            registration_status: self.registration_status.clone(),
            link_date: to_date_time(self.link_date),
            waba_name: self.waba_name.clone(),
            event_destinations: self.event_destinations.clone(),
            marketing_messages_onboarding_status: self
                .marketing_messages_onboarding_status
                .clone(),
            direct_send_enforcement_status: None,
            direct_send_enforcement_status_expires_at: None,
            dataset_id: self.dataset_id.clone(),
            phone_numbers,
        }
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsSocialmessagingSocialmessagingState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsSocialmessagingSocialmessagingState>)
}
