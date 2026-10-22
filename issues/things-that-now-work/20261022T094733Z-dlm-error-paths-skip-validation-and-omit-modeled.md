Issue-Source: trace-sweep-divergence
Issue-Trace: dlm-create-policy-invalid-request-errors
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: dlm-create-policy-invalid-request-errors dlm-create-policy-invalid-description-characters-rejected dlm-resource-not-found-errors dlm-update-policy-error-paths dlm-tag-resource-reserved-aws-prefix-key-rejected
Issue-Shape-Id: com.amazonaws.dlm#dlm_20180112
Issue-Key: dlm|CreateLifecyclePolicy|4xx->2xx|$.Code
Issue-Key: dlm|CreateLifecyclePolicy|4xx->2xx|$.Message
Issue-Key: dlm|CreateLifecyclePolicy|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: dlm|CreateLifecyclePolicy|4xx->2xx|status
Issue-Key: dlm|CreateLifecyclePolicy|4xx->4xx|$.Code
Issue-Key: dlm|CreateLifecyclePolicy|4xx->4xx|$.Message
Issue-Key: dlm|CreateLifecyclePolicy|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: dlm|GetLifecyclePolicy|4xx->4xx|$.Code
Issue-Key: dlm|GetLifecyclePolicy|4xx->4xx|$.Message
Issue-Key: dlm|GetLifecyclePolicy|4xx->4xx|$.ResourceType
Issue-Key: dlm|DeleteLifecyclePolicy|4xx->4xx|$.Code
Issue-Key: dlm|DeleteLifecyclePolicy|4xx->4xx|$.Message
Issue-Key: dlm|DeleteLifecyclePolicy|4xx->4xx|$.ResourceType
Issue-Key: dlm|UpdateLifecyclePolicy|4xx->4xx|$.Code
Issue-Key: dlm|UpdateLifecyclePolicy|4xx->4xx|$.Message
Issue-Key: dlm|UpdateLifecyclePolicy|4xx->4xx|$.ResourceType
Issue-Key: dlm|UpdateLifecyclePolicy|4xx->2xx|$
Issue-Key: dlm|UpdateLifecyclePolicy|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: dlm|UpdateLifecyclePolicy|4xx->2xx|status
Issue-Key: dlm|TagResource|4xx->2xx|$
Issue-Key: dlm|TagResource|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: dlm|TagResource|4xx->2xx|status
Issue-Key: dlm|TagResource|2xx->2xx|status
Issue-Commit: a95ddc9c00c4f94c9371e90406864e89cbfb6e44

# DLM error paths skip validation and omit modeled exception fields

> _Note: this issue was identified against the code base at commit `a95ddc9c00c4f94c9371e90406864e89cbfb6e44`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateLifecyclePolicy

## Description
DLM's error paths diverge in two linked ways: requests AWS rejects are accepted with 200, and the rejections the emulator does produce are not shaped like DLM's modeled exceptions (no Code member, wrong ResourceType, wrong x-amzn-errortype). There are 8 distinct causes in this bundle; all must be closed.

Cause 1 of 8 - CreateLifecyclePolicy accepts a schedule with both Interval and CronExpression (representative cause).
AWS: Smithy CreateLifecyclePolicy models error InvalidRequestException (errorKind client, httpError 400) whose members are Message, Code, RequiredParameters and MutuallyExclusiveParameters; com.amazonaws.dlm#CreateRule documents that you must specify EITHER CronExpression OR Interval+IntervalUnit+Times. The recording shows 400 with Code 'InvalidParameter', x-amzn-errortype 'InvalidRequestException' and a message built from the request naming the offending schedule ('Interval based CreateRule and Cron based CreateRule cannot both be present in schedule {BadSchedule}.' - the rule is "name the schedule that carries both"; the exact text is an example).
Emulator: answers 200 and creates the policy; the response body carries no Code/Message and no x-amzn-errortype header.
Why: a client's negative test passes where AWS fails, and the emulator then holds a policy AWS would never have created, so every later read of that policy is wrong too.
Fix: CreateLifecyclePolicy input validation (the CreateRule mutual-exclusion check) raising InvalidRequestException.
Traces: dlm-create-policy-invalid-request-errors#task#1.

Cause 2 of 8 - the 400 for an invalid Description is not an InvalidRequestException.
AWS: Smithy CreateLifecyclePolicyRequest.Description targets PolicyDescription with pattern ^[0-9A-Za-z _-]+$, and the modeled rejection is InvalidRequestException (400) carrying Code and Message; the message rule is "name the invalid parameter(s)" - recorded example 'The following parameter(s) are invalid: Description {invalid!description#chars}'. The error type on the wire must be a modeled DLM exception.
Emulator: returns the right status (400) but an empty-ish body with no $.Code and no $.Message, and advertises x-amzn-errortype 'ValidationException' - a shape DLM does not model at all.
Why: SDK clients map errors by x-amzn-errortype, so this never deserializes to DlmInvalidRequestException; catch-blocks and retry/branching on InvalidRequestException do not fire, and error.code is empty.
Fix: the DLM error serializer (errortype header + Code/Message members) for the CreateLifecyclePolicy validation path.
Traces: dlm-create-policy-invalid-description-characters-rejected#task#3 (cluster also covers dlm-create-policy-invalid-request-errors#task#2).

Cause 3 of 8 - GetLifecyclePolicy 404 body is not a ResourceNotFoundException.
AWS: Smithy com.amazonaws.dlm#ResourceNotFoundException (httpError 404) has members Message, Code, ResourceType and ResourceIds. The recording shows Code 'ResourceNotFound', ResourceType 'LIFECYCLE_POLICY' and a message of the documented form 'The resource {policy-...} was not found.'
Emulator: 404 with no $.Code at all, ResourceType 'policy' (an internal lowercase token, not the LIFECYCLE_POLICY wire value) and a message of its own invention ('Policy policy-0000000000000001 could not be found.').
Why: the status is right but the payload is not the modeled shape - clients that switch on Code, or on ResourceType to distinguish a missing policy from another missing DLM resource, get nothing usable. (ResourceType is a modeled enum-like wire value here, not free text, so 'policy' is a defect, unlike the message text which is an example.)
Fix: the ResourceNotFoundException construction in DLM's error mapping/serializer - populate Code and emit the LIFECYCLE_POLICY resource-type token.
Traces: dlm-resource-not-found-errors#task#4.

Cause 4 of 8 - DeleteLifecyclePolicy 404 has the same malformed body.
AWS: DeleteLifecyclePolicy models ResourceNotFoundException (404) with Code/ResourceType as above.
Emulator: 404, $.Code missing, ResourceType 'policy'.
Why/Fix: as Cause 3 - confirms the defect is in the shared error mapping rather than one handler, so fixing it must be verified on the DELETE path too.
Traces: dlm-resource-not-found-errors#task#5.

Cause 5 of 8 - UpdateLifecyclePolicy 404 has the same malformed body.
AWS: UpdateLifecyclePolicy (PATCH /policies/{PolicyId}) models ResourceNotFoundException (404) with Code/ResourceType as above.
Emulator: 404, $.Code missing, ResourceType 'policy'.
Why/Fix: as Cause 3, on the PATCH path.
Traces: dlm-update-policy-error-paths#task#5.

Cause 6 of 8 - UpdateLifecyclePolicy accepts a schedule with both Interval and CronExpression, and persists it.
AWS: UpdateLifecyclePolicy models InvalidRequestException (400); the same CreateRule mutual-exclusion rule applies on update. The recording shows 400 with Code 'InvalidParameter', RequiredParameters/MutuallyExclusiveParameters null and a message naming the schedule ('... in schedule {BadUpdateSched}.').
Emulator: 200 with an empty body and no x-amzn-errortype, AND the invalid schedule replaces the stored one - the subsequent read (dlm-update-policy-error-paths#task#8) returns Schedules[0].Name 'BadUpdateSched' with both Interval and CronExpression instead of the preserved 'UpdateErrSched'.
Why: a rejected update is applied, so the emulator's state silently diverges from what AWS would hold; a client that handles the 400 and retries with a corrected body ends up with the bad schedule already live.
Fix: UpdateLifecyclePolicy input validation, which must run and reject BEFORE the policy state is mutated.
Traces: dlm-update-policy-error-paths#task#6 (state consequence visible at #task#8).

Cause 7 of 8 - TagResource accepts a reserved 'aws:' tag key.
AWS: TagResource models InvalidRequestException (400); AWS reserves the aws: tag-key prefix (case-insensitive) and rejects it, recorded as 400 with body {'message': 'Invalid tag key. Customers cannot set tag keys starting with aws: (case insensitive)'} and an error-type header.
Emulator: 200 with an empty body and no x-amzn-errortype, so the reserved tag is applied.
Why: callers can create tags AWS forbids, and anything built on the invariant "aws:* tags are service-owned" (cost allocation, policy conditions) is wrong against the emulator.
Fix: TagResource tag-key validation in the DLM tagging handler.
Traces: dlm-tag-resource-reserved-aws-prefix-key-rejected#task#3.

Cause 8 of 8 - successful TagResource answers 200 where the wire shows 204.
AWS: recorded behavior only, and the model disagrees - Smithy TagResource has an empty output shape (TagResourceResponse, no members) with http code 200, while the recording for the successful tag call shows 204 No Content. Treat the recording as the wire truth (an empty body with a 200 @http code is exactly the case where the service returns 204) and verify against the fixture; this is the lowest-confidence cause in the bundle.
Emulator: 200.
Why: a client asserting on the documented empty-success status sees a different one; no body is involved either way, so impact is limited to status assertions.
Fix: the TagResource handler's success status code.
Traces: dlm-tag-resource-reserved-aws-prefix-key-rejected#task#5.

No open issue covers any of these (list_existing_issues for 'dlm' returned none). The related IAM DeleteRole 409 seen in dlm-get-lifecycle-policies-filter-by-target-tags#teardown#9 is already owned by 20261022T092913Z-iam-deleterole-answers-409-for-a-role-aws.md and is deliberately not part of this issue.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator (emulator_up true, bootstrap_signal false, state_reset_failures 0; score 72.7%, 21 mismatched steps).

dlm-create-policy-invalid-request-errors#task#1 CreateLifecyclePolicy - expected 400, got 200:
  $.Code: expected 'InvalidParameter', got None
  $.Message: expected 'Interval based CreateRule and Cron based CreateRule cannot both be present in schedule {BadSchedule}.', got None
  $headers.x-amzn-errortype: expected 'InvalidRequestException', got None

dlm-create-policy-invalid-description-characters-rejected#task#3 CreateLifecyclePolicy - expected 400, got 400:
  $.Code: expected 'InvalidParameter', got None
  $.Message: expected 'The following parameter(s) are invalid: Description {invalid!description#chars}', got None
  $headers.x-amzn-errortype: got 'ValidationException' (not a modeled DLM error shape)
  (cluster also covers dlm-create-policy-invalid-request-errors#task#2)

dlm-resource-not-found-errors#task#4 GetLifecyclePolicy - 404/404:
  $.Code: expected 'ResourceNotFound', got None
  $.ResourceType: expected 'LIFECYCLE_POLICY', got 'policy'
  $.Message: expected 'The resource {policy-0000000000000001} was not found.', got 'Policy policy-0000000000000001 could not be found.'

dlm-resource-not-found-errors#task#5 DeleteLifecyclePolicy - 404/404: identical Code/ResourceType divergence

dlm-update-policy-error-paths#task#5 UpdateLifecyclePolicy - 404/404: identical Code/ResourceType divergence (policy-0000000000000002)

dlm-update-policy-error-paths#task#6 UpdateLifecyclePolicy - expected 400, got 200:
  $: empty_body (expected {'Code': 'InvalidParameter', 'RequiredParameters': None, 'MutuallyExclusiveParameters': None, 'Message': 'Interval based CreateRule and Cron based CreateRule cannot both be present in schedule {BadUpdateSched}.'}, got {})
  $headers.x-amzn-errortype: missing
  State consequence at #task#8: Schedules[0] = {'Name': 'BadUpdateSched', 'CreateRule': {'Interval': 24, 'IntervalUnit': 'HOURS', 'CronExpression': 'cron(0 12 * * ? *)'}, ...} instead of the stored 'UpdateErrSched'

dlm-tag-resource-reserved-aws-prefix-key-rejected#task#3 TagResource - expected 400, got 200:
  $: empty_body (expected {'message': 'Invalid tag key. Customers cannot set tag keys starting with aws: (case insensitive)'}, got {})
  $headers.x-amzn-errortype: missing

dlm-tag-resource-reserved-aws-prefix-key-rejected#task#5 TagResource - expected 204, got 200 (status only)

Model citations: Smithy com.amazonaws.dlm#InvalidRequestException (client, httpError 400; members Message, Code, RequiredParameters, MutuallyExclusiveParameters); #ResourceNotFoundException (client, httpError 404; members Message, Code, ResourceType, ResourceIds); #CreateRule (CronExpression vs Interval/IntervalUnit/Times mutual exclusion); CreateLifecyclePolicyRequest.Description pattern ^[0-9A-Za-z _-]+$; TagResource errors InvalidRequestException/ResourceNotFoundException, output TagResourceResponse empty with @http code 200; protocol restJson1.

## Steps to Reproduce
1. Replay trace set simparity-wire-aws-v1 against the emulator.
2. Representative: fixture dlm-create-policy-invalid-request-errors, step task#1 - POST /policies with a schedule whose CreateRule carries BOTH Interval/IntervalUnit and CronExpression. Expect 400 + x-amzn-errortype InvalidRequestException + body Code 'InvalidParameter'; the emulator returns 200 and creates the policy.
3. dlm-create-policy-invalid-description-characters-rejected#task#3 - POST /policies with Description violating ^[0-9A-Za-z _-]+$: emulator returns 400 but with no Code/Message and x-amzn-errortype 'ValidationException'.
4. dlm-resource-not-found-errors#task#4 (GET /policies/policy-0000000000000001), #task#5 (DELETE same) and dlm-update-policy-error-paths#task#5 (PATCH policy-0000000000000002): each 404 must carry Code 'ResourceNotFound' and ResourceType 'LIFECYCLE_POLICY'.
5. dlm-update-policy-error-paths#task#6 - PATCH a policy with a CreateRule carrying both Interval and CronExpression: expect 400; the emulator returns 200 AND persists the schedule, visible at #task#8.
6. dlm-tag-resource-reserved-aws-prefix-key-rejected#task#3 - POST /tags/{ResourceArn} with a tag key starting 'aws:': expect 400; emulator returns 200. Then #task#5 - a valid tag call must answer 204, emulator answers 200.
7. All eight must pass before the issue is closed.
