// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-taxsettings-taxsettings service.
//!
//! It holds the tax-registration (TRN) record of each account and the account's
//! tax-inheritance election, plus the projections every reader of a stored
//! record needs. The projections live here, beside the stored record, so
//! `GetTaxRegistration` and `ListTaxRegistrations` report one record the same
//! way instead of each rebuilding it from the raw fields.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::ops::Bound;
use std::time::{Duration, Instant, SystemTime};

use ::aws_smithy_types::base64;

use crate::ops::types::{
    AccountMetaData, AdditionalInfoRequest, AdditionalInfoResponse, Address, Authority,
    CaseCreationLimitExceededException, ConflictException, IndonesiaAdditionalInfo,
    InternalServerException, Jurisdiction, ResourceNotFoundException, SupplementalTaxRegistration,
    SupplementalTaxRegistrationEntry, TaxDocumentMetadata, TaxExemption, TaxExemptionDetails,
    TaxExemptionType, TaxRegistration, TaxRegistrationEntry, TaxRegistrationWithJurisdiction,
    ValidationException, ValidationExceptionField,
};

/// The `TaxRegistrationStatus` values, from the model's enum
/// (`Verified | Pending | Deleted | Rejected`). The model documents the value as
/// the outcome of validating the submitted TRN: `PutTaxRegistration`'s `status`
/// is "the status of your TRN stored in the system after processing. Based on the
/// validation occurring on the TRN, the status can be Verified, Pending or
/// Rejected."
pub const STATUS_PENDING: &str = "Pending";
/// See [`STATUS_PENDING`]. Never reached: a TRN is verified by checking it
/// against the tax authority's registry, and there is none here — see
/// [`SETTLED_REGISTRATION_STATUS`].
pub const STATUS_VERIFIED: &str = "Verified";
/// See [`STATUS_PENDING`].
pub const STATUS_REJECTED: &str = "Rejected";
/// See [`STATUS_PENDING`]. Never reached: `DeleteTaxRegistration` REMOVES the
/// account's record, which is what the real service does too — a read of a
/// deleted registration answers `ResourceNotFoundException`, not a registration
/// in this status.
pub const STATUS_DELETED: &str = "Deleted";

/// The status a submitted TRN reaches once the validation the write started has
/// run.
///
/// Validating a TRN means checking the number against the tax authority's
/// registry, and this emulator has no registry to check one against: no number it
/// is given can be VERIFIED, whatever its shape, so the validation ends in the
/// status AWS gives a TRN whose validation did not succeed. Nothing here inspects
/// the submitted number — a format test would report a verification that never
/// happened.
pub const SETTLED_REGISTRATION_STATUS: &str = STATUS_REJECTED;

/// How long after a write the TRN's validation is due.
///
/// AWS documents no span for it, so this is the emulator's own, inside the
/// project's 250-750 ms band for a transition the clock owes: it is never zero, so
/// a caller that reads the registration straight after writing it still sees the
/// `Pending` the write reported, and it is short enough that a caller polling for
/// a terminal status gets one.
pub const TRN_VALIDATION_DELAY: Duration = Duration::from_millis(500);

/// The `EntityExemptionAccountStatus` values, from the model's enum
/// (`None | Valid | Expired | Pending`), which is what `TaxExemption$status`
/// reports for one exemption of an account.
pub const EXEMPTION_STATUS_NONE: &str = "None";
/// See [`EXEMPTION_STATUS_NONE`]: the exemption is in force.
pub const EXEMPTION_STATUS_VALID: &str = "Valid";
/// See [`EXEMPTION_STATUS_NONE`]: the exemption's expiration date has passed.
pub const EXEMPTION_STATUS_EXPIRED: &str = "Expired";
/// See [`EXEMPTION_STATUS_NONE`]: the submitted exemption certificate has not
/// been reviewed yet.
pub const EXEMPTION_STATUS_PENDING: &str = "Pending";

/// The `HeritageStatus` values, from the model's enum (`OptIn | OptOut`).
pub const HERITAGE_STATUS_OPT_IN: &str = "OptIn";
/// See [`HERITAGE_STATUS_OPT_IN`].
pub const HERITAGE_STATUS_OPT_OUT: &str = "OptOut";

/// The election `GetTaxInheritance` reports for an account that has none
/// recorded.
///
/// `HeritageStatus` is a two-valued enum — an account either inherits its
/// parent's tax settings (`OptIn`) or does not (`OptOut`) — and the member is
/// modelled on the output of a `@readonly` operation that takes no input, so
/// there is no "unset" to report: an account that has not opted in is opted out.
pub const DEFAULT_HERITAGE_STATUS: &str = HERITAGE_STATUS_OPT_OUT;

/// Why `PutTaxInheritance` refuses a caller that is not an AWS Organizations
/// management account.
///
/// Tax inheritance is an organization-level election — whether the member
/// accounts of an organization inherit the management account's tax settings —
/// and only that management account may make it. This emulator presents one
/// standalone account ([`::engine::emulator_account_id`]) that belongs to no
/// organization, so no caller it serves is a management account and the election
/// cannot be made here; the call is answered with the refusal AWS gives a
/// non-management caller rather than with a success that stores nothing AWS would
/// have stored.
///
/// Comparing the caller against [`::engine::emulator_account_id`] is NOT the test
/// that would admit a call: every caller resolves to that one account, so such a
/// check would admit EVERY call — the opposite of what the service does.
/// Management standing is a fact about an organization, which this service
/// neither models nor is told about by any request it serves.
pub const MANAGEMENT_ACCOUNT_ONLY: &str =
    "Only AWS orgs management account is allowed to set tax inheritance.";

/// The role the legal address stored with an account's tax registration plays
/// for that account: one of the model's `AddressRoleType` values
/// (`TaxAddress | BillingAddress | ContactAddress`).
///
/// The address a `TaxRegistrationEntry` carries is the address its TRN is
/// registered at, so the role it plays is the account's tax address — the role
/// the reported account metadata files it under. The other two values are a
/// billing and a contact address, which no operation this service models
/// accepts, so no stored address plays either role.
pub const LEGAL_ADDRESS_ROLE: &str = "TaxAddress";

/// The `TaxRegistrationType` values whose legal address AWS derives from the tax
/// number itself rather than from the request, paired with the country the type
/// belongs to.
///
/// A request that submits a `legalAddress` for one of them is refused: the
/// address is not the caller's to supply. `PAN` is India's Permanent Account
/// Number, and the model's own `ValidationExceptionErrorCode` enum carries
/// `NonIndiaCustomerCanNotSetPAN`, so the type is India-only.
///
/// Country rules that make a member REQUIRED (Saudi Arabia's `addressLine3`,
/// Turkey's `districtOrCounty`, Kenya's `personType`, …) are a different class:
/// they are the per-country business rules [`COUNTRY_RULES`] carries and
/// [`check_country_requirements`] applies.
pub const TYPES_WITH_DERIVED_LEGAL_ADDRESS: &[(&str, &str)] = &[("PAN", "India")];

/// The jurisdictions this emulator stores a SUPPLEMENTAL TRN for.
///
/// `PutSupplementalTaxRegistration` accepts an entry only for a country in this
/// set and refuses every other one with the modelled `ValidationException`,
/// because the real service does: it supports supplemental registration in a
/// bounded set of jurisdictions and answers a 400 for the rest.
///
/// WHERE THIS LIST COMES FROM: not the model. The model constrains
/// `SupplementalTaxRegistrationEntry$address$countryCode` to any two letters and
/// the operation's whole documentation is "Stores supplemental tax registration
/// for a single account", so the model names no jurisdiction; `VAT` is the one
/// `SupplementalTaxRegistrationType` value. The boundary is therefore the
/// emulator's own, and it holds the jurisdiction observed to clear country
/// validation (Switzerland — a Swiss VAT/MWST number is held with the Swiss
/// authority alongside the account's primary TRN). The alternative — accepting
/// every country — would report a supplemental registration AWS refuses, which
/// is the worse answer of the two: a refusal is a modelled outcome a client
/// handles, a fabricated success is not. A later fixer whose recording shows
/// another jurisdiction accepted adds its row here, and nothing else changes.
pub const SUPPLEMENTAL_TRN_COUNTRIES: &[(&str, &str)] = &[("CH", "Switzerland")];

/// `ValidationException.errorCode` values, from the model's
/// `ValidationExceptionErrorCode` enum
/// (`MalformedToken | ExpiredToken | InvalidToken | FieldValidationFailed |
/// MissingInput | NonIndiaCustomerCanNotSetPAN | GSTExistenceBlockSetPAN`).
pub const ERROR_CODE_MISSING_INPUT: &str = "MissingInput";
/// See [`ERROR_CODE_MISSING_INPUT`].
pub const ERROR_CODE_FIELD_VALIDATION_FAILED: &str = "FieldValidationFailed";
/// See [`ERROR_CODE_MISSING_INPUT`].
pub const ERROR_CODE_MALFORMED_TOKEN: &str = "MalformedToken";
/// See [`ERROR_CODE_MISSING_INPUT`].
pub const ERROR_CODE_INVALID_TOKEN: &str = "InvalidToken";

/// The page a listing builds when a request names no `maxResults`, for an
/// operation whose own documentation states no default.
///
/// The bounds are per operation (see [`PageBounds`]): where the operation's
/// documentation states them, they are the service's, and where it states none
/// this is the emulator's own, so a request that names no page size still gets a
/// bounded page and a cursor to continue from rather than the whole store in one
/// response.
pub const DEFAULT_PAGE_SIZE: usize = 100;

/// The largest page a listing builds whose own documentation states no cap,
/// whatever `maxResults` asks for.
///
/// Equal to the ceiling of the shared `MaxResults` range (1..=1000), so it never
/// truncates a page size the model admits; it pins that the SERVER, not the
/// request, owns the upper bound on one response's size and on the work done
/// while the state lock is held.
pub const MAX_PAGE_SIZE: usize = 1_000;

/// The most tax registrations this emulator keeps resident.
///
/// `accountId` is caller-supplied (any 12 digits), so without a bound a caller
/// could grow the store by one record per request and there would be no release
/// path but `DeleteTaxRegistration`. A write that would add a record past this
/// bound is refused with `PutTaxRegistration`'s modelled `ConflictException`;
/// rewriting an account that already has a record is always allowed, because it
/// adds nothing.
pub const MAX_STORED_REGISTRATIONS: usize = 1_000;

/// The most supplemental tax registrations this emulator keeps resident.
///
/// `SupplementalTaxRegistrationEntry$registrationId` is caller-supplied, so
/// without a bound a caller could grow the store by one record per request. A
/// write that would ADD a record past this bound is refused with
/// `PutSupplementalTaxRegistration`'s modelled `ConflictException`; rewriting a
/// supplemental TRN the account already has is always allowed, because it adds
/// nothing.
pub const MAX_STORED_SUPPLEMENTAL_REGISTRATIONS: usize = 1_000;

/// The page-size bounds of ONE listing operation: the page it builds when the
/// request names no `maxResults`, and the largest page it builds whatever
/// `maxResults` asks for.
///
/// Bounds belong to the operation, not to the service: the listings of one
/// service need not share them, and the range of the `MaxResults` shape they
/// share (1..=1000) is what the WIRE admits, not what any one of them serves in
/// a single page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PageBounds {
    /// The page size used when the request names no `maxResults`.
    pub default_size: usize,
    /// The largest page built, whatever `maxResults` asks for.
    pub max_size: usize,
}

impl PageBounds {
    /// Bounds with this default page size and this ceiling.
    pub const fn new(default_size: usize, max_size: usize) -> Self {
        Self {
            default_size,
            max_size,
        }
    }

    /// The page size a request selects under these bounds: what it asked for,
    /// else [`Self::default_size`], never more than [`Self::max_size`] and never
    /// zero.
    ///
    /// A `maxResults` outside the model's 1..=1000 range never reaches here: the
    /// boundary validator refuses it with a `ValidationException` before the
    /// handler runs, as `tests/all/tax_exemptions.rs` pins on each bound and one
    /// past it. The clamp is therefore the SERVER's ceiling on a page, which a
    /// larger `maxResults` does not raise, not a coercion of a value a client
    /// could not have sent.
    pub fn page_size(self, max_results: Option<i32>) -> usize {
        max_results
            .and_then(|max| usize::try_from(max).ok())
            .unwrap_or(self.default_size)
            .clamp(1, self.max_size)
    }
}

/// `ListTaxRegistrations`' page bounds, which its own documentation states: the
/// operation "can be used to retrieve up to 100 accounts' tax registrations in
/// one call (default 50)". So a request naming no `maxResults` is served 50
/// entries and a cursor, and one naming more than 100 — up to the 1000 the
/// shared `MaxResults` shape admits on the wire — is still served at most 100
/// entries and a cursor.
pub const TAX_REGISTRATIONS_PAGE_BOUNDS: PageBounds = PageBounds::new(50, 100);

/// `ListTaxExemptions`' page bounds. Its documentation states neither a default
/// nor a cap, so these are the emulator's own ([`DEFAULT_PAGE_SIZE`] /
/// [`MAX_PAGE_SIZE`]).
pub const TAX_EXEMPTIONS_PAGE_BOUNDS: PageBounds =
    PageBounds::new(DEFAULT_PAGE_SIZE, MAX_PAGE_SIZE);

/// `ListSupplementalTaxRegistrations`' page bounds. Documented like
/// [`TAX_EXEMPTIONS_PAGE_BOUNDS`]: the operation states neither a default nor a
/// cap, so these are the emulator's own.
pub const SUPPLEMENTAL_TAX_REGISTRATIONS_PAGE_BOUNDS: PageBounds =
    PageBounds::new(DEFAULT_PAGE_SIZE, MAX_PAGE_SIZE);

/// Per-service state for com-amazonaws-taxsettings-taxsettings. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsTaxsettingsTaxsettingsState {
    /// Every account's tax registration, keyed by its 12-digit account id.
    ///
    /// A `BTreeMap` so `ListTaxRegistrations` enumerates in one stable order and
    /// a page token can name the key the previous page stopped after.
    pub registrations: BTreeMap<String, StoredTaxRegistration>,
    /// The tax-inheritance election (`OptIn` / `OptOut`) recorded for the
    /// account. One value, because neither `PutTaxInheritance` nor
    /// `GetTaxInheritance` takes an account: both address the caller's own, and
    /// this emulator presents one account.
    ///
    /// `None` while none is recorded, which is every call today: the only writer
    /// AWS models is the management-account call no caller here can be (see
    /// [`MANAGEMENT_ACCOUNT_ONLY`]), so this is the one place a writer that could
    /// be would write, and `GetTaxInheritance` reports
    /// [`DEFAULT_HERITAGE_STATUS`] until one does.
    pub tax_inheritance: Option<String>,
    /// The account's supplemental tax registrations, keyed by the `authorityId`
    /// this service minted for each when it was first stored.
    ///
    /// A `BTreeMap` so `ListSupplementalTaxRegistrations` enumerates in one
    /// stable order and a page token can name the key the previous page stopped
    /// after. One map for the whole store, because neither
    /// `PutSupplementalTaxRegistration` nor `ListSupplementalTaxRegistrations`
    /// takes an account: both address the caller's own, and this emulator
    /// presents one account.
    pub supplemental_registrations: BTreeMap<String, StoredSupplementalRegistration>,
    /// How many `authorityId`s this service has minted. The next one is derived
    /// from it, so every supplemental TRN gets its own and a deleted one's id is
    /// never handed out again.
    pub minted_authority_ids: u64,
    /// The tax exemptions recorded for each account, keyed by its 12-digit
    /// account id.
    ///
    /// A `BTreeMap` so a listing enumerates in one stable order and a page token
    /// can name the key the previous page stopped after, the same way
    /// [`Self::registrations`] does. An account with no exemption has no entry
    /// here — never an empty vector — so "this service holds exemptions for the
    /// account" is one question with one answer. Within an account the records
    /// keep the order they were first recorded in, so every reader reports one
    /// account's exemptions one way.
    ///
    /// `PutTaxExemption` is the writer AWS models for this map (it submits an
    /// exemption certificate for review) and writes it through
    /// [`Self::store_exemptions`].
    pub exemptions: BTreeMap<String, Vec<StoredTaxExemption>>,
    /// How many support cases `PutTaxExemption` has opened. The next `caseId` is
    /// derived from it, so every submission gets its own.
    pub minted_exemption_cases: u64,
}

impl ComAmazonawsTaxsettingsTaxsettingsState {
    /// Materialise every TRN validation the clock has already made due at `now`.
    ///
    /// Each stored registration carries the instant its validation falls due,
    /// which was fixed by the write that started it, so the status a read reports
    /// is the one the registration had at that instant whether or not anybody
    /// called: this settles what is owed, it never starts anything. Every read of
    /// a registration goes through here first, so `GetTaxRegistration` and
    /// `ListTaxRegistrations` cannot report one record in two different states.
    pub fn settle_due(&mut self, now: Instant) {
        for stored in self.registrations.values_mut() {
            stored.settle(now);
        }
        for stored in self.supplemental_registrations.values_mut() {
            stored.settle(now);
        }
    }

    /// Store `entry` as `account_id`'s tax registration, answering the status it
    /// was stored with.
    ///
    /// The one place a `TaxRegistrationEntry` becomes a stored record, so
    /// `PutTaxRegistration` and the per-account loop of
    /// `BatchPutTaxRegistration` cannot write one entry two different ways: the
    /// write replaces the account's record and bumps its revision, which the
    /// document access tokens carry.
    ///
    /// Two refusals, both `ConflictException`s — each caller renders one on its
    /// own channel, a `ConflictException` response or a per-account row of the
    /// batch's `errors` list:
    ///
    /// * the account's stored TRN is still outstanding: both writers document
    ///   "You can't set a TRN if there's a pending TRN. You'll need to delete the
    ///   pending TRN first", so a record in [`STATUS_PENDING`] is NOT replaced —
    ///   it is left exactly as it was and the write is refused
    ///   ([`pending_registration_conflict`]). The validation the clock had
    ///   already made due at `written_at` is materialised first, because it was
    ///   owed at that instant whether or not anybody wrote: a TRN whose
    ///   validation has run is not an outstanding one, so it may be rewritten.
    /// * `accountId` is caller-supplied, so a write that would ADD a record is
    ///   refused once the store holds as many as this emulator keeps resident
    ///   ([`MAX_STORED_REGISTRATIONS`]) while rewriting an account that already
    ///   has one adds nothing.
    pub fn store_registration(
        &mut self,
        account_id: &str,
        entry: &TaxRegistrationEntry,
        written_at: Instant,
    ) -> Result<String, ConflictException> {
        let previous_revision = match self.registrations.get_mut(account_id) {
            Some(stored) => {
                // Settle what the clock owed at this instant before judging the
                // record's status, so "pending" means a validation that is
                // genuinely still outstanding rather than one nobody has read
                // since it fell due.
                stored.settle(written_at);
                if stored.status == STATUS_PENDING {
                    return Err(pending_registration_conflict());
                }
                Some(stored.revision)
            }
            None => None,
        };
        if previous_revision.is_none() && self.registrations.len() >= MAX_STORED_REGISTRATIONS {
            return Err(registration_store_full());
        }
        let revision = previous_revision.map_or(1, |revision| revision.saturating_add(1));
        let stored = StoredTaxRegistration::from_entry(entry, account_id, revision, written_at);
        let status = stored.status.clone();
        self.registrations.insert(account_id.to_string(), stored);
        Ok(status)
    }

    /// Whether this service reports tax exemptions for `account_id` at all —
    /// that is, whether the account is one of those a caller of this service may
    /// read exemptions for.
    ///
    /// The exemption reads address "accounts listed in a consolidated billing
    /// family" (`smithy_query(query="doc",shape="com.amazonaws.taxsettings#ListTaxExemptions")`),
    /// so the accounts they can answer for are the caller's own account and the
    /// accounts whose tax settings this service holds — the only accounts it is
    /// ever told about. Any other 12-digit id names an account outside that
    /// family, which this service holds no tax-settings resource for.
    ///
    /// Comparing a REQUEST-SUPPLIED id against [`::engine::emulator_account_id`]
    /// is a real discriminator (an id naming another account fails it), unlike
    /// comparing the CALLER against it — see [`MANAGEMENT_ACCOUNT_ONLY`] for why
    /// the latter would admit every call.
    pub fn reports_exemptions_for(&self, account_id: &str) -> bool {
        account_id == ::engine::emulator_account_id()
            || self.registrations.contains_key(account_id)
            || self.exemptions.contains_key(account_id)
    }

    /// The `TaxExemptionDetails` the exemption reads report for `account_id` at
    /// `now`, or `None` when this service holds no exemption for it.
    ///
    /// An account the service knows but holds no exemption for therefore gets no
    /// entry in the reported map: the reads report the exemptions an account
    /// has, and an account with none has nothing to report.
    ///
    /// The `heritageObtained*` members describe an exemption INHERITED from the
    /// consolidated billing family's management account. No operation this
    /// service models records such an inheritance — the same reason
    /// `ListTaxRegistrations` reports no `taxInheritanceDetails` — so there is
    /// nothing stored to report and they stay absent.
    pub fn exemption_details(
        &self,
        account_id: &str,
        now: SystemTime,
    ) -> Option<TaxExemptionDetails> {
        let stored = self.exemptions.get(account_id)?;
        if stored.is_empty() {
            return None;
        }
        Some(TaxExemptionDetails {
            tax_exemptions: Some(
                stored
                    .iter()
                    .map(|exemption| exemption.to_tax_exemption(now))
                    .collect(),
            ),
            heritage_obtained_details: None,
            heritage_obtained_parent_entity: None,
            heritage_obtained_reason: None,
        })
    }

    /// The `authorityId` of the stored supplemental TRN with this type and
    /// registration id, if the account already has one.
    ///
    /// `SupplementalTaxRegistrationEntry` carries no `authorityId`, so the TRN
    /// itself — its type and its registration id — is the identity a write
    /// addresses: that is what makes `PutSupplementalTaxRegistration` "adding or
    /// updating" one registration rather than appending a record per call.
    pub fn find_supplemental_authority(
        &self,
        registration_type: &str,
        registration_id: &str,
    ) -> Option<String> {
        self.supplemental_registrations
            .iter()
            .find(|(_, stored)| {
                stored.registration_type == registration_type
                    && stored.registration_id == registration_id
            })
            .map(|(authority_id, _)| authority_id.clone())
    }

    /// Mint the `authorityId` of a supplemental TRN this account did not have
    /// yet.
    ///
    /// `authorityId` is SERVER-generated (no request member carries one) and the
    /// model constrains it only as a `GenericString` of 1..=200 characters, so
    /// AWS publishes no format for it: this mints a short opaque id from the
    /// service's own counter, unique per record and stable for the life of that
    /// record, which is every invariant the member has.
    pub fn mint_supplemental_authority_id(&mut self) -> String {
        self.minted_authority_ids = self.minted_authority_ids.saturating_add(1);
        format!("supplemental-trn-{:08}", self.minted_authority_ids)
    }

    /// Mint the id of the support case a `PutTaxExemption` submission opens.
    ///
    /// `caseId` is SERVER-generated (no request member carries one) and the model
    /// constrains it only as a `GenericString` of 1..=200 characters, so AWS
    /// publishes no format for it: this mints a short opaque id from the service's
    /// own counter, unique per submission, which is every invariant the member
    /// has.
    pub fn mint_exemption_case_id(&mut self) -> String {
        self.minted_exemption_cases = self.minted_exemption_cases.saturating_add(1);
        format!("tax-exemption-case-{:08}", self.minted_exemption_cases)
    }

    /// How many exemption records the store holds, across every account.
    pub fn stored_exemption_count(&self) -> usize {
        self.exemptions.values().map(Vec::len).sum()
    }

    /// Record the exemption `exemption_type` and `authority` identify for every
    /// account of `account_ids`, as submitted at `recorded_at`.
    ///
    /// The one place an exemption becomes a stored record. The exemption's
    /// identity is its type and the jurisdiction it applies in — the request
    /// carries no exemption id — so submitting the same pair for an account again
    /// REPLACES that account's record in place, which is what makes this a `Put`
    /// rather than an append; case and padding in the jurisdiction do not make a
    /// second exemption ([`is_same_exemption`]).
    ///
    /// The record is stored in [`EXEMPTION_STATUS_PENDING`], the status the model
    /// gives an exemption whose certificate has not been reviewed: a submission
    /// attaches the certificate to a support case for review, and granting the
    /// exemption is that reviewer's decision, not something the clock makes due
    /// (see [`StoredTaxExemption::reported_status`]). It carries no effective or
    /// expiration date either: both are facts about the submitted certificate,
    /// which nothing here reads.
    ///
    /// `accountIds` is caller-supplied (up to 550 ids per call), so a submission
    /// that would ADD records past [`MAX_STORED_EXEMPTIONS`] is refused whole,
    /// before any record is written: a partially applied submission would report
    /// one support case covering accounts it did not cover. Replacing exemptions
    /// the accounts already hold is always allowed, because it adds nothing.
    ///
    /// Both passes walk only the accounts the request names plus the records they
    /// hold, and the count they are checked against is bounded by
    /// [`MAX_STORED_EXEMPTIONS`]; a larger bound would want that count kept
    /// incrementally rather than summed per submission.
    pub fn store_exemptions(
        &mut self,
        account_ids: &[String],
        exemption_type: &TaxExemptionTypeRow,
        authority: &Authority,
        recorded_at: SystemTime,
    ) -> Result<(), CaseCreationLimitExceededException> {
        let description = exemption_type.1;
        let distinct: BTreeSet<&str> = account_ids.iter().map(String::as_str).collect();
        let additional = distinct
            .iter()
            .filter(|account_id| {
                self.exemptions.get(**account_id).is_none_or(|held| {
                    !held
                        .iter()
                        .any(|stored| is_same_exemption(stored, description, authority))
                })
            })
            .count();
        if self.stored_exemption_count() + additional > MAX_STORED_EXEMPTIONS {
            return Err(exemption_store_full());
        }

        let submitted = StoredTaxExemption {
            authority: normalized_authority(authority),
            exemption_type: exemption_type_of(exemption_type),
            effective_date: None,
            expiration_date: None,
            system_effective_date: Some(recorded_at),
            status: EXEMPTION_STATUS_PENDING.to_string(),
        };
        for account_id in distinct {
            let held = self.exemptions.entry(account_id.to_string()).or_default();
            match held
                .iter()
                .position(|stored| is_same_exemption(stored, description, authority))
            {
                Some(index) => held[index] = submitted.clone(),
                None => held.push(submitted.clone()),
            }
        }
        Ok(())
    }
}

/// One supplemental tax registration (a supplemental TRN) of the account.
///
/// Every member of the `SupplementalTaxRegistration` shape the reads report is
/// `@required`, and the write's entry carries all of them but `authorityId` and
/// `status`, which the service owns: the id is minted when the record is first
/// stored and the status is the outcome of validating the submitted TRN.
#[derive(Debug, Clone, Default)]
pub struct StoredSupplementalRegistration {
    /// `SupplementalTaxRegistrationEntry$registrationId`, as submitted.
    pub registration_id: String,
    /// `SupplementalTaxRegistrationEntry$registrationType`, one of the model's
    /// `SupplementalTaxRegistrationType` values (today only `VAT`).
    pub registration_type: String,
    /// `SupplementalTaxRegistrationEntry$legalName`, as submitted.
    pub legal_name: String,
    /// `SupplementalTaxRegistrationEntry$address`, as submitted.
    pub address: Address,
    /// The registration's `TaxRegistrationStatus`.
    pub status: String,
    /// When the validation of this TRN falls due, after which [`Self::settle`]
    /// moves [`Self::status`] to [`SETTLED_REGISTRATION_STATUS`] — the same
    /// clock-owed transition a main TRN has, for the same reason. `None` once it
    /// has been settled.
    pub validation_due_at: Option<Instant>,
}

impl StoredSupplementalRegistration {
    /// The record `PutSupplementalTaxRegistration` stores for `entry`.
    pub fn from_entry(entry: &SupplementalTaxRegistrationEntry, written_at: Instant) -> Self {
        Self {
            registration_id: entry.registration_id.clone(),
            registration_type: entry.registration_type.clone(),
            legal_name: entry.legal_name.clone(),
            address: entry.address.clone(),
            // The write starts the validation of the submitted TRN and reports
            // the status the registration is stored with while that validation
            // is outstanding.
            status: STATUS_PENDING.to_string(),
            validation_due_at: Some(written_at + TRN_VALIDATION_DELAY),
        }
    }

    /// Move this registration to the status its validation reached, if that
    /// validation was already due at `now`. Answers whether it moved.
    pub fn settle(&mut self, now: Instant) -> bool {
        match self.validation_due_at {
            Some(due) if due <= now => {
                self.status = SETTLED_REGISTRATION_STATUS.to_string();
                self.validation_due_at = None;
                true
            }
            _ => false,
        }
    }

    /// The `SupplementalTaxRegistration` shape the reads report, under the
    /// `authorityId` this record is stored by.
    pub fn to_supplemental_tax_registration(
        &self,
        authority_id: &str,
    ) -> SupplementalTaxRegistration {
        SupplementalTaxRegistration {
            registration_id: self.registration_id.clone(),
            registration_type: self.registration_type.clone(),
            legal_name: self.legal_name.clone(),
            address: self.address.clone(),
            authority_id: authority_id.to_string(),
            status: self.status.clone(),
        }
    }
}

/// One account's stored tax registration.
///
/// `legalName` and `legalAddress` are stored unconditionally because every shape
/// that reports a registration (`TaxRegistration`,
/// `TaxRegistrationWithJurisdiction`) marks both `@required`: a record missing
/// either could not be reported, so `PutTaxRegistration` refuses the write
/// instead of storing one.
#[derive(Debug, Clone, Default)]
pub struct StoredTaxRegistration {
    /// `TaxRegistrationEntry$registrationId`, as submitted.
    pub registration_id: String,
    /// `TaxRegistrationEntry$registrationType`, one of the model's
    /// `TaxRegistrationType` values.
    pub registration_type: String,
    /// `TaxRegistrationEntry$legalName`, as submitted.
    pub legal_name: String,
    /// `TaxRegistrationEntry$legalAddress`, as submitted. Also the source of the
    /// `Jurisdiction` that `ListTaxRegistrations` reports.
    pub legal_address: Address,
    /// `TaxRegistrationEntry$sector`, one of the model's `Sector` values.
    pub sector: Option<String>,
    /// `TaxRegistrationEntry$additionalTaxInformation`, projected onto the
    /// response shape the reads report.
    pub additional_tax_information: Option<AdditionalInfoResponse>,
    /// `TaxRegistrationEntry$certifiedEmailId`, as submitted.
    pub certified_email_id: Option<String>,
    /// The registration's `TaxRegistrationStatus`.
    pub status: String,
    /// When the validation of this TRN falls due, after which [`Self::settle`]
    /// moves [`Self::status`] to [`SETTLED_REGISTRATION_STATUS`]. `None` once it
    /// has been settled, so settling is idempotent and a settled record never
    /// moves again.
    pub validation_due_at: Option<Instant>,
    /// How many times `PutTaxRegistration` has written this account's TRN. The
    /// document access tokens below are derived from it, because the model
    /// documents that updating a registration invalidates the previous tokens.
    pub revision: u64,
    /// The metadata of the documents submitted with this registration, derived
    /// from `VerificationDetails$taxRegistrationDocuments` when it was stored.
    /// Empty when the registration carried no document.
    pub tax_document_metadatas: Vec<TaxDocumentMetadata>,
}

impl StoredTaxRegistration {
    /// The record `PutTaxRegistration` stores for `entry`.
    ///
    /// `revision` is the account's new revision number (its previous one plus
    /// one), which the derived document access tokens carry. `legalName` and
    /// `legalAddress` are optional on `TaxRegistrationEntry` but required on
    /// every shape that reports a registration, so the caller refuses an entry
    /// that omits either before reaching here.
    pub fn from_entry(
        entry: &TaxRegistrationEntry,
        account_id: &str,
        revision: u64,
        written_at: Instant,
    ) -> Self {
        let legal_name = entry.legal_name.clone().unwrap_or_default();
        let legal_address = entry.legal_address.clone().unwrap_or_default();
        Self {
            registration_id: entry.registration_id.clone(),
            registration_type: entry.registration_type.clone(),
            legal_name,
            legal_address,
            sector: entry.sector.clone(),
            additional_tax_information: entry
                .additional_tax_information
                .as_ref()
                .map(additional_info_response),
            certified_email_id: entry.certified_email_id.clone(),
            // The write starts the validation of the submitted TRN and reports
            // the status the registration is stored with while that validation
            // is outstanding.
            status: STATUS_PENDING.to_string(),
            validation_due_at: Some(written_at + TRN_VALIDATION_DELAY),
            revision,
            tax_document_metadatas: document_metadatas(entry, account_id, revision),
        }
    }

    /// Move this registration to the status its validation reached, if that
    /// validation was already due at `now`. Answers whether it moved.
    pub fn settle(&mut self, now: Instant) -> bool {
        match self.validation_due_at {
            Some(due) if due <= now => {
                self.status = SETTLED_REGISTRATION_STATUS.to_string();
                self.validation_due_at = None;
                true
            }
            _ => false,
        }
    }

    /// The `TaxRegistration` shape `GetTaxRegistration` reports, carrying the
    /// full legal address.
    pub fn to_tax_registration(&self) -> TaxRegistration {
        TaxRegistration {
            registration_id: self.registration_id.clone(),
            registration_type: self.registration_type.clone(),
            legal_name: self.legal_name.clone(),
            status: self.status.clone(),
            sector: self.sector.clone(),
            tax_document_metadatas: self.reported_document_metadatas(),
            certified_email_id: self.certified_email_id.clone(),
            additional_tax_information: self.additional_tax_information.clone(),
            legal_address: self.legal_address.clone(),
        }
    }

    /// The `TaxRegistrationWithJurisdiction` shape `ListTaxRegistrations`
    /// reports: the same record, with the legal address reduced to the
    /// jurisdiction (country code, and state or region where there is one) the
    /// model says this shape carries instead of the full address.
    pub fn to_tax_registration_with_jurisdiction(&self) -> TaxRegistrationWithJurisdiction {
        TaxRegistrationWithJurisdiction {
            registration_id: self.registration_id.clone(),
            registration_type: self.registration_type.clone(),
            legal_name: self.legal_name.clone(),
            status: self.status.clone(),
            sector: self.sector.clone(),
            tax_document_metadatas: self.reported_document_metadatas(),
            certified_email_id: self.certified_email_id.clone(),
            additional_tax_information: self.additional_tax_information.clone(),
            jurisdiction: self.jurisdiction(),
        }
    }

    /// This registration's `Jurisdiction`: the stored legal address reduced to
    /// its country code and, where the address has one, its state or region.
    ///
    /// One projection for every shape that reports where the registration is
    /// held, so the jurisdiction beside the TRN and the one in the account's
    /// address-role map cannot disagree.
    pub fn jurisdiction(&self) -> Jurisdiction {
        Jurisdiction {
            state_or_region: self.legal_address.state_or_region.clone(),
            country_code: self.legal_address.country_code.clone(),
        }
    }

    /// The `AccountMetaData` a listing reports beside the account's TRN: the
    /// legal profile this service holds for the account.
    ///
    /// Every member is projected from the stored record. `accountName` is the
    /// legal name the registration was stored with — the only name any operation
    /// this service models persists for an account; `address` is the stored
    /// legal address in full (the listing's `taxRegistration` carries only its
    /// jurisdiction, so this is where a caller reads the address); `addressType`
    /// and `addressRoleMap` report the role that address plays
    /// ([`LEGAL_ADDRESS_ROLE`]) and, under that role, the jurisdiction of the
    /// address held in it.
    ///
    /// `seller` is the Amazon Web Services seller of record for the account: no
    /// operation this service models carries one and nothing in the store records
    /// one, so it stays absent rather than being invented.
    pub fn to_account_meta_data(&self) -> AccountMetaData {
        AccountMetaData {
            account_name: Some(self.legal_name.clone()),
            seller: None,
            address: Some(self.legal_address.clone()),
            address_type: Some(LEGAL_ADDRESS_ROLE.to_string()),
            address_role_map: Some(HashMap::from([(
                LEGAL_ADDRESS_ROLE.to_string(),
                self.jurisdiction(),
            )])),
        }
    }

    /// `taxDocumentMetadatas` as the reads report it: absent rather than an
    /// empty list, because the model constrains `TaxDocumentMetadatas` to
    /// 1..=5 entries.
    fn reported_document_metadatas(&self) -> Option<Vec<TaxDocumentMetadata>> {
        if self.tax_document_metadatas.is_empty() {
            None
        } else {
            Some(self.tax_document_metadatas.clone())
        }
    }
}

/// One tax exemption recorded for an account.
///
/// The dates are wall-clock instants ([`SystemTime`]) rather than [`Instant`]s
/// because the model reports them as `Timestamp`s to the caller, and because
/// whether an exemption has expired is a fact about the calendar.
#[derive(Debug, Clone)]
pub struct StoredTaxExemption {
    /// `TaxExemption$authority`: the address domain (country, and state where
    /// the exemption is a state one) the exemption applies in.
    pub authority: Authority,
    /// `TaxExemption$taxExemptionType`: the entry of
    /// [`TAX_EXEMPTION_TYPE_CATALOG`] the exemption was granted under.
    pub exemption_type: TaxExemptionType,
    /// When the exemption takes effect. `None` while that is not decided (an
    /// exemption still under review has no effective date yet).
    pub effective_date: Option<SystemTime>,
    /// When the exemption lapses. `None` for one that does not expire.
    pub expiration_date: Option<SystemTime>,
    /// When this service recorded the exemption —
    /// `TaxExemption$systemEffectiveDate`, "the tax exemption recording time in
    /// the `TaxSettings` system".
    pub system_effective_date: Option<SystemTime>,
    /// The exemption's recorded `EntityExemptionAccountStatus`
    /// ([`EXEMPTION_STATUS_VALID`] and friends).
    pub status: String,
}

impl StoredTaxExemption {
    /// The status a read reports at `now`.
    ///
    /// Lapsing is a transition the CLOCK owes: an exemption whose expiration
    /// date has passed is expired at that instant whether or not anybody called,
    /// so a read reports [`EXEMPTION_STATUS_EXPIRED`] for it rather than the
    /// status it was recorded with. Every other status is reported as recorded —
    /// a `Pending` certificate becomes valid only when a reviewer decides it
    /// does, which is not something the clock can make due.
    pub fn reported_status(&self, now: SystemTime) -> &str {
        if self.status == EXEMPTION_STATUS_VALID
            && self.expiration_date.is_some_and(|expires| expires <= now)
        {
            EXEMPTION_STATUS_EXPIRED
        } else {
            &self.status
        }
    }

    /// The `TaxExemption` shape the exemption reads report for this record at
    /// `now`.
    pub fn to_tax_exemption(&self, now: SystemTime) -> TaxExemption {
        TaxExemption {
            authority: self.authority.clone(),
            tax_exemption_type: self.exemption_type.clone(),
            effective_date: self.effective_date.map(::aws_smithy_types::DateTime::from),
            expiration_date: self.expiration_date.map(::aws_smithy_types::DateTime::from),
            system_effective_date: self
                .system_effective_date
                .map(::aws_smithy_types::DateTime::from),
            status: Some(self.reported_status(now).to_string()),
        }
    }
}

/// Project an `AdditionalInfoRequest` (what a client submits) onto the
/// `AdditionalInfoResponse` (what the reads report), country block by country
/// block.
///
/// `brazilAdditionalInfo` and `indiaAdditionalInfo` exist only on the response
/// shape — there is no request member to carry them — so they stay absent.
fn additional_info_response(request: &AdditionalInfoRequest) -> AdditionalInfoResponse {
    AdditionalInfoResponse {
        malaysia_additional_info: request.malaysia_additional_info.clone(),
        israel_additional_info: request.israel_additional_info.clone(),
        estonia_additional_info: request.estonia_additional_info.clone(),
        canada_additional_info: request.canada_additional_info.clone(),
        brazil_additional_info: None,
        spain_additional_info: request.spain_additional_info.clone(),
        kenya_additional_info: request.kenya_additional_info.clone(),
        south_korea_additional_info: request.south_korea_additional_info.clone(),
        turkey_additional_info: request.turkey_additional_info.clone(),
        georgia_additional_info: request.georgia_additional_info.clone(),
        italy_additional_info: request.italy_additional_info.clone(),
        romania_additional_info: request.romania_additional_info.clone(),
        ukraine_additional_info: request.ukraine_additional_info.clone(),
        poland_additional_info: request.poland_additional_info.clone(),
        saudi_arabia_additional_info: request.saudi_arabia_additional_info.clone(),
        india_additional_info: None,
        indonesia_additional_info: request.indonesia_additional_info.clone(),
        vietnam_additional_info: request.vietnam_additional_info.clone(),
        egypt_additional_info: request.egypt_additional_info.clone(),
        greece_additional_info: request.greece_additional_info.clone(),
        uzbekistan_additional_info: request.uzbekistan_additional_info.clone(),
        philippines_additional_info: request.philippines_additional_info.clone(),
        belgium_additional_info: request.belgium_additional_info.clone(),
        chile_additional_info: request.chile_additional_info.clone(),
        france_additional_info: request.france_additional_info.clone(),
        monaco_additional_info: request.monaco_additional_info.clone(),
    }
}

/// The `TaxDocumentMetadata` of each document submitted in
/// `VerificationDetails$taxRegistrationDocuments`.
///
/// A document is named by its uploaded file name, or by its S3 key when it was
/// referenced in S3. The access token locates the document within this account's
/// registration revision, so writing the registration again mints new tokens and
/// the previous ones no longer name anything — the invalidation the model
/// documents for `taxDocumentAccessToken`.
fn document_metadatas(
    entry: &TaxRegistrationEntry,
    account_id: &str,
    revision: u64,
) -> Vec<TaxDocumentMetadata> {
    let Some(documents) = entry
        .verification_details
        .as_ref()
        .and_then(|details| details.tax_registration_documents.as_ref())
    else {
        return Vec::new();
    };
    documents
        .iter()
        .enumerate()
        .map(|(index, document)| {
            let name = document
                .file
                .as_ref()
                .map(|file| file.file_name.clone())
                .or_else(|| document.s3_location.as_ref().map(|loc| loc.key.clone()))
                .unwrap_or_else(|| format!("document-{}", index + 1));
            TaxDocumentMetadata {
                tax_document_access_token: base64::encode(format!(
                    "{account_id}/{revision}/{index}/{name}"
                )),
                tax_document_name: name,
            }
        })
        .collect()
}

/// The account a request addresses: the `accountId` it carries, or the caller's
/// own account when it omits one, as the model documents for every `accountId`
/// member of this service.
pub fn resolve_account_id(account_id: Option<String>) -> String {
    account_id.unwrap_or_else(|| ::engine::emulator_account_id().to_string())
}

/// This service's modelled `ValidationException` (HTTP 400) naming the member it
/// refused.
///
/// `fieldList` carries the parameter the refusal is about, named the way this
/// service names one on the wire — see [`parameter_name`].
pub fn validation(
    error_code: &str,
    member: &str,
    message: impl Into<String>,
) -> ValidationException {
    ValidationException {
        message: message.into(),
        error_code: error_code.to_string(),
        field_list: Some(vec![ValidationExceptionField {
            name: parameter_name(member),
        }]),
    }
}

/// This service's modelled `ValidationException` (HTTP 400) for a refusal that is
/// about no one input member, so it carries no `fieldList`: that member names "the
/// parameter that caused a `ValidationException` error", and a refusal of the
/// CALL rather than of a parameter has none to name.
pub fn validation_of_the_call(
    error_code: &str,
    message: impl Into<String>,
) -> ValidationException {
    ValidationException {
        message: message.into(),
        error_code: error_code.to_string(),
        field_list: None,
    }
}

/// The name this service reports in `ValidationExceptionField$name`: the
/// offending member's own name in CONSTANT_CASE, which is the form AWS answers
/// with (`taxRegistrationEntry.legalAddress` -> `LEGAL_ADDRESS`).
///
/// The member path a message states is dotted lowerCamel, as the boundary
/// validator spells it; the field list names the PARAMETER, so only the last
/// segment appears and it is re-cased.
///
/// The CONSTANT_CASE form is OBSERVED for one parameter (`LEGAL_ADDRESS`, in the
/// refusal of a legal address submitted for an India PAN registration) and
/// INFERRED for the rest: one casing rule for every parameter this service names
/// is more likely than a per-member one, but no recording pins `NEXT_TOKEN` or
/// `HERITAGE_STATUS`. A later fixer with a recording that disagrees should follow
/// the recording.
pub fn parameter_name(member_path: &str) -> String {
    let member = member_path.rsplit('.').next().unwrap_or(member_path);
    let mut name = String::with_capacity(member.len() + 2);
    for character in member.chars() {
        if character.is_ascii_uppercase() && !name.is_empty() {
            name.push('_');
        }
        name.extend(character.to_uppercase());
    }
    name
}

/// Refuse an entry that submits a member its registration type does not admit.
///
/// Today that is the legal address of a type AWS derives it from the tax number
/// for ([`TYPES_WITH_DERIVED_LEGAL_ADDRESS`]): the address is not the caller's to
/// supply, so submitting one is a field violation naming that parameter.
pub fn check_registration_type_members(
    entry: &TaxRegistrationEntry,
) -> Result<(), ValidationException> {
    let derived = TYPES_WITH_DERIVED_LEGAL_ADDRESS
        .iter()
        .find(|(registration_type, _)| *registration_type == entry.registration_type);
    if let Some((registration_type, country)) = derived {
        if entry.legal_address.is_some() {
            return Err(validation(
                ERROR_CODE_FIELD_VALIDATION_FAILED,
                "taxRegistrationEntry.legalAddress",
                format!(
                    "Legal address should not be provided for {country} {registration_type} Tax \
                     Registration."
                ),
            ));
        }
    }
    Ok(())
}

/// Refuse an all-whitespace value for a member the model constrains with its
/// non-blank pattern `^(?!\s*$)[\s\S]+$`.
///
/// That pattern is a negative lookahead, which the boundary validator's regex
/// engine cannot compile, so a blank value reaches the handler having satisfied
/// every constraint the boundary could check. The handler applies the one it
/// could not.
pub fn check_not_blank(member: &str, value: &str) -> Result<(), ValidationException> {
    if !value.trim().is_empty() {
        return Ok(());
    }
    Err(validation(
        ERROR_CODE_FIELD_VALIDATION_FAILED,
        member,
        format!("{member} must not be blank."),
    ))
}

/// Apply [`check_not_blank`] to every `Address` member the model constrains with
/// the non-blank pattern. `countryCode`'s own pattern (`^[a-zA-Z]+$`) is
/// lookahead-free, so the boundary validator already enforces it.
pub fn check_address(member_path: &str, address: &Address) -> Result<(), ValidationException> {
    let members: [(&str, Option<&String>); 7] = [
        ("addressLine1", address.address_line1.as_ref()),
        ("addressLine2", address.address_line2.as_ref()),
        ("addressLine3", address.address_line3.as_ref()),
        ("districtOrCounty", address.district_or_county.as_ref()),
        ("city", address.city.as_ref()),
        ("stateOrRegion", address.state_or_region.as_ref()),
        ("postalCode", Some(&address.postal_code)),
    ];
    for (name, value) in members {
        if let Some(value) = value {
            check_not_blank(&format!("{member_path}.{name}"), value)?;
        }
    }
    Ok(())
}

/// What AWS answers when the `additionalTaxInformation` object a request carries
/// does not satisfy the country's rule.
///
/// Observed for the Kenya rule (an entry whose `kenyaAdditionalInfo` carries no
/// `personType`) and used for every rule about that object: one message per
/// class of refusal, with the offending parameter in `fieldList`, rather than a
/// per-member sentence no recording pins.
pub const ADDITIONAL_INFO_INVALID: &str = "Your request has an AdditionalInfo object that isn't \
                                           valid or is missing information. Check your request \
                                           and try again.";

/// The `Sector` values the model documents as valid for a country whose rule
/// reads "The sector valid values are `Business` and `Individual`" (Malaysia,
/// Nepal, Ukraine) — a subset of the `Sector` enum (`Business | Individual |
/// Government`).
pub const SECTORS_BUSINESS_OR_INDIVIDUAL: &[&str] = &["Business", "Individual"];

/// The `PersonType` values the model documents as valid for Georgia ("The valid
/// `personType` values are `Physical Person` and `Business`") — a subset of the
/// `PersonType` enum (`Legal Person | Physical Person | Business`).
pub const GEORGIA_PERSON_TYPES: &[&str] = &["Physical Person", "Business"];

/// The `TaxRegistrationType` values the model documents as valid for Malaysia
/// ("`RegistrationType` valid values are `NRIC` for individual, and TIN and
/// sales and service tax (SST) for Business").
pub const MALAYSIA_REGISTRATION_TYPES: &[&str] = &["NRIC", "TIN", "SST"];

/// The Malaysian registration types the model documents as the business ones,
/// for which "you must specify a `businessRegistrationNumber` in
/// `MalaysiaAdditionalInfo`".
pub const MALAYSIA_BUSINESS_REGISTRATION_TYPES: &[&str] = &["TIN", "SST"];

/// The Malaysian registration type of a business reseller, for which "you must
/// specify a `businessRegistrationNumber` and `taxInformationNumber` in
/// `MalaysiaAdditionalInfo`".
pub const MALAYSIA_RESELLER_REGISTRATION_TYPE: &str = "SST";

/// The `IndonesiaTaxRegistrationNumberType` values whose
/// `ppnExceptionDesignationCode` set is the wider one: "If the
/// `taxRegistrationNumberType` is set to NPWP or NITKU, valid values for
/// `ppnExceptionDesignationCode` are either 01, 02, 03, 07, or 08."
pub const INDONESIA_WIDE_PPN_TYPES: &[&str] = &["NPWP", "NITKU"];

/// See [`INDONESIA_WIDE_PPN_TYPES`].
pub const INDONESIA_PPN_CODES_WIDE: &[&str] = &["01", "02", "03", "07", "08"];

/// "For other `taxRegistrationNumberType` values, `ppnExceptionDesignationCode`
/// must be either 01, 07, or 08."
pub const INDONESIA_PPN_CODES_NARROW: &[&str] = &["01", "07", "08"];

/// "If `ppnExceptionDesignationCode` is 07 or 08, you must specify the
/// `decisionNumber`."
pub const INDONESIA_PPN_CODES_NEEDING_DECISION_NUMBER: &[&str] = &["07", "08"];

/// The Kenyan `personType` for which "you must specify the tax registration
/// certificate document in the `taxRegistrationDocuments` field".
pub const KENYA_PERSON_TYPE_NEEDING_DOCUMENT: &str = "Physical Person";

/// The Spanish `registrationType` for which "you must specify the tax
/// registration certificate document in the `taxRegistrationDocuments` field".
pub const SPAIN_REGISTRATION_TYPE_NEEDING_DOCUMENT: &str = "Local";

/// The Turkish `sector` for which the model documents "If your `sector` is
/// `Individual`, don't enter this value" of `taxOffice` and `kepEmailId`, and
/// for which `taxOffice` is therefore not required either.
pub const TURKEY_SECTOR_WITHOUT_TAX_OFFICE: &str = "Individual";

/// One per-country business rule `PutTaxRegistration` documents for
/// `taxRegistrationEntry`.
///
/// The operation's model documentation states, country by country, which members
/// of the entry a request for that country must specify and which values those
/// members may take ("Kenya: You must specify the `personType` in the
/// `kenyaAdditionalInfo` field of the `additionalTaxInformation` object"). Each
/// row below is one of those sentences, so the rule set is read off the model
/// rather than written per request shape, and one evaluator
/// ([`check_country_requirements`]) applies all of them.
#[derive(Debug, Clone, Copy)]
pub struct CountryRule {
    /// The `Address$countryCode` whose rule this is. Compared case-insensitively,
    /// because the member's pattern (`^[a-zA-Z]+$`) admits either case.
    pub country_code: &'static str,
    /// The member the rule is about, as the wire spells it. Its last segment is
    /// the parameter the refusal names — see [`parameter_name`].
    pub member_path: &'static str,
    /// The `ValidationExceptionErrorCode` the refusal carries:
    /// [`ERROR_CODE_MISSING_INPUT`] for a member the country requires and the
    /// entry omits, [`ERROR_CODE_FIELD_VALIDATION_FAILED`] for one the entry
    /// carries with a value (or at all) the country does not admit.
    pub error_code: &'static str,
    /// The refusal's message.
    pub message: &'static str,
    /// Whether this rule has anything to say about `entry` — some rules are
    /// documented as conditional on another member of the same entry.
    pub applies: fn(&TaxRegistrationEntry) -> bool,
    /// Whether `entry` satisfies the rule.
    pub satisfied: fn(&TaxRegistrationEntry) -> bool,
}

/// A rule that applies to every entry for its country.
fn always(_entry: &TaxRegistrationEntry) -> bool {
    true
}

/// The `additionalTaxInformation` object of `entry`, if it carries one.
fn additional_info(entry: &TaxRegistrationEntry) -> Option<&AdditionalInfoRequest> {
    entry.additional_tax_information.as_ref()
}

/// Whether `entry` carries at least one tax registration certificate document in
/// `verificationDetails.taxRegistrationDocuments`.
fn has_registration_document(entry: &TaxRegistrationEntry) -> bool {
    entry
        .verification_details
        .as_ref()
        .and_then(|details| details.tax_registration_documents.as_ref())
        .is_some_and(|documents| !documents.is_empty())
}

/// `taxRegistrationEntry.sector`, if the entry carries one.
fn sector(entry: &TaxRegistrationEntry) -> Option<&str> {
    entry.sector.as_deref()
}

/// Whether the entry's `sector` is one of `SECTORS_BUSINESS_OR_INDIVIDUAL`, for
/// a country that admits only those two. An entry carrying no sector is left to
/// the rule that requires one, where the country has one.
fn sector_is_business_or_individual(entry: &TaxRegistrationEntry) -> bool {
    sector(entry).is_none_or(|value| SECTORS_BUSINESS_OR_INDIVIDUAL.contains(&value))
}

/// `additionalTaxInformation.kenyaAdditionalInfo.personType`, if the entry
/// carries the Kenyan block. `personType` is `@required` on
/// `KenyaAdditionalInfo`, so the boundary validator has already refused a block
/// without one: carrying the block is carrying the member.
fn kenya_person_type(entry: &TaxRegistrationEntry) -> Option<&str> {
    additional_info(entry)
        .and_then(|info| info.kenya_additional_info.as_ref())
        .map(|kenya| kenya.person_type.as_str())
}

/// `additionalTaxInformation.spainAdditionalInfo.registrationType`, if the entry
/// carries the Spanish block (`registrationType` is `@required` on it).
fn spain_registration_type(entry: &TaxRegistrationEntry) -> Option<&str> {
    additional_info(entry)
        .and_then(|info| info.spain_additional_info.as_ref())
        .map(|spain| spain.registration_type.as_str())
}

/// The entry's `indonesiaAdditionalInfo` block, if it carries one.
fn indonesia_info(entry: &TaxRegistrationEntry) -> Option<&IndonesiaAdditionalInfo> {
    additional_info(entry).and_then(|info| info.indonesia_additional_info.as_ref())
}

/// `indonesiaAdditionalInfo.ppnExceptionDesignationCode`, if the entry carries
/// one.
fn indonesia_ppn_code(entry: &TaxRegistrationEntry) -> Option<&str> {
    indonesia_info(entry).and_then(|indonesia| indonesia.ppn_exception_designation_code.as_deref())
}

/// Whether the `ppnExceptionDesignationCode` the entry carries is one the
/// documented set for its `taxRegistrationNumberType` admits.
fn indonesia_ppn_code_is_admitted(entry: &TaxRegistrationEntry) -> bool {
    let Some(indonesia) = indonesia_info(entry) else {
        return true;
    };
    let Some(code) = indonesia.ppn_exception_designation_code.as_deref() else {
        return true;
    };
    let admitted = match indonesia.tax_registration_number_type.as_deref() {
        Some(number_type) if INDONESIA_WIDE_PPN_TYPES.contains(&number_type) => {
            INDONESIA_PPN_CODES_WIDE
        }
        _ => INDONESIA_PPN_CODES_NARROW,
    };
    admitted.contains(&code)
}

/// The entry's `turkeyAdditionalInfo` member `select`ed, or `None` when the entry
/// carries no Turkish block.
fn turkey_member(
    entry: &TaxRegistrationEntry,
    select: fn(&crate::ops::types::TurkeyAdditionalInfo) -> Option<&String>,
) -> Option<String> {
    additional_info(entry)
        .and_then(|info| info.turkey_additional_info.as_ref())
        .and_then(|turkey| select(turkey).cloned())
}

/// Whether the entry's `sector` is the Turkish one that does not take a
/// `taxOffice` or a `kepEmailId`.
fn turkey_sector_is_individual(entry: &TaxRegistrationEntry) -> bool {
    sector(entry).is_some_and(|value| value == TURKEY_SECTOR_WITHOUT_TAX_OFFICE)
}

/// Every per-country business rule `PutTaxRegistration`'s model documentation
/// states for `taxRegistrationEntry`, in the order that documentation lists the
/// countries
/// (`smithy_query(query="doc",shape="com.amazonaws.taxsettings#PutTaxRegistration")`).
///
/// Three documented rules are deliberately NOT rows here, because the fact they
/// turn on is not in the request and this service models no API that supplies
/// one: Brazil's and Mexico's "complete the registration process in the console
/// first" / "address and legal name are populated from your tax registration
/// number" carve-outs (see `ops/put_tax_registration.rs`, which refuses an entry
/// omitting either member for every country), and Turkey's "if your `sector` is
/// `Business` and you're subject to KDV tax, you must specify your industry",
/// whose KDV liability no member of the entry states.
pub const COUNTRY_RULES: &[CountryRule] = &[
    // Bangladesh: "You must specify the tax registration certificate document in
    // the taxRegistrationDocuments field of the VerificationDetails object."
    CountryRule {
        country_code: "BD",
        member_path: "taxRegistrationEntry.verificationDetails.taxRegistrationDocuments",
        error_code: ERROR_CODE_MISSING_INPUT,
        message: "A tax registration in Bangladesh must specify the tax registration certificate \
                  document in verificationDetails.taxRegistrationDocuments.",
        applies: always,
        satisfied: has_registration_document,
    },
    // Georgia: "The valid personType values are Physical Person and Business."
    CountryRule {
        country_code: "GE",
        member_path: "additionalTaxInformation.georgiaAdditionalInfo.personType",
        error_code: ERROR_CODE_FIELD_VALIDATION_FAILED,
        message: ADDITIONAL_INFO_INVALID,
        applies: |entry: &TaxRegistrationEntry| {
            additional_info(entry).is_some_and(|info| info.georgia_additional_info.is_some())
        },
        satisfied: |entry: &TaxRegistrationEntry| {
            additional_info(entry)
                .and_then(|info| info.georgia_additional_info.as_ref())
                .is_none_or(|georgia| {
                    GEORGIA_PERSON_TYPES.contains(&georgia.person_type.as_str())
                })
        },
    },
    // Indonesia: "You must specify the taxRegistrationNumberType in the
    // indonesiaAdditionalInfo field of the additionalTaxInformation object."
    CountryRule {
        country_code: "ID",
        member_path: "additionalTaxInformation.indonesiaAdditionalInfo.taxRegistrationNumberType",
        error_code: ERROR_CODE_MISSING_INPUT,
        message: ADDITIONAL_INFO_INVALID,
        applies: always,
        satisfied: |entry: &TaxRegistrationEntry| {
            indonesia_info(entry)
                .is_some_and(|indonesia| indonesia.tax_registration_number_type.is_some())
        },
    },
    // Indonesia: "If you specify decisionNumber, you must specify the
    // ppnExceptionDesignationCode in the indonesiaAdditionalInfo field of the
    // additionalTaxInformation object."
    CountryRule {
        country_code: "ID",
        member_path: "additionalTaxInformation.indonesiaAdditionalInfo.ppnExceptionDesignationCode",
        error_code: ERROR_CODE_MISSING_INPUT,
        message: ADDITIONAL_INFO_INVALID,
        applies: |entry: &TaxRegistrationEntry| {
            indonesia_info(entry).is_some_and(|indonesia| indonesia.decision_number.is_some())
        },
        satisfied: |entry: &TaxRegistrationEntry| indonesia_ppn_code(entry).is_some(),
    },
    // Indonesia: the ppnExceptionDesignationCode value set, which depends on the
    // taxRegistrationNumberType — see `indonesia_ppn_code_is_admitted`.
    CountryRule {
        country_code: "ID",
        member_path: "additionalTaxInformation.indonesiaAdditionalInfo.ppnExceptionDesignationCode",
        error_code: ERROR_CODE_FIELD_VALIDATION_FAILED,
        message: ADDITIONAL_INFO_INVALID,
        applies: |entry: &TaxRegistrationEntry| indonesia_ppn_code(entry).is_some(),
        satisfied: indonesia_ppn_code_is_admitted,
    },
    // Indonesia: "If ppnExceptionDesignationCode is 07 or 08, you must specify
    // the decisionNumber in the indonesiaAdditionalInfo field of the
    // additionalTaxInformation object."
    CountryRule {
        country_code: "ID",
        member_path: "additionalTaxInformation.indonesiaAdditionalInfo.decisionNumber",
        error_code: ERROR_CODE_MISSING_INPUT,
        message: ADDITIONAL_INFO_INVALID,
        applies: |entry: &TaxRegistrationEntry| {
            indonesia_ppn_code(entry)
                .is_some_and(|code| INDONESIA_PPN_CODES_NEEDING_DECISION_NUMBER.contains(&code))
        },
        satisfied: |entry: &TaxRegistrationEntry| {
            indonesia_info(entry).is_some_and(|indonesia| indonesia.decision_number.is_some())
        },
    },
    // Kenya: "You must specify the personType in the kenyaAdditionalInfo field of
    // the additionalTaxInformation object."
    CountryRule {
        country_code: "KE",
        member_path: "additionalTaxInformation.kenyaAdditionalInfo.personType",
        error_code: ERROR_CODE_MISSING_INPUT,
        message: ADDITIONAL_INFO_INVALID,
        applies: always,
        satisfied: |entry: &TaxRegistrationEntry| kenya_person_type(entry).is_some(),
    },
    // Kenya: "If the personType is Physical Person, you must specify the tax
    // registration certificate document in the taxRegistrationDocuments field of
    // the VerificationDetails object."
    CountryRule {
        country_code: "KE",
        member_path: "taxRegistrationEntry.verificationDetails.taxRegistrationDocuments",
        error_code: ERROR_CODE_MISSING_INPUT,
        message: "A tax registration in Kenya for a Physical Person must specify the tax \
                  registration certificate document in \
                  verificationDetails.taxRegistrationDocuments.",
        applies: |entry: &TaxRegistrationEntry| {
            kenya_person_type(entry).is_some_and(|person| person == KENYA_PERSON_TYPE_NEEDING_DOCUMENT)
        },
        satisfied: has_registration_document,
    },
    // Malaysia: "The sector valid values are Business and Individual."
    CountryRule {
        country_code: "MY",
        member_path: "taxRegistrationEntry.sector",
        error_code: ERROR_CODE_FIELD_VALIDATION_FAILED,
        message: "The valid sector values for a tax registration in Malaysia are Business and \
                  Individual.",
        applies: always,
        satisfied: sector_is_business_or_individual,
    },
    // Malaysia: "RegistrationType valid values are NRIC for individual, and TIN
    // and sales and service tax (SST) for Business."
    CountryRule {
        country_code: "MY",
        member_path: "taxRegistrationEntry.registrationType",
        error_code: ERROR_CODE_FIELD_VALIDATION_FAILED,
        message: "The valid registrationType values for a tax registration in Malaysia are NRIC, \
                  TIN and SST.",
        applies: always,
        satisfied: |entry: &TaxRegistrationEntry| {
            MALAYSIA_REGISTRATION_TYPES.contains(&entry.registration_type.as_str())
        },
    },
    // Malaysia: "For business, you must specify a businessRegistrationNumber in
    // MalaysiaAdditionalInfo with a TIN type and tax identification number."
    CountryRule {
        country_code: "MY",
        member_path: "additionalTaxInformation.malaysiaAdditionalInfo.businessRegistrationNumber",
        error_code: ERROR_CODE_MISSING_INPUT,
        message: ADDITIONAL_INFO_INVALID,
        applies: |entry: &TaxRegistrationEntry| {
            MALAYSIA_BUSINESS_REGISTRATION_TYPES.contains(&entry.registration_type.as_str())
        },
        satisfied: |entry: &TaxRegistrationEntry| {
            additional_info(entry)
                .and_then(|info| info.malaysia_additional_info.as_ref())
                .is_some_and(|malaysia| malaysia.business_registration_number.is_some())
        },
    },
    // Malaysia: "For business resellers, you must specify a
    // businessRegistrationNumber and taxInformationNumber in
    // MalaysiaAdditionalInfo with a sales and service tax (SST) type and a valid
    // SST number."
    CountryRule {
        country_code: "MY",
        member_path: "additionalTaxInformation.malaysiaAdditionalInfo.taxInformationNumber",
        error_code: ERROR_CODE_MISSING_INPUT,
        message: ADDITIONAL_INFO_INVALID,
        applies: |entry: &TaxRegistrationEntry| {
            entry.registration_type == MALAYSIA_RESELLER_REGISTRATION_TYPE
        },
        satisfied: |entry: &TaxRegistrationEntry| {
            additional_info(entry)
                .and_then(|info| info.malaysia_additional_info.as_ref())
                .is_some_and(|malaysia| malaysia.tax_information_number.is_some())
        },
    },
    // Mexico: "You must provide a Constancia de Situación fiscal (CSF) document
    // in the verificationDetails field."
    CountryRule {
        country_code: "MX",
        member_path: "taxRegistrationEntry.verificationDetails.taxRegistrationDocuments",
        error_code: ERROR_CODE_MISSING_INPUT,
        message: "A tax registration in Mexico must provide a Constancia de Situacion Fiscal \
                  (CSF) document in verificationDetails.taxRegistrationDocuments.",
        applies: always,
        satisfied: has_registration_document,
    },
    // Nepal: "The sector valid values are Business and Individual."
    CountryRule {
        country_code: "NP",
        member_path: "taxRegistrationEntry.sector",
        error_code: ERROR_CODE_FIELD_VALIDATION_FAILED,
        message: "The valid sector values for a tax registration in Nepal are Business and \
                  Individual.",
        applies: always,
        satisfied: sector_is_business_or_individual,
    },
    // Saudi Arabia: "For address, you must specify addressLine3."
    CountryRule {
        country_code: "SA",
        member_path: "taxRegistrationEntry.legalAddress.addressLine3",
        error_code: ERROR_CODE_MISSING_INPUT,
        message: "A tax registration in Saudi Arabia must specify legalAddress.addressLine3.",
        applies: always,
        satisfied: |entry: &TaxRegistrationEntry| {
            entry
                .legal_address
                .as_ref()
                .is_some_and(|address| address.address_line3.is_some())
        },
    },
    // South Korea: "You must specify the certifiedEmailId and legalName in the
    // TaxRegistrationEntry object." (`legalName` is required for every country
    // here — see `ops/put_tax_registration.rs`.)
    CountryRule {
        country_code: "KR",
        member_path: "taxRegistrationEntry.certifiedEmailId",
        error_code: ERROR_CODE_MISSING_INPUT,
        message: "A tax registration in South Korea must specify certifiedEmailId.",
        applies: always,
        satisfied: |entry: &TaxRegistrationEntry| entry.certified_email_id.is_some(),
    },
    // South Korea: "You must specify the businessRepresentativeName,
    // itemOfBusiness, and lineOfBusiness in the southKoreaAdditionalInfo field of
    // the additionalTaxInformation object." All three are `@required` on
    // `SouthKoreaAdditionalInfo`, so the boundary validator refuses a block
    // missing any one: what is left to check is that the block is there.
    CountryRule {
        country_code: "KR",
        member_path: "additionalTaxInformation.southKoreaAdditionalInfo",
        error_code: ERROR_CODE_MISSING_INPUT,
        message: ADDITIONAL_INFO_INVALID,
        applies: always,
        satisfied: |entry: &TaxRegistrationEntry| {
            additional_info(entry).is_some_and(|info| info.south_korea_additional_info.is_some())
        },
    },
    // South Korea: "You must specify the tax registration certificate document in
    // the taxRegistrationDocuments field of the VerificationDetails object."
    CountryRule {
        country_code: "KR",
        member_path: "taxRegistrationEntry.verificationDetails.taxRegistrationDocuments",
        error_code: ERROR_CODE_MISSING_INPUT,
        message: "A tax registration in South Korea must specify the tax registration certificate \
                  document in verificationDetails.taxRegistrationDocuments.",
        applies: always,
        satisfied: has_registration_document,
    },
    // Spain: "You must specify the registrationType in the spainAdditionalInfo
    // field of the additionalTaxInformation object."
    CountryRule {
        country_code: "ES",
        member_path: "additionalTaxInformation.spainAdditionalInfo.registrationType",
        error_code: ERROR_CODE_MISSING_INPUT,
        message: ADDITIONAL_INFO_INVALID,
        applies: always,
        satisfied: |entry: &TaxRegistrationEntry| spain_registration_type(entry).is_some(),
    },
    // Spain: "If the registrationType is Local, you must specify the tax
    // registration certificate document in the taxRegistrationDocuments field of
    // the VerificationDetails object."
    CountryRule {
        country_code: "ES",
        member_path: "taxRegistrationEntry.verificationDetails.taxRegistrationDocuments",
        error_code: ERROR_CODE_MISSING_INPUT,
        message: "A Local tax registration in Spain must specify the tax registration certificate \
                  document in verificationDetails.taxRegistrationDocuments.",
        applies: |entry: &TaxRegistrationEntry| {
            spain_registration_type(entry)
                .is_some_and(|value| value == SPAIN_REGISTRATION_TYPE_NEEDING_DOCUMENT)
        },
        satisfied: has_registration_document,
    },
    // Turkey: "You must specify the sector in the taxRegistrationEntry object."
    CountryRule {
        country_code: "TR",
        member_path: "taxRegistrationEntry.sector",
        error_code: ERROR_CODE_MISSING_INPUT,
        message: "A tax registration in Turkey must specify sector.",
        applies: always,
        satisfied: |entry: &TaxRegistrationEntry| sector(entry).is_some(),
    },
    // Turkey: "For address, you must specify districtOrCounty."
    CountryRule {
        country_code: "TR",
        member_path: "taxRegistrationEntry.legalAddress.districtOrCounty",
        error_code: ERROR_CODE_MISSING_INPUT,
        message: "A tax registration in Turkey must specify legalAddress.districtOrCounty.",
        applies: always,
        satisfied: |entry: &TaxRegistrationEntry| {
            entry
                .legal_address
                .as_ref()
                .is_some_and(|address| address.district_or_county.is_some())
        },
    },
    // Turkey: "Specify the taxOffice. If your sector is Individual, don't enter
    // this value." — required for the other sectors, refused for Individual.
    CountryRule {
        country_code: "TR",
        member_path: "additionalTaxInformation.turkeyAdditionalInfo.taxOffice",
        error_code: ERROR_CODE_MISSING_INPUT,
        message: ADDITIONAL_INFO_INVALID,
        applies: |entry: &TaxRegistrationEntry| {
            sector(entry).is_some() && !turkey_sector_is_individual(entry)
        },
        satisfied: |entry: &TaxRegistrationEntry| {
            turkey_member(entry, |turkey| turkey.tax_office.as_ref()).is_some()
        },
    },
    CountryRule {
        country_code: "TR",
        member_path: "additionalTaxInformation.turkeyAdditionalInfo.taxOffice",
        error_code: ERROR_CODE_FIELD_VALIDATION_FAILED,
        message: ADDITIONAL_INFO_INVALID,
        applies: turkey_sector_is_individual,
        satisfied: |entry: &TaxRegistrationEntry| {
            turkey_member(entry, |turkey| turkey.tax_office.as_ref()).is_none()
        },
    },
    // Turkey: "(Optional) Specify the kepEmailId. If your sector is Individual,
    // don't enter this value."
    CountryRule {
        country_code: "TR",
        member_path: "additionalTaxInformation.turkeyAdditionalInfo.kepEmailId",
        error_code: ERROR_CODE_FIELD_VALIDATION_FAILED,
        message: ADDITIONAL_INFO_INVALID,
        applies: turkey_sector_is_individual,
        satisfied: |entry: &TaxRegistrationEntry| {
            turkey_member(entry, |turkey| turkey.kep_email_id.as_ref()).is_none()
        },
    },
    // Ukraine: "The sector valid values are Business and Individual."
    CountryRule {
        country_code: "UA",
        member_path: "taxRegistrationEntry.sector",
        error_code: ERROR_CODE_FIELD_VALIDATION_FAILED,
        message: "The valid sector values for a tax registration in Ukraine are Business and \
                  Individual.",
        applies: always,
        satisfied: sector_is_business_or_individual,
    },
    // France: "You must specify the sirenNumber in the franceAdditionalInfo field
    // of the additionalTaxInformation object." (`sirenNumber` is `@required` on
    // `FranceAdditionalInfo`, so carrying the block is carrying the member.)
    CountryRule {
        country_code: "FR",
        member_path: "additionalTaxInformation.franceAdditionalInfo.sirenNumber",
        error_code: ERROR_CODE_MISSING_INPUT,
        message: ADDITIONAL_INFO_INVALID,
        applies: always,
        satisfied: |entry: &TaxRegistrationEntry| {
            additional_info(entry).is_some_and(|info| info.france_additional_info.is_some())
        },
    },
    // Monaco: "You must specify the businessNumber in the monacoAdditionalInfo
    // field of the additionalTaxInformation object." (`businessNumber` is
    // `@required` on `MonacoAdditionalInfo`.)
    CountryRule {
        country_code: "MC",
        member_path: "additionalTaxInformation.monacoAdditionalInfo.businessNumber",
        error_code: ERROR_CODE_MISSING_INPUT,
        message: ADDITIONAL_INFO_INVALID,
        applies: always,
        satisfied: |entry: &TaxRegistrationEntry| {
            additional_info(entry).is_some_and(|info| info.monaco_additional_info.is_some())
        },
    },
];

/// Refuse an entry that does not satisfy the per-country business rules
/// `PutTaxRegistration` documents for the country of its legal address.
///
/// The country is `legalAddress.countryCode`; an entry with no legal address has
/// no country to apply a rule for, and is refused by the caller for omitting a
/// member every shape that reports a registration requires. The first unmet rule
/// in [`COUNTRY_RULES`] is the refusal, so the rule set is shared by every writer
/// of a `TaxRegistrationEntry` rather than re-stated per operation.
pub fn check_country_requirements(
    entry: &TaxRegistrationEntry,
) -> Result<(), ValidationException> {
    let Some(country_code) = entry
        .legal_address
        .as_ref()
        .map(|address| address.country_code.as_str())
    else {
        return Ok(());
    };
    for rule in COUNTRY_RULES {
        if !rule.country_code.eq_ignore_ascii_case(country_code) {
            continue;
        }
        if !(rule.applies)(entry) || (rule.satisfied)(entry) {
            continue;
        }
        return Err(validation(rule.error_code, rule.member_path, rule.message));
    }
    Ok(())
}

/// Refuse a `TaxRegistrationEntry` no write of this service may store.
///
/// The one place both writers — `PutTaxRegistration` and the per-account loop of
/// `BatchPutTaxRegistration` — decide whether an entry is storable, so the two
/// operations cannot disagree about one entry: the structural refusals (a legal
/// address submitted for a type AWS derives it from, a missing legal name or
/// address, a blank value the boundary validator's regex engine could not refuse)
/// and then the per-country business rules [`COUNTRY_RULES`] carries. Why each is
/// not left to the boundary validator is spelled out on the helpers this calls
/// and in `ops/put_tax_registration.rs`.
pub fn check_tax_registration_entry(
    entry: &TaxRegistrationEntry,
) -> Result<(), ValidationException> {
    // A registration type whose legal address AWS derives from the tax number
    // itself does not take one from the request.
    check_registration_type_members(entry)?;

    // Both shapes that report a registration (`TaxRegistration`,
    // `TaxRegistrationWithJurisdiction`) mark `legalName` and `legalAddress`
    // `@required`, so a record missing either could never be read back: refuse
    // the write rather than store one.
    let Some(legal_name) = entry.legal_name.as_deref() else {
        return Err(validation(
            ERROR_CODE_MISSING_INPUT,
            "taxRegistrationEntry.legalName",
            "legalName is required to store a tax registration.",
        ));
    };
    let Some(legal_address) = entry.legal_address.as_ref() else {
        return Err(validation(
            ERROR_CODE_MISSING_INPUT,
            "taxRegistrationEntry.legalAddress",
            "legalAddress is required to store a tax registration.",
        ));
    };

    check_not_blank(
        "taxRegistrationEntry.registrationId",
        &entry.registration_id,
    )?;
    check_not_blank("taxRegistrationEntry.legalName", legal_name)?;
    check_address("taxRegistrationEntry.legalAddress", legal_address)?;

    // The per-country business rules the model documents for a
    // `TaxRegistrationEntry`, keyed off `legalAddress.countryCode`. They run last
    // because they are about an entry that is otherwise storable.
    check_country_requirements(entry)
}

/// Whether this emulator stores a supplemental TRN for `country_code`, with the
/// country's name — see [`SUPPLEMENTAL_TRN_COUNTRIES`].
pub fn supplemental_country(country_code: &str) -> Option<&'static str> {
    SUPPLEMENTAL_TRN_COUNTRIES
        .iter()
        .find(|(code, _)| code.eq_ignore_ascii_case(country_code))
        .map(|(_, country)| *country)
}

/// Refuse a `SupplementalTaxRegistrationEntry` for a jurisdiction this service
/// does not store a supplemental TRN for ([`SUPPLEMENTAL_TRN_COUNTRIES`]).
///
/// The members the boundary validator's regex engine could not refuse (the
/// non-blank pattern on `registrationId`, `legalName` and the address lines) are
/// checked by the handler through [`check_not_blank`] / [`check_address`]; this
/// is the jurisdiction boundary alone.
pub fn check_supplemental_jurisdiction(
    entry: &SupplementalTaxRegistrationEntry,
) -> Result<(), ValidationException> {
    if supplemental_country(&entry.address.country_code).is_some() {
        return Ok(());
    }
    Err(validation(
        ERROR_CODE_FIELD_VALIDATION_FAILED,
        "taxRegistrationEntry.address.countryCode",
        "Tax registration for country is not supported.",
    ))
}

/// This service's modelled `ValidationException` for an `authorityId` that names
/// no live supplemental registration of the caller's account.
///
/// `DeleteSupplementalTaxRegistration` also models `ResourceNotFoundException`,
/// but the real service answers an unknown `authorityId` — a junk string and a
/// well-formed UUID alike — with the 400 class, so this is the refusal both get:
/// an `authorityId` is valid only while it names a registration. The model's
/// two doc strings do not choose between the errors; the recorded units
/// `taxsettings.delete-supplemental-tax-registration-not-found` and
/// `taxsettings.concurrent-delete-supplemental-tax-registration-consistent` do,
/// and `tests/all/missing_resource_errors.rs` pins their outcome against the
/// 404 the `accountId`-addressed operations answer, so the two classes cannot be
/// swapped by re-reading the docs. A later fixer holding a recording that
/// disagrees should follow the recording.
pub fn invalid_authority_id() -> ValidationException {
    validation(
        ERROR_CODE_FIELD_VALIDATION_FAILED,
        "authorityId",
        "Provided authorityId is not valid.",
    )
}

/// This service's modelled `ResourceNotFoundException` (HTTP 404) for an account
/// that has no stored tax registration.
///
/// It carries its `message` and nothing else. `errorCode` on this service's
/// exceptions is a SERVICE-DEFINED code, never the exception's own shape name —
/// the wire already carries that in `x-amzn-errortype` — and this service
/// publishes no code for a missing resource (`ErrorCode` is a free-form string,
/// unlike the `ValidationExceptionErrorCode` enum), so the member is left off the
/// wire rather than filled with the shape name; see `ops/types/shared_types.rs`.
/// The message names no account: the request that asked already names it.
pub fn no_tax_registration() -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: "No tax registration found".to_string(),
    }
}

/// This service's modelled `ResourceNotFoundException` (HTTP 404) for an
/// exemption read that names an account this service holds no tax-settings
/// resource for — one outside the caller's consolidated billing family (see
/// [`ComAmazonawsTaxsettingsTaxsettingsState::reports_exemptions_for`]).
///
/// It carries its `message` only, for the reason [`no_tax_registration`] gives.
/// The 404 and the `ResourceNotFoundException` shape are the contract; the
/// wording is this emulator's own, built from the request so the caller is told
/// WHICH of the accounts it named is the one with nothing behind it.
pub fn no_exemptions_for_account(account_id: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: format!(
            "Account {account_id} is not in your consolidated billing family, so it has no tax \
             exemptions to report."
        ),
    }
}

/// This service's modelled `ConflictException` (HTTP 409) for a write that would
/// take the store past [`MAX_STORED_REGISTRATIONS`]. It carries its `message`
/// only, for the reason [`no_tax_registration`] gives.
pub fn registration_store_full() -> ConflictException {
    ConflictException {
        message: format!(
            "This emulator keeps at most {MAX_STORED_REGISTRATIONS} tax registrations resident. \
             Delete one before adding another."
        ),
    }
}

/// This service's modelled `ConflictException` (HTTP 409) for a write aimed at
/// an account whose stored TRN is still [`STATUS_PENDING`].
///
/// Both writers of a tax registration document the rule: "You can't set a TRN if
/// there's a pending TRN. You'll need to delete the pending TRN first"
/// (`smithy_query(query="doc",shape="com.amazonaws.taxsettings#PutTaxRegistration")`,
/// and the same sentence on `BatchPutTaxRegistration`). The 409 class and the
/// `ConflictException` shape are the contract; the wording below is this
/// emulator's own statement of the rule — AWS publishes no message for it — and
/// names no account, for the reason [`no_tax_registration`] gives.
pub fn pending_registration_conflict() -> ConflictException {
    ConflictException {
        message: "You can't set a TRN while there's a pending TRN for this account. Delete the \
                  pending TRN first."
            .to_string(),
    }
}

/// This service's modelled `ConflictException` (HTTP 409) for a write that would
/// take the supplemental store past [`MAX_STORED_SUPPLEMENTAL_REGISTRATIONS`].
/// It carries its `message` only, for the reason [`no_tax_registration`] gives.
pub fn supplemental_store_full() -> ConflictException {
    ConflictException {
        message: format!(
            "This emulator keeps at most {MAX_STORED_SUPPLEMENTAL_REGISTRATIONS} supplemental tax \
             registrations resident. Delete one before adding another."
        ),
    }
}

/// This service's modelled `InternalServerException` (HTTP 500) for a handler
/// mounted on an engine that holds no state for this service. A handler answers
/// this rather than building an engine of its own, which would give that one
/// request a private store. It carries its `message` only, for the reason
/// [`no_tax_registration`] gives.
pub fn state_unavailable() -> InternalServerException {
    InternalServerException {
        message: "This service's state store is unavailable.".to_string(),
    }
}

/// The opaque page cursor naming the last store key a page emitted — an account
/// id for `ListTaxRegistrations`, an `authorityId` for
/// `ListSupplementalTaxRegistrations`.
///
/// Resuming from it means "the first key strictly greater than this one", which
/// is well defined because both
/// [`ComAmazonawsTaxsettingsTaxsettingsState::registrations`] and
/// [`ComAmazonawsTaxsettingsTaxsettingsState::supplemental_registrations`] are
/// ordered.
///
/// Encoded with `aws_smithy_types::base64`, the codec the wire protocols already
/// use for this service's `blob` members. Its standard alphabet (`A-Za-z0-9+/`)
/// and `=` padding both lie inside the model's `PaginationTokenString` pattern
/// `^[-A-Za-z0-9_+\=\/]+$`, so every token issued here satisfies the constraint
/// the next request's `nextToken` is validated against.
pub fn encode_page_token(last_key: &str) -> String {
    base64::encode(last_key)
}

/// The account id a page cursor names, or a `ValidationException` when the token
/// was not minted by [`encode_page_token`].
pub fn decode_page_token(token: &str) -> Result<String, ValidationException> {
    let account_id = decode_page_token_key(token)?;
    if account_id.len() != 12 || !account_id.bytes().all(|b| b.is_ascii_digit()) {
        return Err(validation(
            ERROR_CODE_INVALID_TOKEN,
            "nextToken",
            "nextToken does not name an account this service can resume from.",
        ));
    }
    Ok(account_id)
}

/// One page of a listing: the entries it reports, in store order, and the cursor
/// a caller continues from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Page<T> {
    /// The entries this page reports, at most the page size asked of
    /// [`build_page`] of them, in the order the store enumerates them.
    pub items: Vec<T>,
    /// The cursor naming the last key this page emitted — issued only when a
    /// further entry remains, so the last page carries none.
    pub next_token: Option<String>,
}

/// Build one page of a listing over an ordered store: resume past the key the
/// cursor names, report up to `page_size` entries, and issue a cursor naming the
/// last key of the page only when a further entry remains.
///
/// The ONE implementation of this service's page-build rule, so every listing
/// pages the same way and a change to paging — a new listing, or a change to how
/// a cursor is minted ([`encode_page_token`], beside this) — has a single place
/// to touch.
///
/// `project` turns a stored entry into the shape its listing reports, and
/// answers `None` for one with nothing to report: such an entry is skipped
/// rather than reported empty, and its key never becomes a cursor.
///
/// Resuming uses the ordered map's own `range`, so a cursor's page costs a
/// lookup rather than a scan of everything before it, and the work done here is
/// bounded by the page — `page_size` reported entries, plus the entries
/// `project` skips and the one peeked at to decide the cursor — not by the size
/// of the store. What a CALLER of this does under the state lock besides
/// building the page (settling what the clock owes, say) is its own cost.
///
/// `page_size` must be at least one: a page of none would report nothing and
/// carry no cursor, leaving a caller unable to continue. Every caller takes it
/// from [`PageBounds::page_size`], which never answers zero, and the assertion
/// below keeps that where the rule lives rather than at each listing.
pub fn build_page<V, T, F>(
    source: &BTreeMap<String, V>,
    resume_after: Option<&str>,
    page_size: usize,
    mut project: F,
) -> Page<T>
where
    F: FnMut(&str, &V) -> Option<T>,
{
    debug_assert!(page_size > 0, "a page carries at least one entry");
    let lower = match resume_after {
        Some(after) => Bound::Excluded(after),
        None => Bound::Unbounded,
    };
    let mut remaining = source
        .range::<str, _>((lower, Bound::Unbounded))
        .filter_map(|(key, stored)| project(key, stored).map(|item| (key.as_str(), item)))
        .peekable();
    let mut items = Vec::new();
    let mut last_key: Option<&str> = None;
    for (key, item) in remaining.by_ref().take(page_size) {
        last_key = Some(key);
        items.push(item);
    }
    let next_token = if remaining.peek().is_some() {
        last_key.map(encode_page_token)
    } else {
        None
    };
    Page { items, next_token }
}

// ---------------------------------------------------------------------------
// The tax exemption type catalog
//
// `GetTaxExemptionTypes` answers "the supported types of tax exemptions": a
// collection the SERVICE publishes, the same for every account, that no
// operation of this service creates, updates or deletes. So the catalog below IS
// that collection — the same way the API Gateway emulator's `SdkType` collection
// is a published table rather than a stored record — and the handler projects it
// into the modelled shapes instead of reading an account's records.
// ---------------------------------------------------------------------------

/// One jurisdiction an exemption type applies in, as the catalog spells it:
/// `(country, state)` — the two members of the model's `Authority` shape
/// (`smithy_query(query="shape", shape="Authority")`: `country` is a required
/// two-letter code, `state` an optional name). `None` means the type applies
/// across the whole country rather than in one state of it.
pub type JurisdictionRow = (&'static str, Option<&'static str>);

/// One entry of the published catalog:
/// `(displayName, description, applicableJurisdictions)` — the three members of
/// the model's `TaxExemptionType` shape.
pub type TaxExemptionTypeRow = (&'static str, &'static str, &'static [JurisdictionRow]);

/// The tax exemption types this service supports.
///
/// Hand-written because the model declares no shape enumerating them: `TaxExemptionType`
/// is a free-form structure, and the only thing the model says about which values are
/// legal is `PutTaxExemptionRequest$exemptionType`'s "Use the supported tax exemption type
/// description" (`smithy_query(query="doc", shape="PutTaxExemptionRequest",
/// member="exemptionType")`) — i.e. the `description` of an entry of THIS catalog is the
/// token a caller names an exemption type by, so the descriptions below are the catalog's
/// identifying values and are kept distinct from one another.
///
/// What is claimed to match AWS: the categories of US sales-and-use-tax exemption AWS
/// accepts a certificate for (AWS Billing and Cost Management User Guide, "Tax exemption"
/// / "Managing your tax exemptions": tax exemptions are offered for United States sales
/// tax, under the certificate categories a reseller, a direct-pay permit holder, a
/// federal, state, local or tribal government body, an educational institution or a
/// section 501(c)(3) charitable organization holds), and the single country-level
/// jurisdiction each of them carries — `US`, with no `state`, because the exemption
/// category is published for the country and a certificate's state is a fact about the
/// certificate rather than about the type.
///
/// What is NOT claimed: the `displayName` and `description` WORDING, and the order of the
/// entries. The model states no sentence for either member, so each is this emulator's own
/// phrasing of a documented category, sorted by `displayName` so the collection is
/// enumerated one stable way; a client that keys off the prose rather than off a type it
/// already read from this catalog is reading wording the real service words differently.
pub const TAX_EXEMPTION_TYPE_CATALOG: &[TaxExemptionTypeRow] = &[
    (
        "501(c)(3)",
        "Charitable organization exempt under section 501(c)(3) of the US Internal Revenue Code",
        UNITED_STATES,
    ),
    (
        "Direct Pay Permit",
        "Holder of a direct pay permit that remits sales and use tax directly to the taxing \
         authority",
        UNITED_STATES,
    ),
    (
        "Educational Institution",
        "Accredited school, college or university exempt from sales and use tax",
        UNITED_STATES,
    ),
    (
        "Federal Government",
        "US federal government body exempt from sales and use tax",
        UNITED_STATES,
    ),
    (
        "Reseller",
        "Purchases for resale, held under a resale certificate",
        UNITED_STATES,
    ),
    (
        "State or Local Government",
        "US state, county or municipal government body exempt from sales and use tax",
        UNITED_STATES,
    ),
    (
        "Tribal Government",
        "Federally recognized tribal government body exempt from sales and use tax",
        UNITED_STATES,
    ),
];

/// The jurisdiction every entry of [`TAX_EXEMPTION_TYPE_CATALOG`] applies in: the
/// United States as a whole. See that constant for why it carries no `state`.
const UNITED_STATES: &[JurisdictionRow] = &[("US", None)];

/// The catalog as the modelled `TaxExemptionType` list `GetTaxExemptionTypes`
/// reports, in [`TAX_EXEMPTION_TYPE_CATALOG`]'s order.
///
/// Every member is reported: the collection exists to tell a caller which types
/// it may name and where each one applies, and an omitted member would leave that
/// unanswerable.
pub fn tax_exemption_type_catalog() -> Vec<TaxExemptionType> {
    TAX_EXEMPTION_TYPE_CATALOG
        .iter()
        .map(exemption_type_of)
        .collect()
}

/// One published row as the modelled `TaxExemptionType` shape, which is how both
/// the catalog listing and a stored exemption report the type it was granted
/// under: one projection, so the two cannot describe one type differently.
pub fn exemption_type_of(
    (display_name, description, jurisdictions): &TaxExemptionTypeRow,
) -> TaxExemptionType {
    TaxExemptionType {
        display_name: Some((*display_name).to_string()),
        description: Some((*description).to_string()),
        applicable_jurisdictions: Some(
            jurisdictions
                .iter()
                .map(|(country, state)| Authority {
                    country: (*country).to_string(),
                    state: state.map(std::string::ToString::to_string),
                })
                .collect(),
        ),
    }
}

/// The published row a caller names by `exemption_type`.
///
/// `PutTaxExemptionRequest$exemptionType` documents "Use the supported tax
/// exemption type description", so the `description` of a
/// [`TAX_EXEMPTION_TYPE_CATALOG`] entry is the token a type is named by; the
/// `displayName` is accepted for the same entry, because it is the catalog's
/// other identifying member and that wording is this emulator's own (see
/// [`TAX_EXEMPTION_TYPE_CATALOG`]) rather than something a client could have read
/// off AWS. Matching ignores case and surrounding whitespace, neither of which
/// distinguishes one published type from another.
pub fn find_exemption_type(exemption_type: &str) -> Option<&'static TaxExemptionTypeRow> {
    let wanted = exemption_type.trim();
    TAX_EXEMPTION_TYPE_CATALOG
        .iter()
        .find(|(display_name, description, _)| {
            description.eq_ignore_ascii_case(wanted) || display_name.eq_ignore_ascii_case(wanted)
        })
}

/// Refuse an `exemptionType` the published catalog does not name, answering the
/// row it does name.
///
/// The exemption types are a collection the SERVICE publishes
/// (`GetTaxExemptionTypes`), and the write documents that a request names one of
/// them, so a request naming anything else asks for an exemption this service
/// does not offer: a field violation naming that parameter, never a stored record
/// under a type no reader could describe.
pub fn check_exemption_type(
    exemption_type: &str,
) -> Result<&'static TaxExemptionTypeRow, ValidationException> {
    find_exemption_type(exemption_type).ok_or_else(|| {
        // The message names the member and what it must satisfy, never the value
        // that was refused — the shape every refusal of this service takes.
        validation(
            ERROR_CODE_FIELD_VALIDATION_FAILED,
            "exemptionType",
            "exemptionType must be the description of a tax exemption type \
             GetTaxExemptionTypes publishes.",
        )
    })
}

/// Refuse an `authority` outside the jurisdictions the exemption type applies
/// in.
///
/// A published type carries the `applicableJurisdictions` it applies in, so an
/// exemption asked for in any other jurisdiction is one this service does not
/// offer. A jurisdiction naming a country only applies throughout that country,
/// so the request may name a state of it (a certificate is held in one) or none;
/// a jurisdiction naming a state applies in that state alone.
pub fn check_exemption_authority(
    (_, description, jurisdictions): &TaxExemptionTypeRow,
    authority: &Authority,
) -> Result<(), ValidationException> {
    let applies = jurisdictions.iter().any(|(country, state)| {
        country.eq_ignore_ascii_case(authority.country.trim())
            && (*state).is_none_or(|state| {
                authority
                    .state
                    .as_deref()
                    .is_some_and(|submitted| submitted.trim().eq_ignore_ascii_case(state))
            })
    });
    if applies {
        return Ok(());
    }
    Err(validation(
        ERROR_CODE_FIELD_VALIDATION_FAILED,
        "authority",
        format!(
            "The {description} tax exemption does not apply in the jurisdiction this request \
             names. Use a jurisdiction GetTaxExemptionTypes publishes for that type."
        ),
    ))
}

// ---------------------------------------------------------------------------
// Writing a tax exemption
//
// `PutTaxExemption` is the one writer of
// [`ComAmazonawsTaxsettingsTaxsettingsState::exemptions`]; the record it stores
// and the projection every reader of one reports are [`StoredTaxExemption`],
// above. What lives here is what a submission needs on top of that record: the
// bound the store keeps, and the identity two submissions share.
// ---------------------------------------------------------------------------

/// The most tax exemption records this emulator keeps resident, across every
/// account.
///
/// `accountIds` is caller-supplied (up to 550 ids per call), so without a bound a
/// caller could grow the store by hundreds of records per request, and this
/// service models no operation that deletes an exemption. A submission that would
/// ADD records past this bound is refused with `PutTaxExemption`'s modelled
/// `CaseCreationLimitExceededException` — the refusal AWS gives when no further
/// support case may be opened, and each submission opens one — while replacing
/// exemptions the accounts already hold is always allowed, because it adds
/// nothing.
pub const MAX_STORED_EXEMPTIONS: usize = 1_000;

/// `authority` as a record stores it: the country upper-cased, because
/// `Authority$country` is a two-letter code whose case carries no meaning, and
/// the state trimmed but otherwise as submitted, being a name.
pub fn normalized_authority(authority: &Authority) -> Authority {
    Authority {
        country: authority.country.trim().to_uppercase(),
        state: authority
            .state
            .as_deref()
            .map(|state| state.trim().to_string()),
    }
}

/// Whether `stored` is the same exemption as the one `description` and
/// `authority` name.
///
/// The type and the jurisdiction are the identity a submission addresses — the
/// request carries no exemption id — so a second submission of the same pair
/// replaces the account's record rather than adding one beside it. Case and
/// padding make no second exemption: they are compared the way
/// [`check_exemption_authority`] admits a jurisdiction, so what the admission
/// check treats as one jurisdiction the store treats as one record.
pub fn is_same_exemption(
    stored: &StoredTaxExemption,
    description: &str,
    authority: &Authority,
) -> bool {
    let same_state = match (stored.authority.state.as_deref(), authority.state.as_deref()) {
        (Some(held), Some(submitted)) => held.trim().eq_ignore_ascii_case(submitted.trim()),
        (None, None) => true,
        _ => false,
    };
    stored
        .exemption_type
        .description
        .as_deref()
        .is_some_and(|held| held.eq_ignore_ascii_case(description))
        && stored
            .authority
            .country
            .trim()
            .eq_ignore_ascii_case(authority.country.trim())
        && same_state
}

/// This service's modelled `CaseCreationLimitExceededException` (HTTP 413) for a
/// submission that would take the store past [`MAX_STORED_EXEMPTIONS`]. It
/// carries its `message` only, for the reason [`no_tax_registration`] gives.
pub fn exemption_store_full() -> CaseCreationLimitExceededException {
    CaseCreationLimitExceededException {
        message: format!(
            "This emulator keeps at most {MAX_STORED_EXEMPTIONS} tax exemptions resident, so no \
             further exemption case can be created."
        ),
    }
}

/// The `authorityId` a supplemental-listing page cursor names, or a
/// `ValidationException` when the token was not minted by
/// [`encode_page_token`].
///
/// The key a supplemental page resumes from is an `authorityId`, which the model
/// constrains as a `GenericString` of 1..=200 characters, so a token carrying
/// anything outside that bound names nothing this listing could resume from.
pub fn decode_supplemental_page_token(token: &str) -> Result<String, ValidationException> {
    let authority_id = decode_page_token_key(token)?;
    if authority_id.is_empty() || authority_id.chars().count() > 200 {
        return Err(validation(
            ERROR_CODE_INVALID_TOKEN,
            "nextToken",
            "nextToken does not name a supplemental tax registration this service can resume \
             from.",
        ));
    }
    Ok(authority_id)
}

/// The store key a page cursor carries, whatever kind of key that listing pages
/// by: the text [`encode_page_token`] encoded, or a `ValidationException` when
/// the token does not decode at all.
fn decode_page_token_key(token: &str) -> Result<String, ValidationException> {
    let malformed = || {
        validation(
            ERROR_CODE_MALFORMED_TOKEN,
            "nextToken",
            "nextToken is not a pagination token this service issued.",
        )
    };
    let decoded = base64::decode(token).map_err(|_| malformed())?;
    String::from_utf8(decoded).map_err(|_| malformed())
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsTaxsettingsTaxsettingsState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsTaxsettingsTaxsettingsState>)
}
