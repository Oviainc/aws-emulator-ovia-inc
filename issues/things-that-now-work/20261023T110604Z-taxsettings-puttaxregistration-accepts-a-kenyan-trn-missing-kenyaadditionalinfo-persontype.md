Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: taxsettings.put-tax-registration-missing-required-field-rejected
Issue-Shape-Id: com.amazonaws.taxsettings#TaxSettings
Issue-Key: taxsettings.put-tax-registration-missing-required-field-rejected|_error#4|AssertionError|raise AssertionError(f"expected the call to be rejected, but it succeeded and returned {result!r}")
Issue-Commit: 048778663a26e363156c6db724999c4fb229af36

# PutTaxRegistration accepts a Kenyan TRN missing kenyaAdditionalInfo.personType with 200 Pending instead of refusing it 400 ValidationException

> _Note: this issue was identified against the code base at commit `048778663a26e363156c6db724999c4fb229af36`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutTaxRegistration

## Description
Cause 1 of 1: PutTaxRegistration performs no per-country business-rule validation on taxRegistrationEntry, so an entry AWS refuses is accepted and persisted.
AWS: Smithy: PutTaxRegistration POST /PutTaxRegistration, modeled errors ValidationException (400), ConflictException (409), InternalServerException (500); input taxRegistrationEntry @required (TaxRegistrationEntry). The operation's own model documentation states the country requirements, including "Kenya You must specify the personType in the kenyaAdditionalInfo field of the additionalTaxInformation object", and shape KenyaAdditionalInfo declares personType @required. So a KE entry carrying no additionalTaxInformation.kenyaAdditionalInfo.personType fails the service's business-rule validation and is refused with HTTP 400 / error code ValidationException, creating nothing: an immediately following GetTaxRegistration still answers 404 ResourceNotFoundException. Recorded response message, as an example of that rule's text: "Your request has an AdditionalInfo object that isn't valid or is missing information. Check your request and try again."
Emulator: the call succeeds — HTTP 200 with body {"status": "Pending"} — and the registration is stored. The handler validates only registrationId/legalName/legalAddress presence, non-blankness and the registration-type member rules; nothing inspects countryCode against the per-country additionalTaxInformation requirements.
Why: the emulator is overly permissive on a documented validation boundary. A client that relies on AWS refusing an incomplete Kenyan TRN sees it accepted and gets an account state AWS would never produce (a stored primary TRN built from an invalid entry), so code paths that handle ValidationException — surfacing the missing field to a user, or retrying with personType filled in — are never exercised, and subsequent reads disagree with real AWS. It also silently occupies the account's singleton primary TRN, which AWS would have left empty.
Fix: PutTaxRegistration's validation — the per-country business-rule checks on taxRegistrationEntry (at minimum the KE requirement on additionalTaxInformation.kenyaAdditionalInfo.personType) must run before the state write and return the modeled ValidationException (400) with nothing persisted. The same rule set belongs in the shared entry validation so BatchPutTaxRegistration reuses it.
Traces: taxsettings.put-tax-registration-missing-required-field-rejected. Handler: services/service-com-amazonaws-taxsettings-taxsettings/src/ops/put_tax_registration.rs:209-302 (validation at lines 225-269, state write at 276-301). Related, filed separately: the BatchPutTaxRegistration 501 scaffold issue, which hides the same missing rule on the batch path.

## Evidence
Suite report, [mismatch] taxsettings.put-tax-registration-missing-required-field-rejected:

  File "check_taxsettings_put_tax_registration_missing_required_field_rejected.py", line 43, in run
    code, message = _error(lambda: ts.put_tax_registration(taxRegistrationEntry=entry))
  AssertionError: expected the call to be rejected, but it succeeded and returned {'ResponseMetadata': {'HTTPStatusCode': 200, ...}, 'status': 'Pending'}

Request entry (abridged): {registrationId: 'P987654321K', registrationType: 'VAT', legalName: 'Sim2Real Kenya Missing Field Ltd', legalAddress: {addressLine1: 'Kenyatta Ave 2', city: 'Nairobi', postalCode: '00100', countryCode: 'KE'}} with additionalTaxInformation deliberately omitted.

Expected (AWS): HTTP 400, Error.Code 'ValidationException' (recorded message example: "Your request has an AdditionalInfo object that isn't valid or is missing information. Check your request and try again."), and a following GetTaxRegistration answering 404 ResourceNotFoundException because nothing was created.
Actual (emulator): HTTP 200, body {"status": "Pending"}, registration stored.

Source confirmation: services/service-com-amazonaws-taxsettings-taxsettings/src/ops/put_tax_registration.rs validates only check_registration_type_members, legalName/legalAddress presence, check_not_blank and check_address; no countryCode-driven additionalTaxInformation check exists before store.registrations.insert(..).

## Steps to Reproduce
1. Boot the emulator and point a botocore taxsettings client at it; ensure the account has no primary TRN (GetTaxRegistration answers ResourceNotFoundException).
2. Call put_tax_registration(taxRegistrationEntry={'registrationId': 'P987654321K', 'registrationType': 'VAT', 'legalName': 'Sim2Real Kenya Missing Field Ltd', 'legalAddress': {'addressLine1': 'Kenyatta Ave 2', 'city': 'Nairobi', 'postalCode': '00100', 'countryCode': 'KE'}}) — note no additionalTaxInformation.
3. Observe HTTP 200 with {'status': 'Pending'} instead of HTTP 400 ValidationException.
4. Call get_tax_registration() and observe the invalid registration was persisted instead of the account still answering 404 ResourceNotFoundException.
5. Replay: taxsettings.put-tax-registration-missing-required-field-rejected.
