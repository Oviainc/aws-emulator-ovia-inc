// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-account-account service.
//!
//! The account record is this service's primary resource: `PutAccountName` and
//! `PutAlternateContact` write it, `GetAccountInformation` and
//! `GetAlternateContact` report it, and `DeleteAlternateContact` removes one of
//! its contacts. The record below is the one copy every one of those handlers
//! reads and writes.
//!
//! Account Management operations all take an OPTIONAL `AccountId`: absent, the
//! call lands on "the Amazon Web Services account of the identity used to call
//! the operation" — which for this emulator is
//! [`::emulator_server::emulator_account_id`]. So a standalone call and a
//! management-account call that names that same id land on ONE record.

use std::collections::BTreeMap;
use std::time::SystemTime;

use crate::ops::types::{
    AccessDeniedException, AlternateContact, ResourceNotFoundException, ValidationException,
    ValidationExceptionField, ALTERNATE_CONTACT_TYPE_VALUES,
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
    /// The `RegionOptStatus` this account carries for a Region it has been opted
    /// into or out of, keyed by Region code. A Region ABSENT from this map still
    /// reports its [`REGION_CATALOG`] status, so the map holds only what an
    /// opt-in call changed: `EnableRegion` and `DisableRegion` are the
    /// operations that write it (each still scaffolded, with its own stub
    /// issue), and `ListRegions` and `GetRegionOptStatus` report it.
    pub region_opt_status: BTreeMap<String, String>,
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
            region_opt_status: BTreeMap::new(),
        }
    }

    /// The `RegionOptStatus` this account reports for `region_name`: the status
    /// an opt-in call stored for it, else its [`REGION_CATALOG`] status. `None`
    /// for a Region code the catalog does not hold.
    ///
    /// A Region AWS turns on by default is never opted out of, so a stored
    /// status is honoured only where the catalog admits one — which keeps
    /// `ENABLED_BY_DEFAULT` the one status such a Region can report.
    pub fn reported_region_status(&self, region_name: &str) -> Option<&str> {
        let catalog = catalog_region_status(region_name)?;
        if catalog == ENABLED_BY_DEFAULT_REGION_STATUS {
            return Some(ENABLED_BY_DEFAULT_REGION_STATUS);
        }
        Some(
            self.region_opt_status
                .get(region_name)
                .map(String::as_str)
                .unwrap_or(catalog),
        )
    }

    /// Every Region in [`REGION_CATALOG`], in catalog order, with the status this
    /// account reports for it — what `ListRegions` projects.
    pub fn reported_regions(&self) -> Vec<(&'static str, &str)> {
        REGION_CATALOG
            .iter()
            .filter_map(|(name, _)| {
                self.reported_region_status(name)
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

/// The `ResourceNotFoundException` payload for an alternate contact that is not
/// set — "the operation failed because it specified a resource that can't be
/// found".
pub fn alternate_contact_not_found(
    account_id: &str,
    contact_type: &str,
) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: format!(
            "No {contact_type} alternate contact is set for account {account_id}"
        ),
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
