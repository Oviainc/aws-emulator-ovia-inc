Issue-Source: trace-sweep-divergence
Issue-Trace: appintegrations-application-lifecycle
Issue-Step: task#6
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: appintegrations-application-lifecycle appintegrations-event-integration-lifecycle
Issue-Shape-Id: com.amazonaws.appintegrations#AmazonAppIntegrationService
Issue-Key: app-integrations|TagResource|2xx->2xx|status
Issue-Key: app-integrations|UntagResource|2xx->2xx|status
Issue-Commit: 7c73ea6d7a96c997ecb38a88ea3442295b1a24d6

# app-integrations tagging operations answer 200 instead of 204 on empty-output success

> _Note: this issue was identified against the code base at commit `7c73ea6d7a96c997ecb38a88ea3442295b1a24d6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TagResource

## Description
TWO net-new wire root causes on the AppIntegrations emulator, both on the SUCCESS path of a tagging operation whose modeled output has no members. Both share one fault: the operation declares the generic HTTP 200 success status instead of the 204 No Content AWS answers for these two tagging operations, so every successful Tag/Untag call diverges on status regardless of the resource type being tagged (the recorded clusters cover both an Application and an Event Integration). The service is bootstrapped and both operations are implemented and mutate state correctly — the tag set written is read back by ListTagsForResource and the Get/List operations — so these are response-status fidelity defects, not bootstrap gaps or state defects. No open issue covers them: the only open AppIntegrations issue, 20261029T223053Z-appintegrations-refusal-fidelity-createapplication-duplicate-name-namespace-answers.md, is about refusal classes and messages on CreateApplication / CreateDataIntegrationAssociation / UpdateDataIntegrationAssociation and carries no replay key for either step below.

Cause 1 of 2 — TagResource success answers HTTP 200 instead of HTTP 204.
AWS: Smithy TagResource — POST /tags/{resourceArn}; input resourceArn -> Arn @httpLabel @required @length 1..2048 @pattern ^arn:aws:...$, tags -> TagMap @required @length 1..200; output shape TagResourceResponse has NO members; modeled errors InvalidRequestException (400), ResourceNotFoundException (404), ThrottlingException (429), InternalServiceError (500) (model: services/service-com-amazonaws-appintegrations-amazonappintegrationservice/smithy-model/com-amazonaws-appintegrations-amazonappintegrationservice.json). The model's @http trait carries no explicit success code, so the model alone reports only the protocol default 200; the success STATUS is therefore recorded behavior only: AWS answers HTTP 204 with no content for a successful TagResource on any taggable AppIntegrations resource. No error code or message is involved — this is a 2xx success path.
Emulator: answers HTTP 200 for the same successful call. The status is hard-wired on the operation: `const SUCCESS_STATUS: u16 = 200u16;` in the Operation impl, and the handler returns `Ok(TagResourceOutput {})`.
Why: a client that branches on the exact success status — a generated SDK or middleware that treats 204 as "no body to parse" and 200 as "parse a body", a conditional-request or proxy layer, or any test asserting AWS's documented response — sees a status AWS never returns for this operation and either attempts to read a body that is not there or reports the call as unexpected. Status is part of the contract even when the payload is empty.
Fix: the success-status declaration of the TagResource operation (its Operation impl's SUCCESS_STATUS, i.e. the operation's serializer contract) must be the 204 AWS returns for this empty-output operation; the handler body and the state mutation are already correct and need no change.
Traces: appintegrations-application-lifecycle#task#6 (representative) and appintegrations-event-integration-lifecycle#task#7 — the same divergence on an Event Integration ARN, which is why this is one cause and not two.

Cause 2 of 2 — UntagResource success answers HTTP 200 instead of HTTP 204.
AWS: Smithy UntagResource — DELETE /tags/{resourceArn}; input resourceArn -> Arn @httpLabel @required @length 1..2048 @pattern ^arn:aws:...$, tagKeys -> TagKeyList @httpQuery("tagKeys") @required @length 1..200; output shape UntagResourceResponse has NO members; modeled errors InvalidRequestException (400), ResourceNotFoundException (404), ThrottlingException (429), InternalServiceError (500). As with TagResource the @http trait states no explicit code, so the success status is recorded behavior only: AWS answers HTTP 204 with no content for a successful UntagResource, including the idempotent case where some requested keys were not present on the resource.
Emulator: answers HTTP 200 for the same successful call; `const SUCCESS_STATUS: u16 = 200u16;` on the UntagResource Operation impl, handler returns `Ok(UntagResourceOutput {})`.
Why: same client consequence as Cause 1 — a caller keyed on 204/no-content for a successful untag sees 200 and treats the response as carrying a payload, and any assertion on AWS's success status fails even though the tags were in fact removed.
Fix: the success-status declaration of the UntagResource operation (its Operation impl's SUCCESS_STATUS / serializer contract) must be 204. State handling is already correct.
Traces: appintegrations-application-lifecycle#task#8 (representative) and appintegrations-event-integration-lifecycle#task#9.

Both causes must be closed before submitting: fixing only TagResource leaves UntagResource diverging on exactly the same step class.

## Evidence
Wire corpus simparity-wire-aws-v1 replayed against the emulator booted from this checkout: emulator_up=true, 7 fixtures replayed, 53 steps, 48 passed, 5 mismatched (score 90.6%), 2 divergence clusters — both of them the ones below, both with empty stub_path and empty owned_by.

Cause 1 — cluster service=app-integrations|TagResource|status=204->200|keys=status, verdict mismatch:
  fixture appintegrations-application-lifecycle, step task#6 — POST /tags/{resourceArn} with a tag map, against the ARN of an Application created earlier in the same fixture
  expected HTTP 204 (no content)
  got      HTTP 200
  detail: "status: expected 204, got 200"
  affected (2): appintegrations-application-lifecycle#task#6, appintegrations-event-integration-lifecycle#task#7

Cause 2 — cluster service=app-integrations|UntagResource|status=204->200|keys=status, verdict mismatch:
  fixture appintegrations-application-lifecycle, step task#8 — DELETE /tags/{resourceArn}?tagKeys=... removing keys task#6 added
  expected HTTP 204 (no content)
  got      HTTP 200
  detail: "status: expected 204, got 200"
  affected (2): appintegrations-application-lifecycle#task#8, appintegrations-event-integration-lifecycle#task#9

Only the status key diverges in both clusters (keys=status): the response bodies and the resulting tag state match, which is what isolates the fault to the declared success status rather than to the handlers.

Code evidence (fix sites): services/service-com-amazonaws-appintegrations-amazonappintegrationservice/src/ops/tag_resource.rs:145 (`const SUCCESS_STATUS: u16 = 200u16;`) and .../src/ops/untag_resource.rs:141 (same). Model evidence: smithy_query(query='operation', operation='TagResource,UntagResource', model_path='services/service-com-amazonaws-appintegrations-amazonappintegrationservice/smithy-model/com-amazonaws-appintegrations-amazonappintegrationservice.json') — both output shapes have no members and no explicit @http success code.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus simparity-wire-aws-v1.
2. Cause 1: replay fixture appintegrations-application-lifecycle step task#6 — create an Application, then POST /tags/<application ARN> with a non-empty tag map. Expect HTTP 204 with no content; observe HTTP 200. Repeat with fixture appintegrations-event-integration-lifecycle step task#7 (an Event Integration ARN) for the same divergence.
3. Cause 2: replay fixture appintegrations-application-lifecycle step task#8 — DELETE /tags/<application ARN>?tagKeys=<keys added in task#6>. Expect HTTP 204 with no content; observe HTTP 200. Repeat with appintegrations-event-integration-lifecycle step task#9.
4. Confirm the tag state itself is unaffected: ListTagsForResource after each step already reports the expected tag set, so only the success status needs changing.
5. After fixing BOTH operations' declared success status, re-replay the corpus and confirm appintegrations-application-lifecycle#task#6, appintegrations-application-lifecycle#task#8, appintegrations-event-integration-lifecycle#task#7 and appintegrations-event-integration-lifecycle#task#9 all pass.
