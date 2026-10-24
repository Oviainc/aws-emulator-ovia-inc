Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: taxsettings.batch-put-tax-registration-empty-accountids-rejected taxsettings.primary-tax-registration-full-lifecycle taxsettings.delete-supplemental-tax-registration-not-found taxsettings.concurrent-delete-supplemental-tax-registration-consistent taxsettings.supplemental-tax-registration-unsupported-country-rejected
Issue-Shape-Id: com.amazonaws.taxsettings#TaxSettings
Issue-Key: taxsettings.batch-put-tax-registration-empty-accountids-rejected|run#7|AssertionError|assert ctrl_code == "ValidationException", (
Issue-Key: taxsettings.primary-tax-registration-full-lifecycle|run#22|AssertionError|assert code2 == "ValidationException", f"expected ValidationException for invalid KE entry, got {code2!r}"
Issue-Key: taxsettings.delete-supplemental-tax-registration-not-found|run#4|AssertionError|assert code == "ValidationException", (
Issue-Key: taxsettings.concurrent-delete-supplemental-tax-registration-consistent|run#6|AssertionError|assert all(r == "ValidationException" for r in results), (
Issue-Key: taxsettings.supplemental-tax-registration-unsupported-country-rejected|run#8|AssertionError|assert code == "ValidationException", (
Issue-Commit: 048778663a26e363156c6db724999c4fb229af36

# Supplemental-TRN and batch tax-registration write paths answer 501 NotImplemented instead of their modeled 200/400 behavior

> _Note: this issue was identified against the code base at commit `048778663a26e363156c6db724999c4fb229af36`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
BatchPutTaxRegistration

## Description
Three distinct taxsettings write-path operations are still 501 `NotImplemented` scaffolds, so every behavioral check that drives them sees `NotImplemented` where AWS answers either a modeled success or a modeled 400 `ValidationException`. This issue bundles 3 root causes, all in the taxsettings service; each must be closed before submitting.

Cause 1 of 3: BatchPutTaxRegistration is not implemented.
AWS: Smithy: BatchPutTaxRegistration POST /BatchPutTaxRegistration, success 200 with output {errors (BatchPutTaxRegistrationErrors, @required), status (TaxRegistrationStatus: Deleted|Pending|Rejected|Verified)}; modeled errors ValidationException (400), ConflictException (409), InternalServerException (500). Input accountIds targets AccountIds with @length min 1 / max 550, taxRegistrationEntry is @required. The model's operation documentation states the country business rules the entry must satisfy, e.g. "Kenya You must specify the personType in the kenyaAdditionalInfo field of the additionalTaxInformation object" (KenyaAdditionalInfo.personType is @required). Recorded behavior: a BatchPut for the caller's own account with a KE entry that omits kenyaAdditionalInfo.personType is refused 400 ValidationException (recorded message, as an example: "Your request has an AdditionalInfo object that isn't valid or is missing information. Check your request and try again.") and stores nothing; a request sent with accountIds=[] (client-side validation disabled) is refused 400 ValidationException naming the member and its constraint (recorded example: "1 validation error detected: Value at 'accountIds' failed to satisfy constraint: Member must have length greater than or equal to 1"); and a valid entry for the caller's account returns 200 with errors=[] and status=Pending.
Emulator: every BatchPutTaxRegistration call returns HTTP 501 with error code `NotImplemented` ("operation BatchPutTaxRegistration is scaffolded but not implemented in this emulator").
Why: a client cannot create or update tax registrations in batch at all, and cannot distinguish a structural rejection (empty accountIds) from a country business-rule rejection — both surface as a non-modeled 501 code that no AWS SDK error handler or retry policy expects. The full primary-TRN lifecycle (Put -> Get -> Delete -> BatchPut -> BatchDelete) cannot complete.
Fix: implement the operation's handler and its validation (the per-account write loop, the accountIds cardinality refusal, and the per-country entry business rules), returning the modeled ValidationException instead of the scaffold error channel.
Traces: taxsettings.batch-put-tax-registration-empty-accountids-rejected (got 'NotImplemented' where ValidationException was expected on both the control and the empty-list call); taxsettings.primary-tax-registration-full-lifecycle (reaches the BatchPut stage after Put/Get/Delete succeed and gets 'NotImplemented' for the invalid KE entry). Handler: services/service-com-amazonaws-taxsettings-taxsettings/src/ops/batch_put_tax_registration.rs:197 returns Scaffolded::not_implemented.

Cause 2 of 3: DeleteSupplementalTaxRegistration is not implemented.
AWS: Smithy: DeleteSupplementalTaxRegistration POST /DeleteSupplementalTaxRegistration, success 200 with an empty output; input authorityId @required, GenericString @length 1..200; modeled errors ValidationException (400), ResourceNotFoundException (404), ConflictException (409), InternalServerException (500). Recorded behavior: on an account holding no supplemental TRN, every authorityId that does not name a live registration — junk strings and a syntactically valid UUID alike — is refused 400 ValidationException (recorded message, as an example: "Provided authorityId is not valid."), and concurrent identical requests all observe that same outcome.
Emulator: every call returns HTTP 501 `NotImplemented`, including all 8 concurrent attempts.
Why: callers cannot delete a supplemental tax registration, and an unknown authorityId produces an unmodeled 501 instead of the 400 client-error class, so client code branching on ValidationException/ResourceNotFoundException never takes either branch.
Fix: implement the operation's handler and its lookup/validation of authorityId against stored supplemental registrations, returning the modeled errors.
Traces: taxsettings.delete-supplemental-tax-registration-not-found; taxsettings.concurrent-delete-supplemental-tax-registration-consistent. Handler: services/service-com-amazonaws-taxsettings-taxsettings/src/ops/delete_supplemental_tax_registration.rs:156.

Cause 3 of 3: PutSupplementalTaxRegistration is not implemented.
AWS: Smithy: PutSupplementalTaxRegistration POST /PutSupplementalTaxRegistration, success 200 with output {authorityId (@required), status (@required, TaxRegistrationStatus)}; input taxRegistrationEntry @required (SupplementalTaxRegistrationEntry); modeled errors ValidationException (400), ConflictException (409), InternalServerException (500). Recorded behavior: the operation accepts only a bounded set of jurisdictions — entries for unsupported countries (FR and GB were both recorded) are refused 400 ValidationException (recorded message, as an example: "Tax registration for country is not supported.") and create nothing, while a supported country (CH) clears country validation and fails only on account state (recorded ConflictException) rather than ValidationException.
Emulator: every call returns HTTP 501 `NotImplemented`, for supported and unsupported countries alike.
Why: supplemental TRNs can never be created, and the country-support boundary is invisible: a client gets the same unmodeled 501 for a request AWS accepts and for one AWS refuses with 400, so it cannot tell a rejected jurisdiction from a transient service problem.
Fix: implement the operation's handler, its country-support validation and the state write that makes the new registration readable by ListSupplementalTaxRegistrations.
Traces: taxsettings.supplemental-tax-registration-unsupported-country-rejected. Handler: services/service-com-amazonaws-taxsettings-taxsettings/src/ops/put_supplemental_tax_registration.rs (handle returns Scaffolded::not_implemented).

## Evidence
Suite report (service taxsettings, 5/16 passed):

[mismatch] taxsettings.batch-put-tax-registration-empty-accountids-rejected
  AssertionError: expected the KE business-rule violation to reach the service as ValidationException, got 'NotImplemented'
  Expected: 400 ValidationException for a KE entry missing additionalTaxInformation.kenyaAdditionalInfo.personType; 400 ValidationException naming accountIds for accountIds=[].
  Actual: HTTP 501, Error.Code 'NotImplemented'.

[mismatch] taxsettings.primary-tax-registration-full-lifecycle
  AssertionError: expected ValidationException for invalid KE entry, got 'NotImplemented'
  (PutTaxRegistration/GetTaxRegistration/DeleteTaxRegistration stages of the same check passed, so the divergence is isolated to BatchPutTaxRegistration.)

[mismatch] taxsettings.delete-supplemental-tax-registration-not-found
  AssertionError: expected ValidationException for a malformed authorityId, got 'NotImplemented'
  Expected: 400 ValidationException for a bogus authorityId. Actual: 501 'NotImplemented'.

[mismatch] taxsettings.concurrent-delete-supplemental-tax-registration-consistent
  AssertionError: expected every concurrent call to see ValidationException consistently, got ['NotImplemented', 'NotImplemented', 'NotImplemented', 'NotImplemented', 'NotImplemented', 'NotImplemented', 'NotImplemented', 'NotImplemented']

[mismatch] taxsettings.supplemental-tax-registration-unsupported-country-rejected
  AssertionError: expected ValidationException for an unsupported supplemental-TRN country, got 'NotImplemented'
  Expected: 400 ValidationException for FR and GB entries; non-ValidationException (recorded: ConflictException) for the supported CH entry. Actual: 501 'NotImplemented' for all of them.

Source confirmation: all three handler files still carry the '//! SCAFFOLDED, NOT IMPLEMENTED' marker and return crate::scaffolded::Scaffolded::not_implemented(..).

## Steps to Reproduce
1. Boot the emulator and point a botocore taxsettings client at it.
2. BatchPutTaxRegistration: call batch_put_tax_registration(accountIds=[<caller account>], taxRegistrationEntry={registrationId:'P123456789K', registrationType:'VAT', legalName:'...', legalAddress:{addressLine1:'Kenyatta Ave 1', city:'Nairobi', postalCode:'00100', countryCode:'KE'}}) with no additionalTaxInformation. Expect 400 ValidationException; observe 501 NotImplemented. Repeat with a client built with Config(parameter_validation=False) and accountIds=[]: expect 400 ValidationException naming accountIds' minimum length; observe 501 NotImplemented.
3. DeleteSupplementalTaxRegistration: call delete_supplemental_tax_registration(authorityId='no-such-authority-<rand>'). Expect 400 ValidationException; observe 501 NotImplemented. Issue 8 of these concurrently and observe all 8 return NotImplemented.
4. PutSupplementalTaxRegistration: call put_supplemental_tax_registration(taxRegistrationEntry={registrationId:'FR12345678901', registrationType:'VAT', legalName:'...', address:{addressLine1:'1 Rue de Test', city:'Testville', postalCode:'75000', countryCode:'FR'}}). Expect 400 ValidationException; observe 501 NotImplemented. Repeat with countryCode 'CH' and observe 501 instead of the operation clearing country validation.
5. Replay: taxsettings.batch-put-tax-registration-empty-accountids-rejected, taxsettings.primary-tax-registration-full-lifecycle, taxsettings.delete-supplemental-tax-registration-not-found, taxsettings.concurrent-delete-supplemental-tax-registration-consistent, taxsettings.supplemental-tax-registration-unsupported-country-rejected.
