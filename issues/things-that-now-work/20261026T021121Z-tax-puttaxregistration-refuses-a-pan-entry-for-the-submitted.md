Issue-Source: trace-sweep-divergence
Issue-Trace: taxsettings-put-tax-registration-validation-error
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-ga-prod-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: taxsettings-put-tax-registration-validation-error
Issue-Shape-Id: com.amazonaws.taxsettings#TaxSettings
Issue-Key: tax|PutTaxRegistration|4xx->4xx|$.errorCode
Issue-Key: tax|PutTaxRegistration|4xx->4xx|$.fieldList[*]
Issue-Key: tax|PutTaxRegistration|4xx->4xx|$.message
Issue-Commit: 52c6ce358d4c16be3768ecaa3eb172ccf7ba3dcc

# PutTaxRegistration refuses a PAN entry for the submitted address's country instead of refusing the address itself

> _Note: this issue was identified against the code base at commit `52c6ce358d4c16be3768ecaa3eb172ccf7ba3dcc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutTaxRegistration

## Description
Cause 1 of 1: for a TaxRegistrationEntry whose registrationType is India's PAN, the emulator reads the countryCode of a submitted legalAddress to decide whether the caller is an India customer, and refuses with the jurisdiction code, instead of first refusing the presence of a member that type does not accept at all.

AWS: PAN is India's Permanent Account Number and AWS derives a PAN registration's legal address from the tax number itself, so legalAddress is not the caller's to supply for that type. A PutTaxRegistration whose taxRegistrationEntry.registrationType is PAN and which carries a legalAddress is refused with the modeled ValidationException at HTTP 400 (Smithy: PutTaxRegistration errors ConflictException 409 / InternalServerException 500 / ValidationException 400; ValidationException members message @required, errorCode @required -> ValidationExceptionErrorCode enum [MalformedToken, ExpiredToken, InvalidToken, FieldValidationFailed, MissingInput, NonIndiaCustomerCanNotSetPAN, GSTExistenceBlockSetPAN], fieldList -> ValidationExceptionFieldList of ValidationExceptionField{name @required}). The recorded response carries errorCode 'FieldValidationFailed', a one-element fieldList naming the offending parameter, and the message 'Legal address should not be provided for India PAN Tax Registration.' In that message the fixed text is 'Legal address should not be provided for ' ... ' Tax Registration.'; the variable parts are the country the registration type belongs to ('India' for PAN) and the registrationType value taken from the request ('PAN'). The fieldList entry names the refused input member in this service's wire vocabulary for a parameter name, i.e. the legal-address member ('LEGAL_ADDRESS'), because that member is what may not be supplied. The country-of-the-customer refusal that errorCode NonIndiaCustomerCanNotSetPAN exists for is a different rule and is decided from the customer's own country, never from a legalAddress the same request is not allowed to carry — a request refused for supplying the address cannot simultaneously have that address read as evidence of where the customer is.

Emulator: the step replays a PutTaxRegistration PAN entry that submits a legalAddress and still answers HTTP 400, but with errorCode 'NonIndiaCustomerCanNotSetPAN', fieldList [{"name": "REGISTRATION_TYPE"}] and message 'Only a customer in India can set a PAN tax registration.' — where AWS answers errorCode 'FieldValidationFailed', fieldList [{"name": "LEGAL_ADDRESS"}] and 'Legal address should not be provided for India PAN Tax Registration.' The status matches; the error code, the named field and the message all diverge. The emulator already holds AWS's exact rule and message template, but applies it second: the entry checker runs the registration-type jurisdiction test (which compares the submitted legalAddress.countryCode against the type's country and emits the NonIndiaCustomerCanNotSetPAN code naming the registrationType parameter) before the test that refuses a legalAddress submitted for a type whose address is derived. So any PAN entry carrying an address for any country other than India is diverted into the jurisdiction refusal and the legal-address refusal is unreachable for exactly the inputs AWS reports it for.

Why: a client branches on ValidationException.errorCode and on fieldList[].name rather than parsing prose. Against AWS it sees FieldValidationFailed plus LEGAL_ADDRESS and corrects its request by dropping the address it must not send; against the emulator it sees NonIndiaCustomerCanNotSetPAN plus REGISTRATION_TYPE and concludes the account is not eligible for a PAN registration at all. The remediation the two answers direct are opposite: one is a fixable field-level mistake, the other reads as a permanent, account-level ineligibility. Any client-side mapping, retry policy or user-facing message keyed on the code therefore takes the wrong branch, and a correct request is never attempted.

Fix: the precedence inside the operation's shared entry validation — the per-entry validation helper that PutTaxRegistration and BatchPutTaxRegistration both call. The member-level refusal for a registration type whose legal address is derived must be decided BEFORE the registration-type jurisdiction refusal, so a submitted legalAddress is refused as a field violation and its countryCode is never consulted to judge the customer's country; the jurisdiction rule must only be reached for an entry whose address is one the type legitimately accepts. The message template, the FieldValidationFailed code and the LEGAL_ADDRESS parameter name the emulator already produces are correct and need no change — only the order in which the two tests run, and the same reordering must apply to the batch writer, which shares the helper.

Traces: taxsettings-put-tax-registration-validation-error#task#1.

## Evidence
Replay cluster (verdict=mismatch, service=tax, operation=PutTaxRegistration, expected_status=400, actual_status=400), divergence_key 'service=tax|PutTaxRegistration|status=400->400|keys=$.errorCode,$.fieldList[0],$.message':

  $.errorCode: expected 'FieldValidationFailed', got 'NonIndiaCustomerCanNotSetPAN'
  $.fieldList[0]: expected {'name': 'LEGAL_ADDRESS'}, got {'name': 'REGISTRATION_TYPE'}
  $.message: expected 'Legal address should not be provided for India PAN Tax Registration.', got 'Only a customer in India can set a PAN tax registration.'

Expected (AWS, recorded): HTTP 400 ValidationException, errorCode FieldValidationFailed, fieldList one entry naming the legal-address member, message 'Legal address should not be provided for <country-of-type> <registrationType> Tax Registration.' — fixed text 'Legal address should not be provided for ' / ' Tax Registration.', variable parts the type's country and the request's registrationType.

Actual (emulator): HTTP 400 ValidationException, errorCode NonIndiaCustomerCanNotSetPAN, fieldList one entry naming the registration-type member, message 'Only a customer in India can set a PAN tax registration.'

Model citations: Smithy PutTaxRegistration POST /PutTaxRegistration, errors ValidationException(400)/ConflictException(409)/InternalServerException(500); ValidationExceptionErrorCode values MalformedToken|ExpiredToken|InvalidToken|FieldValidationFailed|MissingInput|NonIndiaCustomerCanNotSetPAN|GSTExistenceBlockSetPAN; ValidationException.fieldList -> ValidationExceptionField{name @required, pattern ^(?!\s*$)[\s\S]+$}; TaxRegistrationEntry.legalAddress optional with the documented derive carve-outs.

Code evidence (not part of the requirement, cited only to locate the defect): services/service-com-amazonaws-taxsettings-taxsettings/src/state.rs, check_tax_registration_entry (~line 2331) calls check_registration_type_jurisdiction (~line 1348) before check_registration_type_members (~line 1318); the latter is the one that builds AWS's FieldValidationFailed / legalAddress / 'Legal address should not be provided for {country} {registration_type} Tax Registration.' refusal, so it is correct but unreachable for a PAN entry whose submitted address names a non-IN country. Shared with BatchPutTaxRegistration via the same helper (services/service-com-amazonaws-taxsettings-taxsettings/src/ops/put_tax_registration.rs line 286).

Not covered by any open issue: the open taxsettings PutTaxRegistration issues are 20261024T040357Z-taxsettings-puttaxregistration-wrong-behavior.md (Pending-TRN ConflictException) and 20261025T225427Z-taxsettings-puttaxregistration-validation--r2.md (Malaysia registrationType/sector pairing); neither concerns the PAN legal-address vs jurisdiction precedence, and this cluster reported no owned_by and no stub_path.

## Steps to Reproduce
1. Clone the repo and boot the emulator built from it.
2. Replay fixture 'taxsettings-put-tax-registration-validation-error', step 'task#1' — a PutTaxRegistration (POST /PutTaxRegistration) whose taxRegistrationEntry sets registrationType PAN and ALSO supplies a legalAddress whose countryCode is not IN.
3. Observe the response is HTTP 400 ValidationException in both cases, but the emulator's body carries errorCode 'NonIndiaCustomerCanNotSetPAN', fieldList [{"name": "REGISTRATION_TYPE"}] and message 'Only a customer in India can set a PAN tax registration.'
4. AWS answers errorCode 'FieldValidationFailed', fieldList [{"name": "LEGAL_ADDRESS"}] and 'Legal address should not be provided for India PAN Tax Registration.'
5. Confirm the fix by re-replaying taxsettings-put-tax-registration-validation-error#task#1; also send the same PAN-with-legalAddress entry through BatchPutTaxRegistration, which shares the entry validator, and check it reports the same FieldValidationFailed / LEGAL_ADDRESS refusal.
