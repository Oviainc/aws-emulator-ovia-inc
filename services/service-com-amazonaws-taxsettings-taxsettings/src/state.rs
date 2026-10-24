// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-taxsettings-taxsettings service.
//!
//! It holds the tax-registration (TRN) record of each account and the account's
//! tax-inheritance election, plus the projections every reader of a stored
//! record needs. The projections live here, beside the stored record, so
//! `GetTaxRegistration` and `ListTaxRegistrations` report one record the same
//! way instead of each rebuilding it from the raw fields.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use ::aws_smithy_types::base64;

use crate::ops::types::{
    AdditionalInfoRequest, AdditionalInfoResponse, Address, ConflictException,
    IndonesiaAdditionalInfo, InternalServerException, Jurisdiction, ResourceNotFoundException,
    SupplementalTaxRegistration, SupplementalTaxRegistrationEntry, TaxDocumentMetadata,
    TaxRegistration, TaxRegistrationEntry, TaxRegistrationWithJurisdiction, ValidationException,
    ValidationExceptionField,
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

/// The page `ListTaxRegistrations` builds when a request names no `maxResults`.
///
/// AWS documents no default page size for this operation, so this is the
/// emulator's own: a request that names none still gets a bounded page and a
/// cursor to continue from, so no single request can ask the emulator to
/// serialise the whole store into one response.
pub const DEFAULT_PAGE_SIZE: usize = 100;

/// The largest page `ListTaxRegistrations` builds, whatever `maxResults` asks
/// for.
///
/// Equal to the ceiling of the model's own `maxResults` range (1..=1000), so it
/// never truncates a page size the model admits; it pins that the SERVER, not
/// the request, owns the upper bound on one response's size and on the work done
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

/// The page size a listing request selects: what it asked for, else
/// [`DEFAULT_PAGE_SIZE`], never more than [`MAX_PAGE_SIZE`] and never zero.
pub fn page_size(max_results: Option<i32>) -> usize {
    max_results
        .and_then(|max| usize::try_from(max).ok())
        .unwrap_or(DEFAULT_PAGE_SIZE)
        .clamp(1, MAX_PAGE_SIZE)
}

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
            jurisdiction: Jurisdiction {
                state_or_region: self.legal_address.state_or_region.clone(),
                country_code: self.legal_address.country_code.clone(),
            },
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
