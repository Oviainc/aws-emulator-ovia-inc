Issue-Source: trace-sweep-divergence
Issue-Trace: marketplace-deployment-list-tags-for-resource-nonexistent-arn-not-found
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: marketplace-deployment-list-tags-for-resource-nonexistent-arn-not-found
Issue-Shape-Id: com.amazonaws.marketplacedeployment#AWSMPDeploymentParametersService
Issue-Key: marketplace-deployment|ListTagsForResource|4xx->4xx|$headers.x-amzn-errortype
Issue-Commit: 0c5145d1821481e15376cbd37da6da62f6188e28

# ListTagsForResource returns ValidationException error code for a nonexistent deployment-parameter ARN

> _Note: this issue was identified against the code base at commit `0c5145d1821481e15376cbd37da6da62f6188e28`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListTagsForResource

## Description
Cause 1 of 1: ListTagsForResource mis-classifies a resource ARN that names no stored deployment parameter, emitting the wrong wire error code (x-amzn-errortype).

AWS: Smithy (services/service-com-amazonaws-marketplacedeployment-awsmpdeploymentparametersservice/smithy-model/...): ListTagsForResource GET /tags/{resourceArn}, resourceArn is a required @httpLabel; modeled errors AccessDeniedException (403), InternalServerException (500), ResourceNotFoundException (404), ThrottlingException (429), ValidationException (400). Recorded behavior: for a resource ARN that this service holds no deployment parameter for, the recorded response is an error whose x-amzn-errortype is NOT 'ValidationException' — the replay step asserts the recorded error type's shape rather than a literal ('<shaped-not-asserted>'), at the recorded HTTP status 400. The emulator's own code states the recorded convention explicitly: service-com-amazonaws-marketplacedeployment-awsmpdeploymentparametersservice/src/scaffolded.rs:221-238 documents that an ARN naming no deployment parameter is answered with the error code 'NotFoundException' (not the modeled shape name ResourceNotFoundException), and src/state.rs:499-538 documents that only an ARN outside this service's syntax/vendor namespace is a ValidationException. So an ARN this API can address but holds no record for must never be reported as a validation failure of the input member.

Emulator: replay of fixture marketplace-deployment-list-tags-for-resource-nonexistent-arn-not-found, step task#1, answers HTTP 400 (status matches the recording) but with x-amzn-errortype: ValidationException. Verdict: mismatch on $headers.x-amzn-errortype — shape_mismatch (expected '<shaped-not-asserted>', got 'ValidationException'). That is the ValidationException branch of src/ops/list_tags_for_resource.rs:154-156, i.e. crate::state::validate_resource_arn rejected the fixture's ARN as 'not a resource ARN of this service' before the store was ever consulted, so the not-found path at list_tags_for_resource.rs:167-172 (crate::state::parameter_not_found) never runs.

Why: a client branches on the error code, not on the status. A caller that reads 'ValidationException' concludes its resourceArn is permanently malformed and must be rewritten by a human, when in fact the ARN is addressable and the resource simply does not exist (or was deleted) — so tag-reconciliation logic that is supposed to treat a missing resource as 'nothing to read' instead surfaces a hard input error, and any SDK error-shape matching on the recorded not-found code never matches at all.

Fix: the ListTagsForResource handler's ARN classification — crate::state::validate_resource_arn's vendor/syntax acceptance (and its use as a pre-lookup gate in the handler) must accept the resource ARNs this service actually addresses, so that a well-formed ARN naming no stored parameter falls through to the store lookup and is answered with this service's recorded not-found code/status rather than the 400 ValidationException input refusal. Only the error code is wrong here; the 400 status in the recording matched, so classification, not the status mapping, is the component at fault.

Traces: marketplace-deployment-list-tags-for-resource-nonexistent-arn-not-found#task#1 (1 affected step, cluster key service=marketplace-deployment|ListTagsForResource|status=400->400|keys=$headers.x-amzn-errortype).

## Evidence
Replay summary: trace_set_id=simparity-wire-aws-v1, emulator_up=true, bootstrap_signal=false, steps_total=1, steps_passed=0, steps_mismatched=1, score_pct=0.0, state_reset_failures=0.

Divergence cluster (verdict=mismatch):
- fixture_id: marketplace-deployment-list-tags-for-resource-nonexistent-arn-not-found
- step_key: task#1
- service: marketplace-deployment, operation: ListTagsForResource
- expected_status: 400, actual_status: 400 (status matched)
- detail: $headers.x-amzn-errortype: shape_mismatch (expected '<shaped-not-asserted>', got 'ValidationException')
- affected (1): marketplace-deployment-list-tags-for-resource-nonexistent-arn-not-found#task#1
- stub_path: (none), owned_by: (none)

Expected: the recorded error type for an ARN naming no deployment parameter (per the emulator's own documented recorded convention, the code 'NotFoundException' — scaffolded.rs:227 NOT_FOUND_CODE), i.e. anything other than a validation-of-input error code.
Actual: x-amzn-errortype: ValidationException, produced by list_tags_for_resource.rs:154-156 via state.rs validate_resource_arn.

Note: the recorded error MESSAGE is not treated as a requirement here; the divergence is the error code on the wire, which clients branch on.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it.
2. Replay wire fixture marketplace-deployment-list-tags-for-resource-nonexistent-arn-not-found, step task#1 — a ListTagsForResource GET /tags/{resourceArn} whose resourceArn names no stored deployment parameter.
3. Observe the response headers: the emulator returns HTTP 400 with x-amzn-errortype: ValidationException.
4. The recorded response's error type is not ValidationException (the step asserts its shape; the emulator's value fails that assertion), so the step is reported as a mismatch on $headers.x-amzn-errortype.
5. Re-run replay_wire_corpus after the fix: the step must pass with the recorded error type for an unknown resource ARN.
