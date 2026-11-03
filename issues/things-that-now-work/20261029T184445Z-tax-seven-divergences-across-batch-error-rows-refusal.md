Issue-Source: trace-sweep-divergence
Issue-Trace: taxsettings-batch-tax-registration-workflow
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: taxsettings-batch-tax-registration-workflow taxsettings-supplemental-tax-registration-conflict-error taxsettings-tax-exemption-workflow
Issue-Shape-Id: com.amazonaws.taxsettings#TaxSettings
Issue-Key: tax|BatchPutTaxRegistration|2xx->2xx|$.errors[*]
Issue-Key: tax|BatchPutTaxRegistration|2xx->2xx|$.status
Issue-Key: tax|BatchDeleteTaxRegistration|2xx->2xx|$.errors[*]
Issue-Key: tax|PutSupplementalTaxRegistration|4xx->2xx|$.errorCode
Issue-Key: tax|PutSupplementalTaxRegistration|4xx->2xx|$.message
Issue-Key: tax|PutSupplementalTaxRegistration|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: tax|PutSupplementalTaxRegistration|4xx->2xx|status
Issue-Key: tax|GetTaxExemptionTypes|2xx->2xx|$.taxExemptionTypes[*]
Issue-Key: tax|PutTaxExemption|4xx->2xx|$.errorCode
Issue-Key: tax|PutTaxExemption|4xx->2xx|$.fieldList
Issue-Key: tax|PutTaxExemption|4xx->2xx|$.message
Issue-Key: tax|PutTaxExemption|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: tax|PutTaxExemption|4xx->2xx|status
Issue-Key: tax|ListTaxExemptions|2xx->2xx|$.taxExemptionDetailsMap.000000000000.heritageObtainedDetails
Issue-Key: tax|ListTaxExemptions|2xx->2xx|$.taxExemptionDetailsMap.000000000000.taxExemptions[*]
Issue-Key: tax|BatchGetTaxExemptions|2xx->2xx|$.taxExemptionDetailsMap.000000000000.heritageObtainedDetails
Issue-Key: tax|BatchGetTaxExemptions|2xx->2xx|$.taxExemptionDetailsMap.000000000000.taxExemptions[*]
Issue-Commit: f2009969ccef167db9c30e6e9450d638add1f0e3

# tax: seven divergences across batch error rows, refusal gates and the tax-exemption surface

> _Note: this issue was identified against the code base at commit `f2009969ccef167db9c30e6e9450d638add1f0e3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
BatchPutTaxRegistration

## Description
Seven net-new wire divergences on the bootstrapped `tax` (TaxSettings) emulator, sharing one fault: refusals are not produced where the service produces them, so the per-account error channel and the exemption surface report success for input the service rejects. ALL SEVEN CAUSES MUST BE CLOSED before this issue is submitted. Model facts below come from smithy_query against services/service-com-amazonaws-taxsettings-taxsettings/smithy-model/com-amazonaws-taxsettings-taxsettings.json.

=== Cause 1 of 7: BatchPutTaxRegistration reports no per-account error row for an invalid registration number, and emits a top-level status for a batch that wrote nothing (fixture taxsettings-batch-tax-registration-workflow, step task#1) ===
AWS: Smithy: BatchPutTaxRegistration output = {errors: BatchPutTaxRegistrationErrors @required, status: TaxRegistrationStatus (OPTIONAL; enum Verified|Pending|Deleted|Rejected)}. BatchPutTaxRegistrationError = {accountId @required (AccountId, @length 12..12, @pattern ^\d+$), message @required (ErrorMessage, @length 0..1024, sensitive), code (ErrorCode, free-form, OPTIONAL)}. The recorded response is HTTP 200 carrying one error row per refused account: message is the fixed text for this error class, `TCS_INVALID_TAX_REGISTRATION_NUMBER`; accountId is a value taken from the request's `accountIds`; `code` is absent. The recorded body carries NO top-level `status` — `status` reports the status of a registration that was actually written, so a batch in which every named account was refused omits it.
Emulator: 200 with `errors` EMPTY (`$.errors[0]` missing) and `status: "Pending"` — the malformed registration number was validated as acceptable and stored.
Why: a client batching registrations reads an empty `errors` list plus a Pending status and concludes every account registered. The per-account error channel that is the entire point of a Batch* operation never reports the rejection, and the client's subsequent reads are against state AWS does not hold (this is what causes 2, 6 and 7 then observe).
Fix: BatchPutTaxRegistration's validation and handler — the registration-number format check must emit a BatchPutTaxRegistrationError row for each refused account, and the handler must omit `status` when no account was written. Leave `code` off the row, consistent with the crate rule cited by open issue 20261029T063530Z-taxsettings-batchputtaxregistration-error-response (that issue is about the VALUE placed in a conflict row's `code`, not about the row being absent, so it does not cover this).
Traces: taxsettings-batch-tax-registration-workflow#task#1.

=== Cause 2 of 7: BatchDeleteTaxRegistration reports no per-account not-found row for an account that holds no registration (fixture taxsettings-batch-tax-registration-workflow, step task#2) ===
AWS: Smithy: BatchDeleteTaxRegistration output = {errors: BatchDeleteTaxRegistrationErrors @required}. BatchDeleteTaxRegistrationError = {accountId @required, message @required, code OPTIONAL}. The recorded response is HTTP 200 with one row for the account that had nothing to delete: message is the fixed text `TaxRegistrationNotFound`, accountId is taken from the request's `accountIds`, `code` absent. Deleting a registration that does not exist is a per-account row, not a silent success.
Emulator: 200 with `errors` EMPTY (`$.errors[0]` missing). Two things combine here: the emulator holds a registration AWS refused in cause 1, so its delete finds a record where AWS finds none; and independently its batch-delete path has no branch that emits a `TaxRegistrationNotFound` row for an account with no stored registration.
Why: a client deleting registrations across a set of accounts cannot tell which accounts actually had one. Every account reads as deleted, so reconciliation loops never retry or report the gap.
Fix: BatchDeleteTaxRegistration's handler — the per-account deletability check must emit a not-found error row for an account with no stored registration instead of skipping it.
Traces: taxsettings-batch-tax-registration-workflow#task#2.

=== Cause 3 of 7: PutSupplementalTaxRegistration accepts a conflicting write with 200 instead of refusing it 409 ConflictException/GeneralConflict (fixture taxsettings-supplemental-tax-registration-conflict-error, step task#2) ===
AWS: Smithy: PutSupplementalTaxRegistration declares error ConflictException with httpError 409; ConflictException = {message @required (ErrorMessage), errorCode @required (ErrorCode)}. The recorded refusal is HTTP 409, header x-amzn-errortype: `ConflictException`, body {errorCode: `GeneralConflict`, message: `Operation cannot be performed due to conflict in current state.`}. Both the errorCode and the message are entirely fixed text for this error class — the message interpolates nothing from the request or from state.
Emulator: HTTP 200 with a success body; no errorCode, no message, no x-amzn-errortype header. The conflicting supplemental registration is written.
Why: a client whose write conflicts with the current state of the supplemental-TRN record is told it succeeded. It never retries or surfaces the conflict, and the emulator's stored supplemental registrations diverge from what the client believes it wrote. Related but distinct from open issue 20261029T041155Z-taxsettings-putsupplementaltaxregistration-wrong-behavior (which is about a re-spelled number being APPENDED under a new authorityId); this cause is the missing 409 refusal and its error envelope.
Fix: PutSupplementalTaxRegistration's handler — add the conflict gate before the write, and its error serializer must render ConflictException as 409 with errorCode `GeneralConflict`, the fixed message above, and the x-amzn-errortype header.
Traces: taxsettings-supplemental-tax-registration-conflict-error#task#2.

=== Cause 4 of 7: GetTaxExemptionTypes still publishes an invented catalog with no per-state jurisdictions (fixture taxsettings-tax-exemption-workflow, step task#1) ===
AWS: Smithy: GetTaxExemptionTypes output = {taxExemptionTypes: list<TaxExemptionType>}; TaxExemptionType = {displayName (DisplayName, @length 0..50), description (@length 1..200), applicableJurisdictions: list<Authority>}; Authority = {country @required (@length 2..2, @pattern ^[a-zA-Z]+$), state (@length 1..50)}. The recorded catalog is the service's published one: each entry's displayName and description are the service's own short catalog tokens (the first entry is displayName `501C`, description `501C`), and applicableJurisdictions enumerate one Authority PER STATE the type applies in ({country: "US", state: "AZ"}, {country: "US", state: "CO"}, ... ), 23 entries in total.
Emulator: a different, emulator-authored catalog. The first entry is displayName `501(c)(3)` with description `Charitable organization exempt under section 501(c)(3) of the US Internal Revenue Code` and applicableJurisdictions collapsed to a single country-only Authority `[{"country": "US"}]`; all 23 indexes diverge.
Why: this catalog IS the vocabulary that PutTaxExemption's `exemptionType` is validated against and that ListTaxExemptions/BatchGetTaxExemptions echo back. A client that reads a type name from AWS cannot submit it here, and because the per-state jurisdiction lists are absent there is nothing for the authority check in cause 5 to validate against. An earlier issue on this catalog (20261024T200859Z-taxsettings-gettaxexemptiontypes-wrong-behavior) is marked FIXED, but the catalog is still invented — only the invented strings changed — and a resolved issue never covers a live failure.
Fix: the seeded exemption-type catalog backing GetTaxExemptionTypes (its state/data table), which must carry the service's own displayName/description tokens and the per-state applicableJurisdictions list for each type.
Traces: taxsettings-tax-exemption-workflow#task#1.

=== Cause 5 of 7: PutTaxExemption accepts an authority the exemption type does not cover with 200 instead of 400 ValidationException/FieldValidationFailed (fixture taxsettings-tax-exemption-workflow, step task#2) ===
AWS: Smithy: PutTaxExemption input = {accountIds @required, authority @required (Authority{country,state}), exemptionType @required, exemptionCertificate @required}; it declares ValidationException with httpError 400. ValidationException = {message @required, errorCode @required (ValidationExceptionErrorCode enum: MalformedToken|ExpiredToken|InvalidToken|FieldValidationFailed|MissingInput|NonIndiaCustomerCanNotSetPAN|GSTExistenceBlockSetPAN), fieldList: list<ValidationExceptionField{name @required}>}. The recorded refusal is HTTP 400 with errorCode `FieldValidationFailed`, message `Provided tax exemption authority is not supported for the tax exemption type.` and fieldList `[{"name": "AUTHORITY"}]`, plus an x-amzn-errortype header. The message is fixed text for this error class; the variable part of the body is which input member names appear in fieldList — here the offending member, AUTHORITY.
Emulator: HTTP 200; no errorCode, no message, no fieldList, no x-amzn-errortype. The exemption is stored against an authority the chosen type does not apply in.
Why: the client is told its tax exemption was submitted when the service rejected it, and the bogus exemption then shows up on every read path (causes 6 and 7). Nothing tells the client WHICH field was wrong, which is exactly what fieldList exists to convey. Distinct from open issue 20261024T200859Z-taxsettings-puttaxexemption-event-propagation (fabricated caseId / dropped exemptionCertificate on the SUCCESS path); this cause is the missing refusal.
Fix: PutTaxExemption's validation — cross-check the request's `authority` against the applicableJurisdictions of the named exemptionType in the catalog (cause 4) and refuse with ValidationException 400, errorCode FieldValidationFailed, the fixed message, and fieldList naming AUTHORITY.
Traces: taxsettings-tax-exemption-workflow#task#2.

=== Cause 6 of 7: ListTaxExemptions never populates heritageObtainedDetails and reports an exemption the service refused (fixture taxsettings-tax-exemption-workflow, step task#3) ===
AWS: Smithy: ListTaxExemptions output = {taxExemptionDetailsMap: map<AccountId, TaxExemptionDetails>, nextToken}; TaxExemptionDetails = {taxExemptions: list<TaxExemption>, heritageObtainedDetails: Boolean, heritageObtainedParentEntity, heritageObtainedReason}, where heritageObtainedDetails is documented as "the indicator if the tax exemption is inherited from the consolidated billing family management account". In the recorded response each account's entry carries `heritageObtainedDetails: false` — the boolean is present and false when nothing is inherited, not omitted — and carries NO taxExemptions entry, because the PutTaxExemption in cause 5 was refused.
Emulator: `heritageObtainedDetails` is absent from the account's entry entirely, and the entry carries an EXTRA `taxExemptions[0]` for the exemption AWS refused (authority country US / state WA, taxExemptionType displayName+description `501C`, status Pending). (The `systemEffectiveDate` on that extra entry is a per-run clock value and is not itself a requirement — the defect is that the entry exists at all and that the boolean is missing.)
Why: a client cannot distinguish "this exemption is not inherited" from "the service did not say", because the tri-state it actually gets (true / false / absent) is not the boolean the model declares; inheritance-aware billing logic has nothing to branch on. And it reads an exemption that does not exist on the real service.
Fix: ListTaxExemptions' response projection must always populate heritageObtainedDetails for each mapped account; the spurious taxExemptions entry clears once cause 5 refuses the write.
Traces: taxsettings-tax-exemption-workflow#task#3.

=== Cause 7 of 7: BatchGetTaxExemptions repeats both defects on the batch read path (fixture taxsettings-tax-exemption-workflow, step task#4) ===
AWS: Smithy: BatchGetTaxExemptions output = {taxExemptionDetailsMap: map<AccountId, TaxExemptionDetails>, failedAccounts: AccountIds}, with the same TaxExemptionDetails shape as cause 6. The recorded response carries `heritageObtainedDetails: false` for the requested account and no taxExemptions entry.
Emulator: same divergence as cause 6 on a different handler — `heritageObtainedDetails` absent, and an extra `taxExemptions[0]` for the refused exemption (authority US/WA, type `501C`, status Pending).
Why: the batch read is the path a consolidated-billing client uses to fan out across member accounts, so every account it inspects is missing the inheritance indicator and may carry a phantom exemption.
Fix: BatchGetTaxExemptions' own response projection — it builds TaxExemptionDetails separately from the list path, so both need the heritageObtainedDetails member populated (a shared projection would prevent the two copies drifting).
Traces: taxsettings-tax-exemption-workflow#task#4.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout: 18 steps total, 11 passed, 7 mismatched, score 61.1%. emulator_up=true, bootstrap_signal=false, 0 blocked and 0 errored steps, so every one of these is a live fidelity divergence on a service that is answering. All 7 clusters report service=tax with an empty stub_path and empty owned_by.

Cause 1 — taxsettings-batch-tax-registration-workflow#task#1, BatchPutTaxRegistration, expected 200 / actual 200:
  $.errors[0]: missing (expected {'accountId': '000000000000', 'code': None, 'message': 'TCS_INVALID_TAX_REGISTRATION_NUMBER'}, got None)
  $.status: type_mismatch (expected None, got 'Pending')

Cause 2 — taxsettings-batch-tax-registration-workflow#task#2, BatchDeleteTaxRegistration, expected 200 / actual 200:
  $.errors[0]: missing (expected {'accountId': '000000000000', 'code': None, 'message': 'TaxRegistrationNotFound'}, got None)

Cause 3 — taxsettings-supplemental-tax-registration-conflict-error#task#2, PutSupplementalTaxRegistration, expected 409 / actual 200:
  status: expected 409, got 200
  $.errorCode: missing (expected 'GeneralConflict', got None)
  $.message: missing (expected 'Operation cannot be performed due to conflict in current state.', got None)
  $headers.x-amzn-errortype: missing (expected 'ConflictException', got None)

Cause 4 — taxsettings-tax-exemption-workflow#task#1, GetTaxExemptionTypes, expected 200 / actual 200: all 23 indexes of $.taxExemptionTypes diverge. Index 0 expected {'displayName': '501C', 'description': '501C', 'applicableJurisdictions': [{'country':'US','state':'AZ'},{'country':'US','state':'CO'},{'country':'US','state':'CT'},{'country':'US','state':'DC'},{'country':'US','state':'FL'},{'country':'US','state':'GA'},{'country':'US','state':'ID'},{'country':'US','state':'IL'},{'country':'US','state':'IN'},{'country':'US','state':'KS'},{'country':'US','state':'KY'},{'country':'US','state':'LA'},{'country':'US','state':'MA'},{'country':'US','state':'MD'},{'country':'US','state':'MI'},{'country':'US','state':'MN'},{'country':'US','state':'MO'},{'country':'US','state':'MS'},{'country':'US','state':'ND'},{'country':'US','state':'NE'},{'country':'US','state':'NJ'},{'country':'US','state':'NV'},{'country':'US','state':'NY'},{'country':'US','state':'OH'},{'country':'US','state':'OK'},{'country':'US','state':'PA'},{'country':'US','state':'RI'},{'country':'US','state':'SC'},{'country':'US','state':'SD'},{'country':'US','state':'TN'},{'country':'US','state':'TX'},{'country':'US','state':'UT'},{'country':'US','state':'VA'},{'country':'US','state':'VT'},{'country':'US','state':'WI'},{'country':'US','state':'WV'},{'country':'US','state':'WY'}]}, got {'displayName': '501(c)(3)', 'description': 'Charitable organization exempt under section 501(c)(3) of the US Internal Revenue Code', 'applicableJurisdictions': [{'country': 'US'}]}. Divergence key lists $.taxExemptionTypes[0] through [22].

Cause 5 — taxsettings-tax-exemption-workflow#task#2, PutTaxExemption, expected 400 / actual 200:
  status: expected 400, got 200
  $.errorCode: missing (expected 'FieldValidationFailed', got None)
  $.fieldList: missing (expected [{'name': 'AUTHORITY'}], got None)
  $.message: missing (expected 'Provided tax exemption authority is not supported for the tax exemption type.', got None)
  $headers.x-amzn-errortype: missing (expected '<shaped-not-asserted>', got None)

Cause 6 — taxsettings-tax-exemption-workflow#task#3, ListTaxExemptions, expected 200 / actual 200:
  $.taxExemptionDetailsMap.<accountId>.heritageObtainedDetails: missing (expected False, got None)
  $.taxExemptionDetailsMap.<accountId>.taxExemptions[0]: extra (expected None, got {'authority': {'country': 'US', 'state': 'WA'}, 'taxExemptionType': {'displayName': '501C', 'description': '501C', 'applicableJurisdictions': [{'country': 'US', 'state': 'WA'}]}, 'systemEffectiveDate': <per-run clock value>, 'status': 'Pending'})

Cause 7 — taxsettings-tax-exemption-workflow#task#4, BatchGetTaxExemptions, expected 200 / actual 200: identical to cause 6 on the batch handler —
  $.taxExemptionDetailsMap.<accountId>.heritageObtainedDetails: missing (expected False, got None)
  $.taxExemptionDetailsMap.<accountId>.taxExemptions[0]: extra (expected None, got the same refused exemption record)

Model grounding (smithy_query on services/service-com-amazonaws-taxsettings-taxsettings/smithy-model/com-amazonaws-taxsettings-taxsettings.json): BatchPutTaxRegistration output {errors @required, status optional TaxRegistrationStatus}; BatchPutTaxRegistrationError/BatchDeleteTaxRegistrationError {accountId @required, message @required, code optional ErrorCode}; ConflictException httpError 409 {message @required, errorCode @required}; ValidationException httpError 400 {message @required, errorCode @required ValidationExceptionErrorCode (includes FieldValidationFailed), fieldList}; ValidationExceptionField {name @required}; TaxExemptionDetails {taxExemptions, heritageObtainedDetails Boolean, heritageObtainedParentEntity, heritageObtainedReason}; TaxExemptionType {displayName, description, applicableJurisdictions list<Authority{country @required, state}>}.

Note on values: the 12-digit account id, the generated authorityId and the systemEffectiveDate timestamp are per-environment values that fill these templates; the requirements are the statuses, the error codes, the fixed message texts quoted above, the presence of the error rows and of heritageObtainedDetails, and the per-state shape of applicableJurisdictions.

## Steps to Reproduce
Boot the emulator from this checkout and replay trace set simparity-wire-aws-v1 (replay_wire_corpus), or exercise each cause directly against the tax (TaxSettings) endpoint:

Cause 1 — POST /BatchPutTaxRegistration with accountIds=[<a 12-digit account id>] and a taxRegistrationEntry whose registrationId is not a valid registration number for its registrationType/country. Expect 200 with errors[0]={accountId:<that id>, message:'TCS_INVALID_TAX_REGISTRATION_NUMBER'} and no top-level status; observe 200 with errors=[] and status='Pending'. (fixture taxsettings-batch-tax-registration-workflow, step task#1)

Cause 2 — against the same account, which therefore holds no registration, POST /BatchDeleteTaxRegistration with accountIds=[<that id>]. Expect 200 with errors[0]={accountId:<that id>, message:'TaxRegistrationNotFound'}; observe 200 with errors=[]. (step task#2)

Cause 3 — POST /PutSupplementalTaxRegistration once to establish the record, then POST a second taxRegistrationEntry that conflicts with the current state of that record. Expect 409 with x-amzn-errortype: ConflictException and body {errorCode:'GeneralConflict', message:'Operation cannot be performed due to conflict in current state.'}; observe 200 and the write applied. (fixture taxsettings-supplemental-tax-registration-conflict-error, step task#2)

Cause 4 — POST /GetTaxExemptionTypes with an empty body. Expect the service's 23-entry catalog whose first entry is displayName '501C' / description '501C' with one applicableJurisdictions Authority per applicable US state; observe an emulator-authored catalog ('501(c)(3)' with a prose description and applicableJurisdictions collapsed to [{country:'US'}]). (fixture taxsettings-tax-exemption-workflow, step task#1)

Cause 5 — POST /PutTaxExemption with accountIds=[<account id>], exemptionType set to a published type name, exemptionCertificate present, and authority={country:'US', state:<a state NOT in that type's applicableJurisdictions>}. Expect 400 with errorCode 'FieldValidationFailed', fieldList [{name:'AUTHORITY'}] and message 'Provided tax exemption authority is not supported for the tax exemption type.'; observe 200 and the exemption stored. (step task#2)

Cause 6 — POST /ListTaxExemptions. Expect taxExemptionDetailsMap[<account id>] to carry heritageObtainedDetails:false and no taxExemptions entry; observe heritageObtainedDetails absent and a taxExemptions[0] holding the exemption that should have been refused in cause 5. (step task#3)

Cause 7 — POST /BatchGetTaxExemptions with accountIds=[<account id>]. Expect the same {heritageObtainedDetails:false, no taxExemptions} projection; observe the same two divergences as cause 6 from the separate batch handler. (step task#4)
