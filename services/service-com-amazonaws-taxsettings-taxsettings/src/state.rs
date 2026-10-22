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
    InternalServerException, Jurisdiction, ResourceNotFoundException, TaxDocumentMetadata,
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
/// Turkey's `districtOrCounty`, Kenya's `personType`, …) are a different class
/// and are not enforced here; see this module's siblings in `src/ops/`.
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

/// The opaque page cursor naming the last account id a page emitted.
///
/// Resuming from it means "the first key strictly greater than this one", which
/// is well defined because [`ComAmazonawsTaxsettingsTaxsettingsState::registrations`]
/// is ordered.
///
/// Encoded with `aws_smithy_types::base64`, the codec the wire protocols already
/// use for this service's `blob` members. Its standard alphabet (`A-Za-z0-9+/`)
/// and `=` padding both lie inside the model's `PaginationTokenString` pattern
/// `^[-A-Za-z0-9_+\=\/]+$`, so every token issued here satisfies the constraint
/// the next request's `nextToken` is validated against.
pub fn encode_page_token(last_account_id: &str) -> String {
    base64::encode(last_account_id)
}

/// The account id a page cursor names, or a `ValidationException` when the token
/// was not minted by [`encode_page_token`].
pub fn decode_page_token(token: &str) -> Result<String, ValidationException> {
    let decoded = base64::decode(token).map_err(|_| {
        validation(
            ERROR_CODE_MALFORMED_TOKEN,
            "nextToken",
            "nextToken is not a pagination token this service issued.",
        )
    })?;
    let account_id = String::from_utf8(decoded).map_err(|_| {
        validation(
            ERROR_CODE_MALFORMED_TOKEN,
            "nextToken",
            "nextToken is not a pagination token this service issued.",
        )
    })?;
    if account_id.len() != 12 || !account_id.bytes().all(|b| b.is_ascii_digit()) {
        return Err(validation(
            ERROR_CODE_INVALID_TOKEN,
            "nextToken",
            "nextToken does not name an account this service can resume from.",
        ));
    }
    Ok(account_id)
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
