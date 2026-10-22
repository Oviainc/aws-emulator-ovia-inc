Issue-Source: trace-sweep-divergence
Issue-Trace: uxc-error-invalid-visible-service-pattern
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: uxc-error-invalid-visible-service-pattern uxc-configure-visible-regions-roundtrip uxc-partial-update-preserves-other-fields
Issue-Shape-Id: com.amazonaws.uxc#AWSAccountUXSetting
Issue-Key: uxc|UpdateAccountCustomizations|4xx->4xx|$.message
Issue-Key: uxc|UpdateAccountCustomizations|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: uxc|UpdateAccountCustomizations|2xx->2xx|$.visibleRegions[*]
Issue-Key: uxc|UpdateAccountCustomizations|2xx->2xx|$.visibleServices[*]
Issue-Commit: 5598afbcdd0af564c379fb305d1563cf7235aea7

# UpdateAccountCustomizations diverges on list order and invalid-member error

> _Note: this issue was identified against the code base at commit `5598afbcdd0af564c379fb305d1563cf7235aea7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateAccountCustomizations

## Description
THREE causes, all on uxc UpdateAccountCustomizations (PATCH /v1/account-customizations). All three must be closed before submitting; each names its own fixture and step so each can be re-replayed.

Cause 1 of 3: an invalid body member is refused with the wrong error code (fixture uxc-error-invalid-visible-service-pattern, step task#2).
AWS: recorded behavior only for the error identity -- the Smithy model declares UpdateAccountCustomizations' errors as AccessDeniedException (403), InternalServerException (500), ThrottlingException (429) and ValidationException (400), and constrains visibleServices members as Service @length 1..64 @pattern ^[a-z0-9]+(-[a-z0-9]+)*$. The recording shows that a body whose visibleServices element violates that pattern is refused BEFORE modeled constraint validation, with HTTP 400 and x-amzn-errortype: BadRequestException (an error code this service's model does not declare) and the generic body message 'Invalid request body' (example text; the requirement is the status and the error code).
Emulator: HTTP 400 -- status correct -- but x-amzn-errortype: ValidationException, with a per-member constraint message (example: "1 validation error detected: Value 'Invalid_Service!' at 'visibleServices.1.member' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[a-z0-9]+(-[a-z0-9]+)*$").
Why: the error code in x-amzn-errortype is what an SDK uses to pick the modeled exception it raises. A client that catches the service's BadRequestException for a malformed body gets an unmatched ValidationException (or a generic service error) from the emulator, so error-path code and retry/branching logic behave differently against the emulator than against AWS.
Fix: the request-body boundary refusal for this service -- the validation layer that turns a constraint violation in the PATCH body into a response -- which must answer this class of malformed-body rejection with the BadRequestException error code and status 400 rather than lifting it into the modeled ValidationException.

Cause 2 of 3: visibleRegions is not echoed in the order the request supplied (fixture uxc-configure-visible-regions-roundtrip, step task#4).
AWS: Smithy: UpdateAccountCustomizations.visibleRegions -> RegionsList, a LIST (@length max 100, member Region @pattern ^[a-z]{2}(-[a-z]{1,10}){1,2}-[1-9]$) with no @uniqueItems, so member order is part of the value, and the documented output is "The current list of visible Region codes after the update". The recording shows HTTP 200 whose visibleRegions is the configured sequence ['eu-west-1', 'us-east-1', 'us-west-2'].
Emulator: HTTP 200 with the same three members in a different order, ['us-east-1', 'us-west-2', 'eu-west-1'] -- the earlier-configured members retain their positions and the newly supplied one lands last, i.e. the stored list is not replaced by the request's sequence. Hypothesis: the regions written for this account in an earlier step of the fixture survive in their original positions instead of the update's sequence being stored verbatim.
Why: the operation is @idempotent and documented to answer with the current settings, so a client that PATCHes a region list and renders or diffs the echoed list sees members in an order it never sent; a roundtrip assertion (PATCH then GetAccountCustomizations) that compares lists positionally fails against the emulator while passing against AWS.
Fix: the UpdateAccountCustomizations write path and the stored record it reads back -- the list supplied for a member must replace the stored list, preserving the request's member order, and the response must serialize that order.

Cause 3 of 3: visibleServices is not preserved in order across a partial update (fixture uxc-partial-update-preserves-other-fields, step task#2).
AWS: Smithy: UpdateAccountCustomizations.visibleServices -> ServiceList, a LIST (@length max 500, member Service @length 1..64 @pattern ^[a-z0-9]+(-[a-z0-9]+)*$) with no @uniqueItems; the operation documents that "Omitted settings remain unchanged" and that the response carries "The current list of visible service identifiers after the update". The recording shows HTTP 200 echoing the previously configured sequence ['s3', 'ec2'] unchanged by an update that did not mention visibleServices.
Emulator: HTTP 200 with those members reordered to ['ec2', 's3'].
Why: a client that sets a service list and then updates an unrelated setting (for example accountColor) gets its list back in a different order; order-sensitive console rendering and any positional comparison diverge, and the operation is not observably order-stable across the partial update AWS documents as leaving the setting unchanged.
Fix: as Cause 2 -- the stored customization record and the UpdateAccountCustomizations read-back, which must round-trip a list member sequence exactly.

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator up, 36 steps, 26 passed, 10 mismatched).

Cause 1 -- uxc-error-invalid-visible-service-pattern#task#2, UpdateAccountCustomizations, expected 400 / actual 400: "$headers.x-amzn-errortype: value_mismatch (expected 'BadRequestException', got 'ValidationException')" and "$.message: value_mismatch (expected 'Invalid request body', got \"1 validation error detected: Value 'Invalid_Service!' at 'visibleServices.1.member' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[a-z0-9]+(-[a-z0-9]+)*$\")". The status matches; the error code does not.

Cause 2 -- uxc-configure-visible-regions-roundtrip#task#4, UpdateAccountCustomizations, expected 200 / actual 200: "$.visibleRegions[0]: value_mismatch (expected 'eu-west-1', got 'us-east-1')", "$.visibleRegions[1]: value_mismatch (expected 'us-east-1', got 'us-west-2')", "$.visibleRegions[2]: value_mismatch (expected 'us-west-2', got 'eu-west-1')" -- same membership, different sequence.

Cause 3 -- uxc-partial-update-preserves-other-fields#task#2, UpdateAccountCustomizations, expected 200 / actual 200: "$.visibleServices[0]: value_mismatch (expected 's3', got 'ec2')", "$.visibleServices[1]: value_mismatch (expected 'ec2', got 's3')".

State-reset failures in this replay: 0, so the stale ordering in Causes 2-3 is produced within each fixture, not leaked from a previous fixture.

Emulator source: services/service-com-amazonaws-uxc-awsaccountuxsetting/src/ops/update_account_customizations.rs lines 210-243 merge the input into the record and read it back; services/service-com-amazonaws-uxc-awsaccountuxsetting/src/state.rs lines 14-22 hold visible_services / visible_regions as Option<Vec<String>>; the handler's only pre-state refusal (lines 176-195) is the accountColor enum guard, so a member-pattern violation is refused by the shared boundary validator as ValidationException.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay trace set simparity-wire-aws-v1.
2. Cause 1: replay fixture uxc-error-invalid-visible-service-pattern, step task#2 -- PATCH /v1/account-customizations with a visibleServices element that violates ^[a-z0-9]+(-[a-z0-9]+)*$ (recording uses 'Invalid_Service!' as the second element) and read the x-amzn-errortype response header: expect BadRequestException, emulator sends ValidationException.
3. Cause 2: replay fixture uxc-configure-visible-regions-roundtrip through step task#4 -- configure visibleRegions, then PATCH the list ['eu-west-1','us-east-1','us-west-2'] and compare $.visibleRegions positionally; the emulator answers ['us-east-1','us-west-2','eu-west-1'].
4. Cause 3: replay fixture uxc-partial-update-preserves-other-fields through step task#2 -- set visibleServices to ['s3','ec2'], then PATCH only an unrelated setting and compare $.visibleServices positionally; the emulator answers ['ec2','s3'].
