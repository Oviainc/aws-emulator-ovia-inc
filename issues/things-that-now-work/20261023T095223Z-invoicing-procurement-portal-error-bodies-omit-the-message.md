Issue-Source: trace-sweep-divergence
Issue-Trace: invoicing-procurement-portal-preference-errors
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: invoicing-procurement-portal-preference-errors invoicing-procurement-portal-preference-lifecycle
Issue-Shape-Id: com.amazonaws.invoicing#Invoicing
Issue-Key: invoicing|GetProcurementPortalPreference|4xx->4xx|$.Message
Issue-Key: invoicing|SendProcurementPortalValidation|4xx->4xx|$.Message
Issue-Key: invoicing|VerifyProcurementPortalValidation|4xx->4xx|$.Message
Issue-Commit: 3113486ab2df4ef97dea8b3047048aff3439051f

# Invoicing procurement-portal error bodies omit the Message member and use constant text

> _Note: this issue was identified against the code base at commit `3113486ab2df4ef97dea8b3047048aff3439051f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetProcurementPortalPreference

## Description
FOUR causes, all on the invoicing procurement-portal-preference plane: every refusal this plane answers reaches the wire WITHOUT the `Message` member the recorded AWS bodies carry, so the explanatory text a client reads is simply absent. The HTTP status and the `__type` error code are correct on all four (the replay flags neither), so the defect is confined to the message member: its wire key and its text.

Shared diagnosis (applies to all four causes). Smithy: `ResourceNotFoundException` and `ValidationException` each declare a member `message` (target BasicString, len 0..1024) with NO `@jsonName` (smithy_query query="shape", shape="ResourceNotFoundException,ValidationException"); the protocol is awsJson1_0. The recorded procurement-portal error bodies key that text as `Message` (capital M), while the recorded BatchGetInvoiceProfile 403 in this same corpus keys it as lowercase `message` — so the fix is NOT a blanket capitalisation of every invoicing error, it is to serialize this plane's errors the way the recordings show. Hypothesis: the emulator emits the model member name verbatim (`message`), so a case-sensitive reader of `Message` finds nothing even though the handler did populate the text.

Cause 1 of 4: GetProcurementPortalPreference on an ARN that does not exist.
AWS: 404 `ResourceNotFoundException` whose message is BUILT FROM THE REQUEST — it names the resource type and the exact ARN asked about. Recorded example: "Procurement portal preference with arn arn:aws:invoicing::000000000000:procurement-portal-preference/does-not-exist-000 does not exist" (recorded behavior only; the model documents only the generic shape doc "The resource could not be found."). The request-derived rule, not that literal sentence, is the requirement.
Emulator: 404 with the right `__type`, but `$.Message` absent (replay: "$.Message: missing ... got None"). The handler builds the exception through crate::state::no_such_resource, which sets a CONSTANT message (`RESOURCE_NOT_FOUND_MESSAGE` = "The resource could not be found.") and never interpolates the ARN into prose.
Why: a client that logs or branches on the message field of a 404 gets an empty value, and even once the key is fixed it cannot tell WHICH preference was missing from the message, because the recorded text names it and the constant does not.
Fix: the error serialization of this service's exception shapes (the `Message` key) plus the not-found constructor in the service's state/error helpers, which must take the requested ARN and render AWS's request-derived sentence for the procurement-portal plane.
Traces: invoicing-procurement-portal-preference-errors#task#1.

Cause 2 of 4: SendProcurementPortalValidation on an ARN that does not exist.
AWS: 404 `ResourceNotFoundException` (modeled error of the operation, httpError 404) with the same request-derived message naming the requested preference ARN.
Emulator: 404, correct `__type`, `$.Message` missing — same shared cause reached from send_procurement_portal_validation.rs, whose existence check returns crate::state::no_such_resource(&input.procurement_portal_preference_arn).
Why: identical client impact to cause 1 on the write path, so a client cannot distinguish "no such preference" details from an empty body field.
Fix: same serializer/constructor seam as cause 1; this op needs no separate logic once the constructor and key are right.
Traces: invoicing-procurement-portal-preference-errors#task#4.

Cause 3 of 4: VerifyProcurementPortalValidation on an ARN that does not exist.
AWS: 404 `ResourceNotFoundException` with the request-derived message naming the ARN.
Emulator: 404, correct `__type`, `$.Message` missing (same constructor, verify_procurement_portal_validation.rs existence check).
Why: same as causes 1-2.
Fix: same seam.
Traces: invoicing-procurement-portal-preference-errors#task#5.

Cause 4 of 4: VerifyProcurementPortalValidation submitted with a code that is not the outstanding one.
AWS: 400 `ValidationException` whose message is derived from the preference's validation STATE — it reports the failed-attempt counter against a three-attempt budget. Recorded example: "The code you entered is invalid. Attempt 1 of 3 failed. Please try again." (recorded behavior only; the model states only reason/fieldList/message members). The requirement is 400 + `ValidationException` + a message that states the attempt number and the budget; the exact prose is an example.
Emulator: 400 with the right `__type` and no `$.Message` at all. The handler does construct `ValidationException { message: Some(INVALID_CODE_MESSAGE), reason: Some("invalidInput") }`, i.e. a single constant with no attempt counter, and tracks no per-preference failed-attempt count at all (state keeps only `IssuedPortalValidation { code, issued_at, verified_at }`).
Why: an OTP client shows the user nothing, and cannot implement the "3 attempts then stop" flow AWS's message drives, because neither the count nor the budget is reported anywhere on the wire.
Fix: the VerifyProcurementPortalValidation handler plus the stored validation record — the failed-attempt count has to live beside the issued code so the refusal can state it; the message key itself is the shared serializer fix.
Traces: invoicing-procurement-portal-preference-lifecycle#task#7.

## Evidence
Replay of simparity-wire-aws-v1 against the emulator built from mainline (emulator_up=true, bootstrap_signal=false, state_reset_failures=0):

1) invoicing-procurement-portal-preference-errors#task#1, GetProcurementPortalPreference, verdict=mismatch, expected 404 / actual 404: "$.Message: missing (expected 'Procurement portal preference with arn arn:aws:invoicing::000000000000:procurement-portal-preference/does-not-exist-000 does not exist', got None)".
2) invoicing-procurement-portal-preference-errors#task#4, SendProcurementPortalValidation, expected 404 / actual 404: same "$.Message: missing" detail.
3) invoicing-procurement-portal-preference-errors#task#5, VerifyProcurementPortalValidation, expected 404 / actual 404: same "$.Message: missing" detail.
4) invoicing-procurement-portal-preference-lifecycle#task#7, VerifyProcurementPortalValidation, expected 400 / actual 400: "$.Message: missing (expected 'The code you entered is invalid. Attempt 1 of 3 failed. Please try again.', got None)".

In all four the status and `__type` matched (neither is reported as diverging), so only the message member diverges.

Contrast inside the same corpus: the recorded invoicing BatchGetInvoiceProfile 403 body keys its text as lowercase `message`, while all four procurement-portal bodies above key it as `Message`.

Code, as checked out:
- services/service-com-amazonaws-invoicing-invoicing/src/state.rs:710-727 — `RESOURCE_NOT_FOUND_MESSAGE` is a constant ("The resource could not be found.") and `no_such_resource(arn)` sets it, putting the ARN in a separate member instead of the recorded sentence.
- services/service-com-amazonaws-invoicing-invoicing/src/ops/verify_procurement_portal_validation.rs:230-244 — the invalid-code refusal uses a single `INVALID_CODE_MESSAGE` constant and `reason: "invalidInput"`, with no attempt counter.
- services/service-com-amazonaws-invoicing-invoicing/src/ops/get_procurement_portal_preference.rs:167-176 and src/ops/send_procurement_portal_validation.rs:178-188 — both reach the same `no_such_resource` constructor.
- Model: smithy_query(query="shape", shape="ResourceNotFoundException,ValidationException", model_path="services/service-com-amazonaws-invoicing-invoicing/smithy-model/com-amazonaws-invoicing-invoicing.json") — member `message`, no `@jsonName`; httpError 404 / 400.

## Steps to Reproduce
1. Boot the emulator from mainline and replay the simparity-wire-aws-v1 corpus.
2. Cause 1: POST Invoicing.GetProcurementPortalPreference with ProcurementPortalPreferenceArn = arn:aws:invoicing::<account>:procurement-portal-preference/does-not-exist-000 (an ARN no create produced). Observe 404 with `__type: ResourceNotFoundException` and NO `Message` key in the JSON body; AWS returns 404 whose Message names the resource type and that ARN. Replay fixture invoicing-procurement-portal-preference-errors step task#1.
3. Cause 2: same ARN against Invoicing.SendProcurementPortalValidation -> 404 with no `Message` (fixture invoicing-procurement-portal-preference-errors step task#4).
4. Cause 3: same ARN against Invoicing.VerifyProcurementPortalValidation with any Code -> 404 with no `Message` (fixture invoicing-procurement-portal-preference-errors step task#5).
5. Cause 4: create a preference, call SendProcurementPortalValidation on it, then call VerifyProcurementPortalValidation with a Code other than the issued one. Observe 400 `ValidationException` with no `Message`; AWS returns 400 whose Message states the failed-attempt number out of 3. Replay fixture invoicing-procurement-portal-preference-lifecycle step task#7.
