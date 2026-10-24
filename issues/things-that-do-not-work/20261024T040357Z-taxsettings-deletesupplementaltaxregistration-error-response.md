Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 733b41f401fdbdc43a4f3ac4e403542c858679fd

# DeleteSupplementalTaxRegistration answers 400 ValidationException for an authorityId that names no record instead of 404 ResourceNotFoundException

_Filed: 2026-10-24T04:03:57Z_
_Operation: DeleteSupplementalTaxRegistration | Class: error-response_

> _Note: this issue was identified against the code base at commit `733b41f401fdbdc43a4f3ac4e403542c858679fd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Expected: an `authorityId` that satisfies the member's modeled constraints (`@length 1..200`, pattern `^[\s\S]*$`) but names no stored supplemental registration is a missing RESOURCE, which this operation models as `ResourceNotFoundException` (HTTP 404) — documented in the model as "The exception thrown when the input doesn't have a resource associated to it." That is precisely this case, and it is the only reason this operation would ever raise that error (nothing else in the input can name a resource).

Actual: the handler returns the modeled `ValidationException` (HTTP 400, `errorCode: FieldValidationFailed`, message "Provided authorityId is not valid.") for every unknown authorityId, including deleting a registration twice. `ValidationException` is documented as "The exception when the input doesn't pass validation for at least one of the input parameters" — the parameter here passes every modeled constraint, so the wrong error shape, status and errorCode go out and the operation's modeled 404 is unreachable. Note the sibling resource operations in this crate do answer 404 for a missing record (`DeleteTaxRegistration`, `GetTaxRegistration` via `crate::state::no_tax_registration()`), so the supplemental delete is also internally inconsistent.

## Evidence

Spec: `smithy_query operation=DeleteSupplementalTaxRegistration` lists errors `ConflictException(409)`, `InternalServerException(500)`, `ResourceNotFoundException(404)`, `ValidationException(400)`; `ResourceNotFoundException` doc = "The exception thrown when the input doesn't have a resource associated to it."

Handler: `src/ops/delete_supplemental_tax_registration.rs` `handle()` — `if store.supplemental_registrations.remove(&input.authority_id).is_none() { return Err(...ValidationException(crate::state::invalid_authority_id())) }`; `invalid_authority_id()` in `src/state.rs` builds `ValidationException { errorCode: FieldValidationFailed, fieldList: [authorityId], message: "Provided authorityId is not valid." }`.

Tests: `tests/all/supplemental_tax_registrations.rs` (unknown-authority rows around lines 870-949 and `concurrent_deletes_of_one_authority_agree`) currently assert the 400/ValidationException outcome, i.e. the tests pin the emulator's own choice rather than the modeled 404 — they would need updating with the fix.

## Steps to Reproduce

1. POST /PutSupplementalTaxRegistration with a valid entry for a supported country; note the returned authorityId.
2. POST /DeleteSupplementalTaxRegistration with {"authorityId":"no-such-authority-7f3"} (a value inside the modeled 1..200 length and matching the pattern).
3. Observe HTTP 400 ValidationException / errorCode FieldValidationFailed. Expected HTTP 404 ResourceNotFoundException for an authorityId with no associated resource.
4. Delete the live authorityId (200), then delete it again — observe the same 400 instead of a 404.
