Issue-Source: trace-sweep-divergence
Issue-Trace: partnercentral-benefits-list-and-tag-validation-error-paths
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: partnercentral-benefits-list-and-tag-validation-error-paths
Issue-Shape-Id: com.amazonaws.partnercentralbenefits#PartnerCentralBenefitsService
Issue-Key: partnercentral-benefits|ListBenefits|4xx->4xx|status
Issue-Key: partnercentral-benefits|ListTagsForResource|4xx->4xx|$.Message
Issue-Key: partnercentral-benefits|ListTagsForResource|4xx->4xx|$.__type
Issue-Key: partnercentral-benefits|ListTagsForResource|4xx->4xx|status
Issue-Commit: 9cc0add35ee0fa09b41f1226e545932e7bbe5c43

# Partner registration access denial answered as 403 and as ResourceNotFoundException

> _Note: this issue was identified against the code base at commit `9cc0add35ee0fa09b41f1226e545932e7bbe5c43`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListTagsForResource

## Description
This issue bundles 2 causes sharing one fault: on the request path where the caller has no active AWS partner benefit registration, AWS answers with HTTP 400 carrying `com.amazon.partnercentral.benefits#AccessDeniedException`, and the emulator instead answers with a different status and, on one operation, a different error shape altogether.

Cause 1 of 2: ListBenefits returns the wrong HTTP status for the access denial. AWS: recorded behavior only — the recorded response for this request is HTTP 400 (the awsJson1_0 error shape is AccessDeniedException; note the model declares AccessDeniedException with @error("client") and @httpError(403), so AWS's 400 on this path is not predictable from the model and the RECORDING is the authority). Emulator: HTTP 403. Why: an HTTP status is a client-visible requirement — a client (or retry/classification middleware) branching on 400 vs 403 takes the wrong branch, and any test pinned to AWS's recorded status fails against the emulator. Hypothesis: the emulator derives the status from the shape's modelled @httpError(403) rather than from the status AWS actually serves for this denial. Fix: the status selected when this handler's authorization/registration precondition fails — i.e. the error-to-status mapping used by ListBenefits' handler, not the body, since only `status` diverged. Traces: partnercentral-benefits-list-and-tag-validation-error-paths#task#1.

Cause 2 of 2: ListTagsForResource reports the wrong error shape AND status, leaking an internal catalog lookup. AWS: recorded behavior only — HTTP 400 with `$.__type` = `com.amazon.partnercentral.benefits#AccessDeniedException`; the message states the registration rule and is reproducible from it ("This action requires an active AWS partner benefit. Register as an AWS partner if you haven't already, or contact AWS partner central support."). Emulator: HTTP 404 with `$.__type` = `ResourceNotFoundException` and message "Catalog `null` not found". Why: the error CODE is a requirement, and a client distinguishing "not entitled" from "resource absent" is told the resource does not exist, so it will not surface a registration prompt and may treat the call as a successful negative lookup; the literal `null` in the message also shows the emulator interpolating an unset value into client-facing text. Note ListTagsForResourceRequest has only `resourceArn` (Smithy: TaggableResourceArn, @required, @length 1..1000, catalog-ARN @pattern) and NO `Catalog` member, so there is no catalog to resolve from the input — the handler is consulting a catalog it was never given and failing that lookup before the registration/authorization check runs. Fix: ListTagsForResource's handler — the entitlement/authorization precondition must be evaluated before any catalog-state lookup, and the catalog must be derived from the resourceArn rather than from an absent input field. Traces: partnercentral-benefits-list-and-tag-validation-error-paths#task#4.

No existing open issue covers these (a search of the open issues for "partnercentral-benefits" and "benefit" returned none); the other 15 corpus divergences for this service are the missing-ValidationException-Message fault filed under the two companion bundles.

## Evidence
Replay of trace set simparity-wire-aws-v1, fixture `partnercentral-benefits-list-and-tag-validation-error-paths` (2 of 17 corpus divergences, both verdict mismatch).

task#1 — ListBenefits, divergence key `service=partnercentral-benefits|ListBenefits|status=400->403|keys=status`. Expected status 400; actual status 403. Status is the only diverging key, so the body matched.

task#4 — ListTagsForResource, divergence key `service=partnercentral-benefits|ListTagsForResource|status=400->404|keys=$.Message,$.__type,status`:
  expected: status 400, `$.__type` = 'com.amazon.partnercentral.benefits#AccessDeniedException', `$.Message` = "This action requires an active AWS partner benefit. Register as an AWS partner if you haven't already, or contact AWS partner central support."
  actual:   status 404, `$.__type` = 'ResourceNotFoundException', `$.Message` = 'Catalog `null` not found'.

Model evidence (on-disk Smithy, PartnerCentralBenefitsService, awsJson1_0): AccessDeniedException { Message @required } @error(client) @httpError(403); ResourceNotFoundException @httpError(404); ListTagsForResource binds AccessDeniedException, InternalServerException, ResourceNotFoundException, ThrottlingException, ValidationException, and its input shape ListTagsForResourceRequest declares exactly one member, `resourceArn` — no Catalog member exists to produce the emulator's `Catalog \`null\`` lookup.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay trace set simparity-wire-aws-v1.
2. Replay fixture `partnercentral-benefits-list-and-tag-validation-error-paths` step task#1: POST the recorded ListBenefits request (X-Amz-Target PartnerCentralBenefitsService.ListBenefits) for a caller without an active partner benefit registration. Observe HTTP 403; AWS's recorded response for this request is HTTP 400.
3. Replay the same fixture's step task#4: POST the recorded ListTagsForResource request, whose only member is `resourceArn`. Observe HTTP 404 with `__type` ResourceNotFoundException and message "Catalog `null` not found"; AWS returns HTTP 400 with `__type` com.amazon.partnercentral.benefits#AccessDeniedException and the "requires an active AWS partner benefit" message.
4. Re-replay both steps after the fix and confirm the status and `__type` match the recordings.
