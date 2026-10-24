// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-account-account service.
//!
//! The account record is this service's primary resource: `PutAccountName`,
//! `PutAlternateContact` and `PutContactInformation` write it,
//! `GetAccountInformation`, `GetAlternateContact` and `GetContactInformation`
//! report it, and `DeleteAlternateContact` removes one of its contacts. The
//! record below is the one copy every one of those handlers reads and writes.
//! It also carries the account's most recent primary email update, which
//! `StartPrimaryEmailUpdate` records, `AcceptPrimaryEmailUpdate` confirms and
//! `GetPrimaryEmailUpdateStatus` reports, the primary email address a confirmed
//! update put on the account once it completed, and the phone-number
//! verification of its primary contact, which `SendPhoneNumberVerification`
//! starts, `VerifyPhoneNumber` completes and `GetContactInformation` reports as
//! `VerificationStatus` ([`PhoneNumberVerification`]).
//!
//! Account Management operations all take an OPTIONAL `AccountId`: absent, the
//! call lands on "the Amazon Web Services account of the identity used to call
//! the operation" — which for this emulator is
//! [`::emulator_server::emulator_account_id`]. So a standalone call and a
//! management-account call that names that same id land on ONE record.
//!
//! The Region opt-in slice hangs off that same record:
//! [`Account::region_opt_status`] holds the opt status of every Region the
//! account has been opted into or out of, which `EnableRegion` and
//! `DisableRegion` write and `GetRegionOptStatus` and `ListRegions` report.
//! [`REGION_CATALOG`] is what decides whether a Region has an opt status at all
//! and what it reports before any opt call.

use std::collections::BTreeMap;
use std::time::{Duration, SystemTime};

use crate::ops::types::{
    AccessDeniedException, AlternateContact, ConflictException, ContactInformation,
    ResourceNotFoundException, ValidationException, ValidationExceptionField,
    ALTERNATE_CONTACT_TYPE_VALUES,
};

/// The `AccountState` an account this emulator knows carries.
///
/// `AccountState` is modelled `ACTIVE | CLOSED | PENDING_ACTIVATION | SUSPENDED`
/// (see `ACCOUNT_STATE_VALUES`). Closing or suspending an account is an
/// Organizations operation, not one this service binds, so an account that
/// exists here is active.
pub const ACTIVE_ACCOUNT_STATE: &str = "ACTIVE";

/// The `ValidationExceptionReason` a refused input member reports: one of that
/// enum's two values (`fieldValidationFailed | invalidRegionOptTarget`).
pub const FIELD_VALIDATION_FAILED: &str = "fieldValidationFailed";

/// The other `ValidationExceptionReason` value: the request named a Region that
/// cannot be the target of a Region opt-in or opt-out at all.
pub const INVALID_REGION_OPT_TARGET: &str = "invalidRegionOptTarget";

/// The `PhoneNumberVerificationStatus` the phone number of a stored primary
/// contact carries before any one-time-passcode flow has run on THAT number:
/// "UNVERIFIED – The phone number has not been verified"
/// (`PHONE_NUMBER_VERIFICATION_STATUS_VALUES` is
/// `NOT_SUPPORTED | PENDING | UNVERIFIED | VERIFIED`).
///
/// A phone number becomes `PENDING` when `SendPhoneNumberVerification` sends a
/// one-time passcode to it and `VERIFIED` when `VerifyPhoneNumber` is handed
/// that passcode back; see [`PhoneNumberVerification`].
pub const UNVERIFIED_PHONE_NUMBER: &str = "UNVERIFIED";

/// The `PhoneNumberVerificationStatus` of a phone number a one-time passcode has
/// been sent to and not yet submitted for: "PENDING – A one-time passcode has
/// been sent and is waiting to be submitted".
///
/// `PENDING` is not a status the CLOCK moves on: nothing advances it but a
/// `VerifyPhoneNumber` call carrying the passcode, so no due instant is stored
/// for it and a number nobody verifies stays `PENDING`.
pub const PENDING_PHONE_NUMBER_VERIFICATION: &str = "PENDING";

/// The `PhoneNumberVerificationStatus` of a phone number the one-time passcode
/// was submitted for: "VERIFIED – The phone number has been verified".
pub const VERIFIED_PHONE_NUMBER: &str = "VERIFIED";

/// How many characters a one-time passcode has: `Otp` is modelled
/// `^[a-zA-Z0-9]{6}$` (`smithy_query(query="operation",
/// operation="VerifyPhoneNumber")`), so a passcode this service mints has to be
/// six alphanumerics for the caller to be able to submit it at all.
pub const OTP_LENGTH: usize = 6;

/// A fresh one-time passcode: [`OTP_LENGTH`] hex characters, which satisfies
/// `Otp`'s modelled `^[a-zA-Z0-9]{6}$`.
///
/// The randomness is the `uuid` crate's, reached through `emulator_telemetry`'s
/// uuid-backed id newtype (`BatchId::random()` is a v4 UUID rendered
/// canonically), which this crate already depends on — nothing here is a
/// hand-rolled generator. A passcode is never derived from the request or from
/// the account, so two sends never produce the same one.
///
/// The passcode is `@sensitive` on the wire and is NEVER returned or logged:
/// AWS sends it to the phone number itself, and the only thing this service
/// reports is the resulting status.
pub fn new_otp() -> String {
    let mut passcode = String::with_capacity(OTP_LENGTH);
    while passcode.len() < OTP_LENGTH {
        passcode.extend(
            ::emulator_telemetry::BatchId::random()
                .as_str()
                .chars()
                .filter(char::is_ascii_alphanumeric),
        );
    }
    passcode.truncate(OTP_LENGTH);
    passcode
}

/// What a handler reports when the engine it was built on carries no store for
/// this service — a server fault, never a bad request.
pub const STORE_UNAVAILABLE: &str = "the account store is unavailable";

/// The name an account record carries from the moment it exists, before any
/// `PutAccountName` renames it.
///
/// Every Amazon Web Services account HAS a name: the name is set when the
/// account is created, and `GetAccountInformation` is documented to retrieve
/// "information about the specified account including its account name, account
/// ID, account creation date and time, and account state" — so no account ever
/// reports that member absent. An account record this emulator holds therefore
/// gets a name when it is created, not when a caller first writes one.
///
/// The value names the account by its id, which keeps it distinct per record and
/// inside `AccountName`'s modelled `@length(min: 1, max: 50)` and
/// `^[ -;=?-~]+$`: 8 characters of prefix plus a 12-digit id is 20, and every
/// byte of it (the ASCII letters, the space and the digits) is in the admitted
/// range.
pub fn initial_account_name(account_id: &str) -> String {
    format!("Account {account_id}")
}

/// How many account records the store holds before a call that would create a
/// NEW one is refused.
///
/// This is the emulator's OWN bound on its store, not a claim about any limit
/// AWS publishes: `AccountId` is only constrained to `^\d{12}$`, which bounds
/// the shape of a key and not the number of them, so without this bound a
/// caller could mint records without end by varying `AccountId`. It is set far
/// above any workflow an emulator serves, and it is checked only when a request
/// would ADD a record: every account already in the store stays writable.
pub const MAX_ACCOUNTS: usize = 1_000;

/// The `PrimaryEmailUpdateStatus` an update carries from the moment
/// `StartPrimaryEmailUpdate` records it: `PENDING`, one of that enum's four
/// values (`PRIMARY_EMAIL_UPDATE_STATUS_VALUES` in `ops::types`).
///
/// `PENDING` is not a status the CLOCK moves on: Account Management sends a
/// one-time passcode to the proposed address and the update advances only when a
/// caller presents that passcode through `AcceptPrimaryEmailUpdate`. So no due
/// instant is stored for it — waiting changes nothing, and an update nobody
/// accepts stays `PENDING`.
pub const PENDING_PRIMARY_EMAIL_UPDATE_STATUS: &str = "PENDING";

/// The `PrimaryEmailUpdateStatus` an update carries from the moment
/// `AcceptPrimaryEmailUpdate` confirms it: `ACCEPTED`, another of that enum's
/// four values.
///
/// `ACCEPTED` is NOT terminal — the request has been accepted and the address
/// change still has to be applied — so the clock owes the update its
/// [`COMPLETED_PRIMARY_EMAIL_UPDATE_STATUS`] after
/// [`PRIMARY_EMAIL_UPDATE_COMPLETION`].
pub const ACCEPTED_PRIMARY_EMAIL_UPDATE_STATUS: &str = "ACCEPTED";

/// The TERMINAL `PrimaryEmailUpdateStatus` of an accepted update: `COMPLETED`,
/// the status an update carries once the address it was started for is the
/// account's primary email.
pub const COMPLETED_PRIMARY_EMAIL_UPDATE_STATUS: &str = "COMPLETED";

/// How long this emulator takes to carry an accepted primary email update from
/// `ACCEPTED` to `COMPLETED`.
///
/// AWS publishes no duration for it, so this span is the emulator's OWN choice.
/// It is deliberately non-zero: a caller that reads the status straight after
/// accepting still observes `ACCEPTED`, and the state errors that go with it,
/// exactly as it would against AWS.
pub const PRIMARY_EMAIL_UPDATE_COMPLETION: Duration = Duration::from_millis(500);

/// The `RegionOptStatus` a Region that Amazon Web Services turns on for every
/// account carries: `ENABLED_BY_DEFAULT`, one of `RegionOptStatus`'s five values
/// (`REGION_OPT_STATUS_VALUES` in `ops::types`). Such a Region cannot be
/// disabled, so no account ever reports anything else for it.
pub const ENABLED_BY_DEFAULT_REGION_STATUS: &str = "ENABLED_BY_DEFAULT";

/// The `RegionOptStatus` an opt-in Region carries for an account that has not
/// enabled it: `DISABLED`. `EnableRegion` is what moves such a Region to
/// `ENABLING` and then `ENABLED`.
pub const DISABLED_REGION_STATUS: &str = "DISABLED";

/// Every Region of the commercial (`aws`) partition this emulator reports, with
/// the `RegionOptStatus` an account that has neither enabled nor disabled it
/// reports for that Region.
///
/// MEMBERSHIP is service data, not a modelled list: `RegionName` is a free
/// string constrained only to `@length(min: 1, max: 50)`
/// (`smithy_query(query="shape",shape="RegionName")`), so there is no modelled
/// list to project. SOURCE: the AWS General Reference's Region table
/// ("Regions, Availability Zones, and Local Zones") together with the opt-in
/// Regions the Account Management guide lists under "Specify which AWS Regions
/// your account can use" — the Regions a new account must enable before use. A
/// Region AWS turns on by default is `ENABLED_BY_DEFAULT` here; an opt-in Region
/// is `DISABLED` until `EnableRegion` says otherwise.
///
/// Regions of the GovCloud and China partitions are deliberately absent: this
/// emulator serves the commercial partition, which is the partition its
/// endpoints and its signing scope name.
///
/// Kept SORTED by Region code and free of duplicates: `ListRegions` reports it in
/// this order and pages through it by Region code, so a resume point is a
/// position in this sequence.
pub const REGION_CATALOG: &[(&str, &str)] = &[
    ("af-south-1", DISABLED_REGION_STATUS),
    ("ap-east-1", DISABLED_REGION_STATUS),
    ("ap-northeast-1", ENABLED_BY_DEFAULT_REGION_STATUS),
    ("ap-northeast-2", ENABLED_BY_DEFAULT_REGION_STATUS),
    ("ap-northeast-3", ENABLED_BY_DEFAULT_REGION_STATUS),
    ("ap-south-1", ENABLED_BY_DEFAULT_REGION_STATUS),
    ("ap-south-2", DISABLED_REGION_STATUS),
    ("ap-southeast-1", ENABLED_BY_DEFAULT_REGION_STATUS),
    ("ap-southeast-2", ENABLED_BY_DEFAULT_REGION_STATUS),
    ("ap-southeast-3", DISABLED_REGION_STATUS),
    ("ap-southeast-4", DISABLED_REGION_STATUS),
    ("ap-southeast-5", DISABLED_REGION_STATUS),
    ("ap-southeast-7", DISABLED_REGION_STATUS),
    ("ca-central-1", ENABLED_BY_DEFAULT_REGION_STATUS),
    ("ca-west-1", DISABLED_REGION_STATUS),
    ("eu-central-1", ENABLED_BY_DEFAULT_REGION_STATUS),
    ("eu-central-2", DISABLED_REGION_STATUS),
    ("eu-north-1", ENABLED_BY_DEFAULT_REGION_STATUS),
    ("eu-south-1", DISABLED_REGION_STATUS),
    ("eu-south-2", DISABLED_REGION_STATUS),
    ("eu-west-1", ENABLED_BY_DEFAULT_REGION_STATUS),
    ("eu-west-2", ENABLED_BY_DEFAULT_REGION_STATUS),
    ("eu-west-3", ENABLED_BY_DEFAULT_REGION_STATUS),
    ("il-central-1", DISABLED_REGION_STATUS),
    ("me-central-1", DISABLED_REGION_STATUS),
    ("me-south-1", DISABLED_REGION_STATUS),
    ("mx-central-1", DISABLED_REGION_STATUS),
    ("sa-east-1", ENABLED_BY_DEFAULT_REGION_STATUS),
    ("us-east-1", ENABLED_BY_DEFAULT_REGION_STATUS),
    ("us-east-2", ENABLED_BY_DEFAULT_REGION_STATUS),
    ("us-west-1", ENABLED_BY_DEFAULT_REGION_STATUS),
    ("us-west-2", ENABLED_BY_DEFAULT_REGION_STATUS),
];

/// The status [`REGION_CATALOG`] gives `region_name` before any account-specific
/// opt-in is recorded, or `None` for a Region code the catalog does not hold.
///
/// The catalog is sorted by Region code, so the lookup is a binary search over it.
pub fn catalog_region_status(region_name: &str) -> Option<&'static str> {
    REGION_CATALOG
        .binary_search_by(|(name, _)| (*name).cmp(region_name))
        .ok()
        .map(|index| REGION_CATALOG[index].1)
}

/// Which alternate contact a record is: the `AlternateContactType` enum, whose
/// wire values are `BILLING | OPERATIONS | SECURITY`
/// (`ALTERNATE_CONTACT_TYPE_VALUES` in `ops::types`).
///
/// `Ord` is what keys the per-account [`BTreeMap`], so the contacts of one
/// account iterate in a stable order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum AlternateContactType {
    Billing,
    Operations,
    Security,
}

impl AlternateContactType {
    /// The type a wire value names, or `None` for a value the enum does not
    /// admit.
    pub fn from_wire(value: &str) -> Option<Self> {
        match value {
            "BILLING" => Some(Self::Billing),
            "OPERATIONS" => Some(Self::Operations),
            "SECURITY" => Some(Self::Security),
            _ => None,
        }
    }

    /// The wire value of this type — what the `AlternateContact` structure
    /// reports back.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Billing => "BILLING",
            Self::Operations => "OPERATIONS",
            Self::Security => "SECURITY",
        }
    }
}

/// One alternate contact, exactly as `PutAlternateContact` stored it.
#[derive(Debug, Clone)]
pub struct AlternateContactRecord {
    pub name: String,
    pub title: String,
    pub email_address: String,
    pub phone_number: String,
    pub contact_type: AlternateContactType,
}

impl AlternateContactRecord {
    /// The `AlternateContact` structure `GetAlternateContact` returns: every
    /// member is the stored one.
    pub fn to_alternate_contact(&self) -> AlternateContact {
        AlternateContact {
            name: Some(self.name.clone()),
            title: Some(self.title.clone()),
            email_address: Some(self.email_address.clone()),
            phone_number: Some(self.phone_number.clone()),
            alternate_contact_type: Some(self.contact_type.as_wire().to_string()),
        }
    }
}

/// Which column of the Region catalog a Region code is in: whether the Region
/// is a Region-opt target at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegionKind {
    /// Available without opting in, so neither `EnableRegion` nor
    /// `DisableRegion` can act on it.
    EnabledByDefault,
    /// Disabled until the account opts in.
    OptIn,
}

/// Which kind of Region `region_name` is, read off [`REGION_CATALOG`], or `None`
/// for a value the catalog does not hold.
pub fn region_kind(region_name: &str) -> Option<RegionKind> {
    catalog_region_status(region_name).map(|status| {
        if status == ENABLED_BY_DEFAULT_REGION_STATUS {
            RegionKind::EnabledByDefault
        } else {
            RegionKind::OptIn
        }
    })
}

/// How long this emulator takes to carry a Region from a transient opt status
/// (`ENABLING`, `DISABLING`) to its terminal one (`ENABLED`, `DISABLED`).
///
/// AWS publishes no duration for it — `EnableRegion`'s `RegionName`
/// documentation says only that the process "takes a few minutes for most
/// accounts, but it can take several hours" — so this span is the emulator's
/// OWN choice. It is deliberately non-zero: a caller that reads the status
/// straight after the call still observes the transient status, and the state
/// errors that go with it, exactly as it would against AWS.
pub const REGION_OPT_TRANSITION: Duration = Duration::from_millis(500);

/// A Region's opt status for one account: the `RegionOptStatus` enum
/// (`DISABLED | DISABLING | ENABLED | ENABLED_BY_DEFAULT | ENABLING`).
///
/// A transient status carries the instant its terminal status is DUE, so the
/// transition is owed by the CLOCK rather than by whoever happens to call next:
/// [`settled`](Self::settled) reports the terminal status from that instant on,
/// whether or not anybody read or wrote in between.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegionOptState {
    /// Available without opting in: the status of every Region
    /// [`REGION_CATALOG`] marks [`ENABLED_BY_DEFAULT_REGION_STATUS`].
    EnabledByDefault,
    /// Opted in, with the opt-in not yet complete.
    Enabling { ready_at: SystemTime },
    /// Opted in and usable.
    Enabled,
    /// Opted out, with the opt-out not yet complete.
    Disabling { ready_at: SystemTime },
    /// Not opted in: the status of an opt-in Region the account has never
    /// enabled, and the one an opt-out settles to.
    Disabled,
}

impl RegionOptState {
    /// The wire value of this status, one of `REGION_OPT_STATUS_VALUES`.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::EnabledByDefault => ENABLED_BY_DEFAULT_REGION_STATUS,
            Self::Enabling { .. } => "ENABLING",
            Self::Enabled => "ENABLED",
            Self::Disabling { .. } => "DISABLING",
            Self::Disabled => DISABLED_REGION_STATUS,
        }
    }

    /// The status a wire `RegionOptStatus` value names, with a transient value
    /// settling at `ready_at`. `None` for a value the enum does not admit.
    pub fn from_wire(value: &str, ready_at: SystemTime) -> Option<Self> {
        match value {
            ENABLED_BY_DEFAULT_REGION_STATUS => Some(Self::EnabledByDefault),
            "ENABLING" => Some(Self::Enabling { ready_at }),
            "ENABLED" => Some(Self::Enabled),
            "DISABLING" => Some(Self::Disabling { ready_at }),
            DISABLED_REGION_STATUS => Some(Self::Disabled),
            _ => None,
        }
    }

    /// This status as of `now`: a transient status whose due instant has passed
    /// IS its terminal status from that instant on, and every other status is
    /// itself. A pure function of the record and the clock, so a reader reports
    /// the settled status without writing anything.
    pub fn settled(self, now: SystemTime) -> Self {
        match self {
            Self::Enabling { ready_at } if now >= ready_at => Self::Enabled,
            Self::Disabling { ready_at } if now >= ready_at => Self::Disabled,
            other => other,
        }
    }
}

/// The most recent primary email update of one account, as
/// `StartPrimaryEmailUpdate` recorded it and `GetPrimaryEmailUpdateStatus`
/// reports it.
#[derive(Debug, Clone)]
pub struct PrimaryEmailUpdate {
    /// The address the update was started for (`PrimaryEmailAddress`,
    /// `@length(min: 5, max: 64)` and `@sensitive`). Held because the update is
    /// about an address, and `AcceptPrimaryEmailUpdate` has to check the address
    /// a caller confirms against the one that was requested; it is never put in
    /// a response or a log by this service.
    pub requested_email: String,
    /// One of `PrimaryEmailUpdateStatus`'s values; see
    /// [`PENDING_PRIMARY_EMAIL_UPDATE_STATUS`].
    pub status: String,
    /// When this status was last set — what `GetPrimaryEmailUpdateStatus`
    /// reports as `UpdatedAt`.
    pub updated_at: SystemTime,
    /// The instant this update's `COMPLETED` status is DUE, set when
    /// `AcceptPrimaryEmailUpdate` moves it to `ACCEPTED`, and `None` for a
    /// status the clock does not move on (`PENDING`, and `COMPLETED` once it
    /// has been materialised).
    ///
    /// Held so the transition is owed by the CLOCK rather than by whoever
    /// happens to call next: [`settled_status`](Self::settled_status) reports
    /// the terminal status from that instant on, whether or not anybody read or
    /// wrote in between.
    pub completes_at: Option<SystemTime>,
}

impl PrimaryEmailUpdate {
    /// The update `StartPrimaryEmailUpdate` records at `now`: `PENDING`, because
    /// the address still has to be confirmed with the one-time passcode.
    pub fn started(requested_email: String, now: SystemTime) -> Self {
        Self {
            requested_email,
            status: PENDING_PRIMARY_EMAIL_UPDATE_STATUS.to_string(),
            updated_at: now,
            completes_at: None,
        }
    }

    /// Whether this update is still waiting for the one-time passcode, which is
    /// the one status `AcceptPrimaryEmailUpdate` can act on.
    pub fn is_pending(&self) -> bool {
        self.status == PENDING_PRIMARY_EMAIL_UPDATE_STATUS
    }

    /// Whether this update has been confirmed and its completion is owed: the
    /// address it names is on its way to being an account's primary email, so
    /// it already counts as in use.
    pub fn is_accepted(&self) -> bool {
        self.status == ACCEPTED_PRIMARY_EMAIL_UPDATE_STATUS
    }

    /// Confirm this update at `now`: it becomes `ACCEPTED`, and its `COMPLETED`
    /// status comes due [`PRIMARY_EMAIL_UPDATE_COMPLETION`] later.
    pub fn accept(&mut self, now: SystemTime) {
        self.status = ACCEPTED_PRIMARY_EMAIL_UPDATE_STATUS.to_string();
        self.updated_at = now;
        self.completes_at = Some(now + PRIMARY_EMAIL_UPDATE_COMPLETION);
    }

    /// This update's status as of `now`: an accepted update whose due instant
    /// has passed IS `COMPLETED` from that instant on, and every other status
    /// is the stored one. A pure function of the record and the clock, so a
    /// reader reports the settled status without writing anything.
    pub fn settled_status(&self, now: SystemTime) -> &str {
        if self.completes_at.is_some_and(|due| now >= due) {
            COMPLETED_PRIMARY_EMAIL_UPDATE_STATUS
        } else {
            &self.status
        }
    }

    /// When the status [`settled_status`](Self::settled_status) reports was set:
    /// the due instant for a completion the clock already owes, else the instant
    /// the stored status was written.
    pub fn settled_updated_at(&self, now: SystemTime) -> SystemTime {
        match self.completes_at {
            Some(due) if now >= due => due,
            _ => self.updated_at,
        }
    }

    /// Materialise the completion the clock already owes: an `ACCEPTED` update
    /// whose due instant is at or before `now` becomes `COMPLETED` in the
    /// store, and the address it was started for is answered back as the one
    /// the account's primary email becomes.
    ///
    /// Owed by the CLOCK, so this changes nothing a reader could not already
    /// see ([`settled_status`](Self::settled_status) reports the same status
    /// from the same instant without writing), and it is idempotent.
    pub fn settle(&mut self, now: SystemTime) -> Option<String> {
        let due = self.completes_at?;
        if now < due {
            return None;
        }
        self.status = COMPLETED_PRIMARY_EMAIL_UPDATE_STATUS.to_string();
        self.updated_at = due;
        self.completes_at = None;
        Some(self.requested_email.clone())
    }
}

/// Where one phone number got to in the one-time-passcode flow.
///
/// There is no third state: a number this emulator holds no record for is
/// `UNVERIFIED`, which is what [`Account::phone_number_verification_status`]
/// reports for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PhoneNumberVerificationState {
    /// `SendPhoneNumberVerification` sent a passcode and it has not been
    /// submitted yet. The passcode is held so `VerifyPhoneNumber` can check what
    /// the caller submits against it; it is `@sensitive` and never leaves the
    /// store.
    Pending { otp: String },
    /// The passcode was submitted and matched.
    Verified,
}

impl PhoneNumberVerificationState {
    /// The wire `PhoneNumberVerificationStatus` of this state, one of
    /// `PHONE_NUMBER_VERIFICATION_STATUS_VALUES`.
    pub fn as_wire(&self) -> &'static str {
        match self {
            Self::Pending { .. } => PENDING_PHONE_NUMBER_VERIFICATION,
            Self::Verified => VERIFIED_PHONE_NUMBER,
        }
    }
}

/// The phone-number verification of one account, as
/// `SendPhoneNumberVerification` recorded it and `VerifyPhoneNumber` advanced
/// it.
///
/// A verification is ABOUT a phone number, not about an account: AWS sends the
/// passcode to "the phone number in the primary contact information", so the
/// number it was sent to is stored with it and a record only speaks for the
/// number it names. Replace the primary contact's phone number and the stored
/// record no longer applies to it — see
/// [`Account::phone_number_verification_status`].
#[derive(Debug, Clone)]
pub struct PhoneNumberVerification {
    /// The primary-contact phone number the passcode was sent to.
    pub phone_number: String,
    /// How far that number got.
    pub state: PhoneNumberVerificationState,
    /// When this state was last set.
    pub updated_at: SystemTime,
}

impl PhoneNumberVerification {
    /// The record `SendPhoneNumberVerification` writes at `now`: a passcode is
    /// outstanding for `phone_number`, so the number is `PENDING`.
    pub fn sent(phone_number: String, otp: String, now: SystemTime) -> Self {
        Self {
            phone_number,
            state: PhoneNumberVerificationState::Pending { otp },
            updated_at: now,
        }
    }

    /// The passcode outstanding for this number, or `None` once it has been
    /// submitted: a one-time passcode is consumed by the call that verifies it.
    pub fn outstanding_otp(&self) -> Option<&str> {
        match &self.state {
            PhoneNumberVerificationState::Pending { otp } => Some(otp.as_str()),
            PhoneNumberVerificationState::Verified => None,
        }
    }
}

/// One Amazon Web Services account, as this emulator persists it.
#[derive(Debug, Clone)]
pub struct Account {
    /// The 12-digit account id this record is keyed by (`^\d{12}$`).
    pub account_id: String,
    /// The account's name: [`initial_account_name`] at creation, and whatever
    /// `PutAccountName` last stored after that. Never absent — an account
    /// always has a name (see [`initial_account_name`]).
    pub account_name: String,
    /// Set ONCE, when the record is first created, and never recomputed — so
    /// every later `GetAccountInformation` reports the same instant.
    pub created_date: SystemTime,
    /// One of `AccountState`'s values; see [`ACTIVE_ACCOUNT_STATE`].
    pub state: String,
    /// At most one contact per `AlternateContactType`, which is what makes
    /// `PutAlternateContact` an upsert rather than an append.
    pub alternate_contacts: BTreeMap<AlternateContactType, AlternateContactRecord>,
    /// The account's PRIMARY contact information, exactly as the last
    /// `PutContactInformation` stored it, and `None` until one has: this
    /// emulator learns an account's postal address only by being told it, and
    /// never invents one. There is at most one primary contact per account, so
    /// a second put replaces the whole structure rather than merging into it.
    pub contact_information: Option<ContactInformation>,
    /// The `RegionOptStatus` this account carries for a Region it has been opted
    /// into or out of, keyed by Region code. A Region ABSENT from this map still
    /// reports its [`REGION_CATALOG`] status, so the map holds only what an
    /// opt-in call changed: `EnableRegion` and `DisableRegion` are the
    /// operations that write it, and `ListRegions` and `GetRegionOptStatus`
    /// report it. A transient entry carries the instant its terminal status is
    /// due, so a reader settles it from the clock rather than from a write.
    pub region_opt_status: BTreeMap<String, RegionOptState>,
    /// The MOST RECENT primary email update of this account, or `None` while
    /// none has ever been started — which is what makes a status read of an
    /// account with no update a refusal rather than a reported status.
    pub primary_email_update: Option<PrimaryEmailUpdate>,
    /// The account's primary email address (the root user email address), and
    /// `None` until an update has COMPLETED on this record: this emulator
    /// learns an account's primary email only by being told it through the
    /// `StartPrimaryEmailUpdate` / `AcceptPrimaryEmailUpdate` flow, and never
    /// invents one.
    ///
    /// Written by [`settle_primary_email_update`](Self::settle_primary_email_update)
    /// when the accepted update completes, and read to decide whether an
    /// address is already in use — which is the conflict Account Management
    /// documents for a root user email change.
    pub primary_email: Option<String>,
    /// The MOST RECENT phone-number verification of this account, or `None`
    /// while no `SendPhoneNumberVerification` has ever sent a passcode. The
    /// record names the number the passcode went to, so it stops applying when
    /// the primary contact's phone number changes.
    pub phone_verification: Option<PhoneNumberVerification>,
}

impl Account {
    /// A new account record, created at `now`.
    pub fn new(account_id: String, now: SystemTime) -> Self {
        Self {
            account_name: initial_account_name(&account_id),
            account_id,
            created_date: now,
            state: ACTIVE_ACCOUNT_STATE.to_string(),
            alternate_contacts: BTreeMap::new(),
            contact_information: None,
            region_opt_status: BTreeMap::new(),
            primary_email_update: None,
            primary_email: None,
            phone_verification: None,
        }
    }

    /// Materialise the primary email update completion the clock already owes:
    /// an `ACCEPTED` update whose due instant is at or before `now` becomes
    /// `COMPLETED`, and the address it was started for becomes this account's
    /// [`primary_email`](Self::primary_email).
    ///
    /// Owed by the CLOCK and idempotent, exactly like
    /// [`settle_region_opts`](Self::settle_region_opts). The mutating
    /// primary-email operations call it before deciding, so the record they
    /// write over is the settled one.
    pub fn settle_primary_email_update(&mut self, now: SystemTime) {
        let completed = self
            .primary_email_update
            .as_mut()
            .and_then(|update| update.settle(now));
        if let Some(address) = completed {
            self.primary_email = Some(address);
        }
    }

    /// The phone number in this account's primary contact information, or `None`
    /// while no contact information is stored. `PhoneNumber` is `@required` on
    /// `ContactInformation`, so a stored contact always has one.
    pub fn primary_phone_number(&self) -> Option<&str> {
        self.contact_information
            .as_ref()
            .map(|contact| contact.phone_number.as_str())
    }

    /// The `PhoneNumberVerificationStatus` this account reports for the phone
    /// number currently in its primary contact information.
    ///
    /// A verification speaks only for the number it was sent to, so a stored
    /// record is honoured only while that number is still the primary contact's
    /// — otherwise, and with no record at all, the number is
    /// [`UNVERIFIED_PHONE_NUMBER`]. A pure function of the record: a reporting
    /// operation answers from it without writing anything.
    ///
    /// `NOT_SUPPORTED` ("phone number verification isn't available for this
    /// account") is never reported: every account this emulator holds can run
    /// the passcode flow.
    pub fn phone_number_verification_status(&self) -> &'static str {
        let Some(phone_number) = self.primary_phone_number() else {
            return UNVERIFIED_PHONE_NUMBER;
        };
        match &self.phone_verification {
            Some(verification) if verification.phone_number == phone_number => {
                verification.state.as_wire()
            }
            _ => UNVERIFIED_PHONE_NUMBER,
        }
    }

    /// This account's verification record, but only while it speaks for the
    /// phone number currently in the primary contact information — what
    /// `VerifyPhoneNumber` checks a submitted passcode against.
    pub fn current_phone_verification_mut(&mut self) -> Option<&mut PhoneNumberVerification> {
        let phone_number = self.primary_phone_number()?.to_string();
        self.phone_verification
            .as_mut()
            .filter(|verification| verification.phone_number == phone_number)
    }

    /// Materialise every Region opt transition the clock already owes: an
    /// `ENABLING` or `DISABLING` Region whose due instant is at or before `now`
    /// becomes `ENABLED` or `DISABLED` in the store.
    ///
    /// Owed by the CLOCK, so this changes nothing a reader could not already
    /// see — [`RegionOptState::settled`] reports the same status from the same
    /// instant without writing — and it is idempotent. The mutating opt
    /// operations call it before deciding, so the record they write over is the
    /// settled one.
    pub fn settle_region_opts(&mut self, now: SystemTime) {
        for status in self.region_opt_status.values_mut() {
            *status = status.settled(now);
        }
    }

    /// This account's opt status for the Region `region_name` of kind `kind`,
    /// as of `now`.
    ///
    /// A default-enabled Region reports `ENABLED_BY_DEFAULT` whatever the store
    /// holds, and an opt-in Region the account never acted on reports
    /// `DISABLED`. A transient stored status settles from the clock
    /// ([`RegionOptState::settled`]), so the answer never depends on whether
    /// anybody wrote the terminal status back.
    pub fn region_opt_state(
        &self,
        region_name: &str,
        kind: RegionKind,
        now: SystemTime,
    ) -> RegionOptState {
        match kind {
            RegionKind::EnabledByDefault => RegionOptState::EnabledByDefault,
            RegionKind::OptIn => self
                .region_opt_status
                .get(region_name)
                .copied()
                .unwrap_or(RegionOptState::Disabled)
                .settled(now),
        }
    }

    /// The `RegionOptStatus` this account reports for `region_name` as of `now`:
    /// the status an opt-in call stored for it, settled, else its
    /// [`REGION_CATALOG`] status. `None` for a Region code the catalog does not
    /// hold.
    ///
    /// A Region AWS turns on by default is never opted out of, so a stored
    /// status is honoured only where the catalog admits one — which keeps
    /// `ENABLED_BY_DEFAULT` the one status such a Region can report.
    pub fn reported_region_status(
        &self,
        region_name: &str,
        now: SystemTime,
    ) -> Option<&'static str> {
        let kind = region_kind(region_name)?;
        Some(self.region_opt_state(region_name, kind, now).as_wire())
    }

    /// Every Region in [`REGION_CATALOG`], in catalog order, with the status this
    /// account reports for it as of `now` — what `ListRegions` projects.
    pub fn reported_regions(&self, now: SystemTime) -> Vec<(&'static str, &'static str)> {
        REGION_CATALOG
            .iter()
            .filter_map(|(name, _)| {
                self.reported_region_status(name, now)
                    .map(|status| (*name, status))
            })
            .collect()
    }
}

/// Per-service state for com-amazonaws-account-account. A handler reaches it through [`service_state`].
#[derive(Debug)]
pub struct ComAmazonawsAccountAccountState {
    /// Every account this emulator knows, keyed by its 12-digit id. The
    /// caller's own account ([`default_account_id`]) is always present.
    pub accounts: BTreeMap<String, Account>,
}

/// The store starts holding the caller's OWN account: that account exists
/// before any call is made, so a reporting operation never has to create it.
/// Its `created_date` is fixed here, at the one instant the store comes up.
impl Default for ComAmazonawsAccountAccountState {
    fn default() -> Self {
        let own = default_account_id().to_string();
        let mut accounts = BTreeMap::new();
        accounts.insert(own.clone(), Account::new(own, SystemTime::now()));
        Self { accounts }
    }
}

impl ComAmazonawsAccountAccountState {
    /// The account `account_id` names, for a REPORTING operation: no record is
    /// created, so a read leaves the store exactly as it found it.
    pub fn account(&self, account_id: &str) -> Option<&Account> {
        self.accounts.get(account_id)
    }

    /// The account `account_id` names, for a MUTATING operation, creating the
    /// record on first touch.
    ///
    /// An account id reaching here satisfies `^\d{12}$` (the boundary refuses
    /// anything else), and writing to a member account is what the optional
    /// `AccountId` member is for — so the accounts this emulator knows are
    /// exactly the ones it has been told about, up to [`MAX_ACCOUNTS`].
    ///
    /// `None` when the id names no record and the store is already at that
    /// bound: no record is created and nothing is written.
    pub fn account_mut(&mut self, account_id: &str, now: SystemTime) -> Option<&mut Account> {
        if !self.accounts.contains_key(account_id) && self.accounts.len() >= MAX_ACCOUNTS {
            return None;
        }
        Some(
            self.accounts
                .entry(account_id.to_string())
                .or_insert_with(|| Account::new(account_id.to_string(), now)),
        )
    }

    /// Materialise every primary email update completion the clock already owes,
    /// across every account — see
    /// [`Account::settle_primary_email_update`].
    pub fn settle_primary_email_updates(&mut self, now: SystemTime) {
        for account in self.accounts.values_mut() {
            account.settle_primary_email_update(now);
        }
    }

    /// Whether `address` is already in use as an account's primary email
    /// address: either an account holds it, or another account's update is
    /// ACCEPTED for it, so its completion is already owed.
    ///
    /// Settle first ([`settle_primary_email_updates`](Self::settle_primary_email_updates)):
    /// an address a completed update put on an account is in use from the
    /// instant that completion came due. An address a PENDING update merely
    /// asked for is NOT in use — nothing has been changed for it yet, and the
    /// passcode it waits for may never arrive.
    pub fn primary_email_in_use(&self, address: &str) -> bool {
        self.accounts.values().any(|account| {
            account.primary_email.as_deref() == Some(address)
                || account
                    .primary_email_update
                    .as_ref()
                    .is_some_and(|update| update.is_accepted() && update.requested_email == address)
        })
    }

    /// The account `account_id` names, for a mutating operation that must NOT
    /// bring a record into existence: `None` when this emulator holds no record
    /// of that account, and nothing is written.
    ///
    /// What the phone-number passcode operations need: they act on the primary
    /// contact information of an account that already exists, so an id naming no
    /// record is a refusal rather than a new account.
    pub fn existing_account_mut(&mut self, account_id: &str) -> Option<&mut Account> {
        self.accounts.get_mut(account_id)
    }
}

/// The account a call lands on when its `AccountId` member is absent: "the
/// Amazon Web Services account of the identity used to call the operation".
pub fn default_account_id() -> &'static str {
    ::engine::emulator_account_id()
}

/// The account id a request operates on: the one it names, else the caller's own.
pub fn resolve_account_id(requested: Option<&str>) -> String {
    requested
        .map(str::to_string)
        .unwrap_or_else(|| default_account_id().to_string())
}

/// The `AccessDeniedException` payload for an account the caller may not read.
///
/// Naming another account requires the caller to be an identity in the
/// organization's management account, with the named account a member of the
/// same organization — which an account this emulator holds no record of is not.
pub fn account_not_accessible(account_id: &str) -> AccessDeniedException {
    AccessDeniedException {
        message: format!(
            "The operation failed because the calling identity doesn't have the minimum \
             required permissions for account {account_id}"
        ),
        error_type: None,
    }
}

/// The `AccessDeniedException` payload for a write that would add an account
/// past [`MAX_ACCOUNTS`]: the emulator admits no further records, so the write
/// is refused rather than stored.
pub fn account_store_full(account_id: &str) -> AccessDeniedException {
    AccessDeniedException {
        message: format!(
            "The operation failed because this emulator already holds the maximum of \
             {MAX_ACCOUNTS} account records and cannot add account {account_id}"
        ),
        error_type: None,
    }
}

/// The one sentence Account Management states for an alternate contact that is
/// not set.
///
/// INVARIANT OF THE REQUEST: it names neither the `AlternateContactType` asked
/// for nor the account the call resolved to, so every contact type and both the
/// `GetAlternateContact` and the `DeleteAlternateContact` path state the
/// identical literal — and no account identifier is surfaced in an error body.
pub const ALTERNATE_CONTACT_NOT_FOUND_MESSAGE: &str =
    "No contact of the inputted alternate contact type found.";

/// The `ResourceNotFoundException` payload for an alternate contact that is not
/// set — "the operation failed because it specified a resource that can't be
/// found".
///
/// Takes no argument on purpose: see [`ALTERNATE_CONTACT_NOT_FOUND_MESSAGE`].
pub fn alternate_contact_not_found() -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: ALTERNATE_CONTACT_NOT_FOUND_MESSAGE.to_string(),
        error_type: None,
    }
}

/// The `ValidationException` payload for a primary-email call on an account that
/// has never had an update started: there is no update to report or to accept.
///
/// `GetPrimaryEmailUpdateStatus` reports "the status of the MOST RECENT primary
/// email update" and models `Status` as REQUIRED, so an account with no update
/// has no status to report and the call is refused rather than answered with an
/// invented status; `AcceptPrimaryEmailUpdate` accepts "the request that
/// originated from StartPrimaryEmailUpdate", so with no such request there is
/// nothing it can accept. The modelled refusal is the 400 `ValidationException`; the
/// `reason` member is left absent because neither
/// `ValidationExceptionReason` value (`fieldValidationFailed |
/// invalidRegionOptTarget`) describes it — no input member was invalid.
pub fn no_primary_email_update() -> ValidationException {
    ValidationException {
        message: "No email update found for account".to_string(),
        reason: None,
        field_list: None,
    }
}

/// The `ConflictException` payload for an `AcceptPrimaryEmailUpdate` whose
/// update is not waiting for its one-time passcode: only a `PENDING` update can
/// be accepted, and "the request could not be processed because of a conflict in
/// the current status of the resource".
///
/// The message names the status the update IS in, which is what a caller needs
/// to tell "already accepted" from "nothing in flight"; it names no address,
/// because `PrimaryEmailAddress` is `@sensitive`.
pub fn primary_email_update_not_pending(status: &str) -> ConflictException {
    ConflictException {
        message: format!(
            "The primary email update is in status {status}; only a \
             {PENDING_PRIMARY_EMAIL_UPDATE_STATUS} update can be accepted"
        ),
        error_type: None,
    }
}

/// The `ValidationException` payload for an `AcceptPrimaryEmailUpdate` whose
/// `PrimaryEmail` is not the address the update was started for.
///
/// The rule is the model's own `PrimaryEmail` documentation: "This must match
/// the PrimaryEmail from the StartPrimaryEmailUpdate API call." `reason` is
/// `fieldValidationFailed` and `fieldList` names the member, so a caller can see
/// which input was refused — and neither the presented nor the requested address
/// is echoed, because `PrimaryEmailAddress` is `@sensitive`.
pub fn primary_email_does_not_match_request() -> ValidationException {
    const RULE: &str =
        "PrimaryEmail must match the PrimaryEmail from the StartPrimaryEmailUpdate API call";
    ValidationException {
        message: RULE.to_string(),
        reason: Some(FIELD_VALIDATION_FAILED.to_string()),
        field_list: Some(vec![ValidationExceptionField {
            name: "PrimaryEmail".to_string(),
            message: RULE.to_string(),
        }]),
    }
}

/// The `ConflictException` payload for a primary email change to an address that
/// is already in use — the conflict this service's own `ConflictException`
/// documentation gives as its example: "you try to change an account's root user
/// email to an email address which is already in use".
///
/// It names no address: `PrimaryEmailAddress` is `@sensitive`, and the address
/// in use belongs to whichever account already holds it.
pub fn primary_email_already_in_use() -> ConflictException {
    ConflictException {
        message: "The request could not be processed because the email address is already in \
                  use as the primary email address of an account"
            .to_string(),
        error_type: None,
    }
}

/// The `AccessDeniedException` payload for a primary email update operation
/// (`StartPrimaryEmailUpdate`, `AcceptPrimaryEmailUpdate`) whose `AccountId` is
/// the caller's own account.
///
/// The model's own `AccountId` documentation states the rule: the operation "can
/// only be called from the management account or the delegated administrator
/// account of an organization for a member account", and "the management account
/// can't specify its own `AccountId`". The message names no account id, so it is
/// the same sentence whichever account the caller is.
pub fn cannot_target_own_account() -> AccessDeniedException {
    AccessDeniedException {
        message: "The operation failed because the management account can't specify its own \
                  AccountId; this operation updates a member account of the organization"
            .to_string(),
        error_type: None,
    }
}

/// The `ResourceNotFoundException` payload for an account whose primary contact
/// information this emulator holds no record of — "the operation failed because
/// it specified a resource that can't be found".
pub fn contact_information_not_set(account_id: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: format!("No primary contact information is set for account {account_id}"),
        error_type: None,
    }
}

/// The `ValidationException` payload for an `AlternateContactType` value the
/// modelled enum does not admit.
///
/// `reason` is `fieldValidationFailed` (one of `ValidationExceptionReason`'s two
/// values) and `fieldList` names the member, so a client can see which input was
/// invalid and why.
pub fn invalid_alternate_contact_type(value: &str) -> ValidationException {
    let admitted = ALTERNATE_CONTACT_TYPE_VALUES.join(" | ");
    ValidationException {
        message: format!("AlternateContactType must be one of {admitted}"),
        reason: Some(FIELD_VALIDATION_FAILED.to_string()),
        field_list: Some(vec![ValidationExceptionField {
            name: "AlternateContactType".to_string(),
            message: format!("{value:?} is not one of {admitted}"),
        }]),
    }
}

/// The `ValidationException` payload for a `RegionName` that is not a Region
/// code of this partition.
///
/// `RegionName` is modelled only `@length(min: 1, max: 50)`, so the boundary
/// admits any string of that length: WHICH strings name a Region is a rule this
/// service enforces itself. `reason` is `fieldValidationFailed` and `fieldList`
/// names the member, so the caller can see which input was rejected.
pub fn unknown_region(region_name: &str) -> ValidationException {
    ValidationException {
        message: format!("RegionName {region_name:?} is not an Amazon Web Services Region code"),
        reason: Some(FIELD_VALIDATION_FAILED.to_string()),
        field_list: Some(vec![ValidationExceptionField {
            name: "RegionName".to_string(),
            message: format!("{region_name:?} is not an Amazon Web Services Region code"),
        }]),
    }
}

/// The `ValidationException` payload for a Region that cannot be opted into or
/// out of: it is enabled by default, so it is not a Region-opt target.
///
/// `reason` is `invalidRegionOptTarget` — the `ValidationExceptionReason` value
/// that exists for exactly this refusal.
pub fn region_not_opt_target(region_name: &str) -> ValidationException {
    ValidationException {
        message: format!(
            "Region {region_name} is enabled by default and cannot be enabled or disabled"
        ),
        reason: Some(INVALID_REGION_OPT_TARGET.to_string()),
        field_list: None,
    }
}

/// The `ConflictException` payload for a Region-opt call that conflicts with
/// the Region's current status — enabling one that is `DISABLING`, or disabling
/// one that is `ENABLING`, neither of which can be reversed before the status
/// in flight has settled.
pub fn region_opt_in_progress(region_name: &str, status: &str) -> ConflictException {
    ConflictException {
        message: format!(
            "Region {region_name} is in status {status}; wait for that operation to complete \
             before changing its opt status again"
        ),
        error_type: None,
    }
}

/// The `ConflictException` payload for a `VerifyPhoneNumber` on a phone number
/// that has no outstanding one-time passcode: the request conflicts with the
/// current status of the resource, which is what that error is modelled for.
///
/// `status` is the `PhoneNumberVerificationStatus` the number carries instead of
/// `PENDING` — `UNVERIFIED` when no passcode was ever sent to it (or the primary
/// contact's number changed since), `VERIFIED` when the passcode was already
/// submitted and consumed. No passcode is named: `Otp` is `@sensitive`.
pub fn no_outstanding_passcode(status: &str) -> ConflictException {
    ConflictException {
        message: format!(
            "The phone number in the primary contact information is in status {status}; no \
             one-time passcode is waiting to be submitted. Call SendPhoneNumberVerification \
             to send one"
        ),
        error_type: None,
    }
}

/// The `ValidationException` payload for a `VerifyPhoneNumber` whose `Otp` is
/// not the outstanding passcode.
///
/// WHICH six alphanumerics are the passcode is not something the model can
/// express, so the boundary admits any `^[a-zA-Z0-9]{6}$` and this service
/// refuses the wrong one itself: `reason` is `fieldValidationFailed` and
/// `fieldList` names the member, so a client can see which input was invalid.
/// Neither the submitted nor the outstanding passcode is echoed — `Otp` is
/// `@sensitive`.
pub fn incorrect_passcode() -> ValidationException {
    ValidationException {
        message: "The one-time passcode submitted in Otp is not the passcode sent to the phone \
                  number in the primary contact information"
            .to_string(),
        reason: Some(FIELD_VALIDATION_FAILED.to_string()),
        field_list: Some(vec![ValidationExceptionField {
            name: "Otp".to_string(),
            message: "the submitted one-time passcode is incorrect".to_string(),
        }]),
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsAccountAccountState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsAccountAccountState>)
}
