Issue-Source: trace-sweep-divergence
Issue-Trace: taxsettings-put-tax-registration-validation-error
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: taxsettings-put-tax-registration-validation-error taxsettings-put-tax-exemption-invalid-certificate-filename-rejected taxsettings-tax-inheritance-toggle taxsettings-tax-registration-lifecycle
Issue-Shape-Id: com.amazonaws.taxsettings#TaxSettings
Issue-Key: tax|PutTaxRegistration|4xx->2xx|$.errorCode
Issue-Key: tax|PutTaxRegistration|4xx->2xx|$.fieldList
Issue-Key: tax|PutTaxRegistration|4xx->2xx|$.message
Issue-Key: tax|PutTaxRegistration|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: tax|PutTaxRegistration|4xx->2xx|status
Issue-Key: tax|PutTaxExemption|4xx->4xx|$.message
Issue-Key: tax|GetTaxInheritance|2xx->2xx|$
Issue-Key: tax|PutTaxInheritance|4xx->2xx|$
Issue-Key: tax|PutTaxInheritance|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: tax|PutTaxInheritance|4xx->2xx|status
Issue-Key: tax|GetTaxRegistration|2xx->2xx|$.taxRegistration.status
Issue-Key: tax|GetTaxRegistration|4xx->4xx|$.errorCode
Issue-Key: tax|GetTaxRegistration|4xx->4xx|$.message
Issue-Commit: dcb14db06d6eb8eb19e965c6ac371e7edd3d0be4

# tax (taxsettings): 6 fidelity divergences — validation not enforced, inheritance/registration state not returned, error bodies malformed

> _Note: this issue was identified against the code base at commit `dcb14db06d6eb8eb19e965c6ac371e7edd3d0be4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutTaxRegistration

## Description
Replay of the simparity-wire-aws-v1 corpus against the taxsettings (wire service token `tax`) emulator produced 6 distinct divergence clusters over 16 mismatched steps (2/18 steps passed). They share one root fault family: the write operations accept everything and persist nothing observable, so request-level validation is never enforced, read operations do not reflect stored state, and the two error shapes that ARE produced are serialized with the wrong member set. This bundle carries SIX causes; all six must be closed before submitting.

Cause 1 of 6 — PutTaxRegistration accepts an invalid India PAN registration instead of rejecting it (fixture `taxsettings-put-tax-registration-validation-error`, step `task#1`).
AWS: PutTaxRegistration models `ValidationException` (errorKind client, httpError 400) and the operation's own success code is 200 (Smithy: PutTaxRegistration http {method POST, uri /PutTaxRegistration, code 200}; errors ConflictException/InternalServerException/ValidationException). `ValidationException` requires `message` and `errorCode` (target `ValidationExceptionErrorCode`, an enum whose values include `FieldValidationFailed`) and may carry `fieldList` of `ValidationExceptionField{name}`. `TaxRegistrationEntry.legalAddress` is optional and is country/registration-type dependent. The recording shows AWS answering HTTP 400 with header `x-amzn-errortype: ValidationException` and body errorCode `FieldValidationFailed`, fieldList `[{"name": "LEGAL_ADDRESS"}]`, message `Legal address should not be provided for India PAN Tax Registration.` (message quoted as an example of the rule: the field that violated the constraint is named in `fieldList`).
Emulator: returns HTTP 200 with no errorCode, no fieldList, no message and no `x-amzn-errortype` header — the entry is accepted.
Why: a client that relies on AWS rejecting an India PAN entry carrying a legal address believes the registration was stored as submitted. Error-handling paths (retry/repair of the offending field, which AWS pinpoints via `fieldList[].name`) are never exercised, and SDK callers get a success result for a request AWS refuses.
Fix: the request validation for PutTaxRegistration in the operation's handler — reject registration-type/field combinations AWS rejects and serialize `ValidationException` with its required `errorCode` plus `fieldList`.
Traces: taxsettings-put-tax-registration-validation-error#task#1.

Cause 2 of 6 — PutTaxInheritance accepts a non-management-account call instead of rejecting it (fixture `taxsettings-tax-inheritance-toggle`, step `task#2`; cluster also covers `task#3`).
AWS: PutTaxInheritance models `ValidationException` (httpError 400) and an empty output (Smithy: PutTaxInheritance input {heritageStatus: HeritageStatus}, output {}, errors ConflictException/InternalServerException/ResourceNotFoundException/ValidationException). The recording shows AWS answering HTTP 400 with `x-amzn-errortype: ValidationException` and body errorCode `FieldValidationFailed`, fieldList null, message `Only AWS orgs management account is allowed to set tax inheritance.` (message as an example; the requirement is the 400 and the `ValidationException`/`FieldValidationFailed` pair).
Emulator: returns HTTP 200 with an empty body and no `x-amzn-errortype` header, for both affected steps.
Why: tax inheritance appears to toggle successfully from an account that AWS forbids from toggling it, so a caller cannot distinguish an authorized management-account call from a rejected member-account call.
Fix: the authorization/validation branch of the PutTaxInheritance handler, which must emit `ValidationException` with errorCode `FieldValidationFailed` and HTTP 400.
Traces: taxsettings-tax-inheritance-toggle#task#2, taxsettings-tax-inheritance-toggle#task#3.

Cause 3 of 6 — GetTaxInheritance returns an empty body instead of the stored heritage status (fixture `taxsettings-tax-inheritance-toggle`, step `task#1`).
AWS: GetTaxInheritance is readonly with output member `heritageStatus` targeting the `HeritageStatus` enum, values `OptIn` | `OptOut` (Smithy: GetTaxInheritance output {heritageStatus: HeritageStatus}; http 200 POST /GetTaxInheritance). The recording shows HTTP 200 with body `{"heritageStatus": "OptOut"}`.
Emulator: returns HTTP 200 with `{}` — the modeled output member is absent entirely.
Why: a client reading the account's tax-inheritance setting gets `None`/absent rather than an enum value, so it cannot tell `OptOut` from `OptIn` and any code branching on heritage status takes its default path. Combined with Cause 2 this means the inheritance setting is neither settable nor readable.
Fix: the GetTaxInheritance handler and the inheritance field of the service state — persist the heritage status and serialize it into the response.
Traces: taxsettings-tax-inheritance-toggle#task#1.

Cause 4 of 6 — GetTaxRegistration reports a stale `Verified` status instead of the registration's actual lifecycle status (fixture `taxsettings-tax-registration-lifecycle`, step `task#2`).
AWS: `TaxRegistration.status` is required and targets the `TaxRegistrationStatus` enum, documented as `Verified`, `Pending`, `Deleted` or `Rejected` (Smithy: TaxRegistration.status -> TaxRegistrationStatus, values [Verified, Pending, Deleted, Rejected]); GetTaxRegistration is readonly and returns `taxRegistration`. The recording shows the registration reaching `Rejected`, and the fixture polls for it with `Verified` in its fail-on set.
Emulator: returns `$.taxRegistration.status = "Verified"` on the first poll attempt and never leaves it, so the poll aborts.
Why: the status member is the only signal a client has that a submitted TRN was refused. Pinning it to `Verified` makes every registration look accepted; a client waiting for a terminal `Rejected`/`Deleted` transition waits forever or proceeds on a registration AWS did not verify.
Fix: the tax-registration state machine in the service state (status transitions driven by PutTaxRegistration/DeleteTaxRegistration) rather than a constant in the GetTaxRegistration handler.
Traces: taxsettings-tax-registration-lifecycle#task#2.

Cause 5 of 6 — ResourceNotFoundException body carries an `errorCode` member AWS does not send (fixture `taxsettings-tax-registration-lifecycle`, step `teardown#4`).
AWS: GetTaxRegistration models `ResourceNotFoundException` (errorKind client, httpError 404) with members `message` and `errorCode` (Smithy: ResourceNotFoundException{message: ErrorMessage, errorCode: ErrorCode}). Recorded behavior only: on this not-found path AWS returns HTTP 404 with `errorCode` ABSENT from the body and message `No tax registration found` (the message is an example; the requirement is the 404 and the member set actually serialized).
Emulator: returns HTTP 404 (status matches) but adds `errorCode: "ResourceNotFoundException"` to the body, and renders the message as `No tax registration found for account 000000000000.`
Why: a client that reads `errorCode` to classify taxsettings failures sees a populated discriminator on AWS's 404 where AWS sends none, so code keyed on its presence/absence (or on `errorCode` being a service-defined code rather than the exception's own name) branches differently against the emulator than against AWS. Note the message difference is reported here only as an example — the defect is the extra serialized member, not the wording.
Fix: the error serializer for taxsettings `ResourceNotFoundException` — emit only the members AWS populates on this path instead of defaulting `errorCode` to the exception's shape name.
Traces: taxsettings-tax-registration-lifecycle#teardown#4.

Cause 6 of 6 — PutTaxExemption's constraint-violation message echoes the regex anchors and interpolates the offending value (fixture `taxsettings-put-tax-exemption-invalid-certificate-filename-rejected`, step `task#1`).
AWS: `ExemptionCertificate.documentName` is required with `@pattern ^([A-Za-z0-9_.-]+)\.([pP][dD][fF]|[jJ][pP][gG]|[pP][nN][gG])$` and `@length max 128`; PutTaxExemption models `ValidationException` at httpError 400 (Smithy: PutTaxExemption.exemptionCertificate -> ExemptionCertificate; errors AccessDeniedException/AttachmentUploadException/CaseCreationLimitExceededException/InternalServerException/ResourceNotFoundException/ValidationException). This message is built from the request, so its shape is a requirement: AWS's standard constraint message names the member path and the pattern it must satisfy, WITHOUT the leading `^`/trailing `$` anchors and WITHOUT echoing the submitted value — recorded as `1 validation error detected: Value at 'exemptionCertificate.documentName' failed to satisfy constraint: Member must satisfy regular expression pattern: ([A-Za-z0-9_.-]+)\.([pP][dD][fF]|[jJ][pP][gG]|[pP][nN][gG])`.
Emulator: status 400 is correct, but the message is `1 validation error detected: Value 'sim2real-cert.txt' at 'exemptionCertificate.documentName' failed to satisfy constraint: Member must satisfy regular expression pattern: ^([A-Za-z0-9_.-]+)\.([pP][dD][fF]|[jJ][pP][gG]|[pP][nN][gG])$` — it inserts `Value '<submitted filename>'` and reproduces the anchored pattern.
Why: callers and test suites that parse the standard `1 validation error detected: Value at '<path>' ... pattern: <pattern>` form to recover the offending member path and the constraint see a differently-shaped string, and the echoed value leaks the submitted document name into a message AWS keeps value-free.
Fix: the shared constraint-violation message builder used by the PutTaxExemption input validation — strip the `^`/`$` anchors when echoing a `@pattern` and omit the submitted value for this member.
Traces: taxsettings-put-tax-exemption-invalid-certificate-filename-rejected#task#1.

## Evidence
Replay summary: trace_set_id simparity-wire-aws-v1, emulator_up true, bootstrap_signal false, state_reset_failures 0, steps_total 18, steps_passed 2, steps_mismatched 16, steps_errored 0, steps_blocked 0, score_pct 11.1, divergence_count 6. No cluster carried a stub_path or an owned_by.

Cause 1 — PutTaxRegistration, taxsettings-put-tax-registration-validation-error#task#1, verdict mismatch:
  expected status 400, actual 200
  $.errorCode: missing (expected 'FieldValidationFailed', got None)
  $.fieldList: missing (expected [{'name': 'LEGAL_ADDRESS'}], got None)
  $.message: missing (expected 'Legal address should not be provided for India PAN Tax Registration.', got None)
  $headers.x-amzn-errortype: missing (expected 'ValidationException', got None)

Cause 2 — PutTaxInheritance, taxsettings-tax-inheritance-toggle#task#2 (also task#3), verdict mismatch:
  expected status 400, actual 200
  $: empty_body (expected {'errorCode': 'FieldValidationFailed', 'fieldList': None, 'message': 'Only AWS orgs management account is allowed to set tax inheritance.'}, got {})
  $headers.x-amzn-errortype: missing (expected 'ValidationException', got None)

Cause 3 — GetTaxInheritance, taxsettings-tax-inheritance-toggle#task#1, verdict mismatch:
  status 200 == 200
  $: empty_body (expected {'heritageStatus': 'OptOut'}, got {})

Cause 4 — GetTaxRegistration, taxsettings-tax-registration-lifecycle#task#2, verdict mismatch:
  status 200 == 200
  $.taxRegistration.status: value_mismatch (expected 'Rejected', got 'Verified')
  $.taxRegistration.status: poll: 'Verified' is in fail_on after 1 attempt(s)

Cause 5 — GetTaxRegistration, taxsettings-tax-registration-lifecycle#teardown#4, verdict mismatch:
  status 404 == 404
  $.errorCode: type_mismatch (expected None, got 'ResourceNotFoundException')
  $.message: value_mismatch (expected 'No tax registration found', got 'No tax registration found for account 000000000000.') [example only]

Cause 6 — PutTaxExemption, taxsettings-put-tax-exemption-invalid-certificate-filename-rejected#task#1, verdict mismatch:
  status 400 == 400
  $.message: value_mismatch
    expected: "1 validation error detected: Value at 'exemptionCertificate.documentName' failed to satisfy constraint: Member must satisfy regular expression pattern: ([A-Za-z0-9_.-]+)\\.([pP][dD][fF]|[jJ][pP][gG]|[pP][nN][gG])"
    actual:   "1 validation error detected: Value 'sim2real-cert.txt' at 'exemptionCertificate.documentName' failed to satisfy constraint: Member must satisfy regular expression pattern: ^([A-Za-z0-9_.-]+)\\.([pP][dD][fF]|[jJ][pP][gG]|[pP][nN][gG])$"

Model evidence (services/service-com-amazonaws-taxsettings-taxsettings/smithy-model/com-amazonaws-taxsettings-taxsettings.json):
  ValidationException: client error, httpError 400, required members message + errorCode (enum ValidationExceptionErrorCode = MalformedToken, ExpiredToken, InvalidToken, FieldValidationFailed, MissingInput, NonIndiaCustomerCanNotSetPAN, GSTExistenceBlockSetPAN), optional fieldList of ValidationExceptionField{name}
  ResourceNotFoundException: client error, httpError 404, members message + errorCode (ErrorCode, length 0..50)
  HeritageStatus enum = OptIn | OptOut; GetTaxInheritance output {heritageStatus}
  TaxRegistrationStatus enum = Verified | Pending | Deleted | Rejected; TaxRegistration.status required
  ExemptionCertificate.documentName required, @pattern ^([A-Za-z0-9_.-]+)\\.([pP][dD][fF]|[jJ][pP][gG]|[pP][nN][gG])$, @length 0..128
  TaxRegistrationEntry.legalAddress optional (country/registration-type dependent)

Affected emulator component: services/service-com-amazonaws-taxsettings-taxsettings/src/ops/ (operation handlers + input validation), src/state.rs (inheritance setting, tax-registration status transitions), and the shared error/constraint-message serializer.

## Steps to Reproduce
1. git clone the mockforge-aws-emulator repo (branch mainline) and build/boot the emulator from that checkout; confirm /__admin/health answers 200 and the data listener accepts.
2. Replay the simparity-wire-aws-v1 wire corpus (replay_wire_corpus with no arguments). Only 2 of 18 taxsettings steps pass; 16 mismatch across 6 clusters.
3. Per cause, re-replay the named fixture/step:
   - Cause 1: fixture taxsettings-put-tax-registration-validation-error, step task#1 — POST /PutTaxRegistration with an India PAN taxRegistrationEntry that includes legalAddress. Expect HTTP 400 + x-amzn-errortype: ValidationException + errorCode FieldValidationFailed + fieldList [{name: LEGAL_ADDRESS}]; observe HTTP 200 with an empty body.
   - Cause 2: fixture taxsettings-tax-inheritance-toggle, steps task#2 and task#3 — POST /PutTaxInheritance from a non-management account. Expect HTTP 400 + x-amzn-errortype: ValidationException + errorCode FieldValidationFailed; observe HTTP 200 with an empty body.
   - Cause 3: fixture taxsettings-tax-inheritance-toggle, step task#1 — POST /GetTaxInheritance. Expect HTTP 200 with heritageStatus set to an OptIn/OptOut enum value (OptOut in the recording); observe HTTP 200 with {}.
   - Cause 4: fixture taxsettings-tax-registration-lifecycle, step task#2 — POST /GetTaxRegistration and poll $.taxRegistration.status. Expect the status to reach the terminal Rejected value; observe it pinned at Verified on attempt 1, which is in the fixture's fail_on set.
   - Cause 5: fixture taxsettings-tax-registration-lifecycle, step teardown#4 — POST /GetTaxRegistration for a deleted/absent registration. Expect HTTP 404 whose ResourceNotFoundException body has NO errorCode member; observe HTTP 404 with errorCode: "ResourceNotFoundException" added.
   - Cause 6: fixture taxsettings-put-tax-exemption-invalid-certificate-filename-rejected, step task#1 — POST /PutTaxExemption with exemptionCertificate.documentName not matching the modeled pattern (e.g. a .txt name). Expect HTTP 400 whose message is the standard "1 validation error detected: Value at '<member path>' failed to satisfy constraint: Member must satisfy regular expression pattern: <pattern without ^/$ anchors>"; observe the submitted filename interpolated as "Value '<name>'" and the pattern echoed with its ^ and $ anchors.
4. All six causes must be closed and each listed fixture#step re-replayed clean before submitting.
