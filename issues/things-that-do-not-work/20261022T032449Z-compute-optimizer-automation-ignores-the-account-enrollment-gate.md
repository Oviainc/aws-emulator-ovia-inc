Issue-Source: trace-sweep-divergence
Issue-Trace: compute-optimizer-automation-automation-event-error-paths
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: compute-optimizer-automation-automation-event-error-paths
Issue-Shape-Id: com.amazonaws.computeoptimizerautomation#ComputeOptimizerAutomationService
Issue-Key: compute-optimizer-automation|GetAutomationEvent|4xx->4xx|$.__type
Issue-Key: compute-optimizer-automation|GetAutomationEvent|4xx->4xx|$.message
Issue-Key: compute-optimizer-automation|GetAutomationEvent|4xx->4xx|status
Issue-Key: compute-optimizer-automation|ListAutomationEvents|4xx->2xx|$.__type
Issue-Key: compute-optimizer-automation|ListAutomationEvents|4xx->2xx|$.message
Issue-Key: compute-optimizer-automation|ListAutomationEvents|4xx->2xx|status
Issue-Key: compute-optimizer-automation|ListRecommendedActions|4xx->2xx|$.__type
Issue-Key: compute-optimizer-automation|ListRecommendedActions|4xx->2xx|$.message
Issue-Key: compute-optimizer-automation|ListRecommendedActions|4xx->2xx|status
Issue-Key: compute-optimizer-automation|ListAutomationRules|4xx->2xx|$.__type
Issue-Key: compute-optimizer-automation|ListAutomationRules|4xx->2xx|$.message
Issue-Key: compute-optimizer-automation|ListAutomationRules|4xx->2xx|status
Issue-Key: compute-optimizer-automation|ListTagsForResource|4xx->4xx|$.__type
Issue-Key: compute-optimizer-automation|ListTagsForResource|4xx->4xx|$.message
Issue-Key: compute-optimizer-automation|ListTagsForResource|4xx->4xx|status
Issue-Commit: f81229b87ab2500fc3bfee437baf8654766d98fe

# Compute Optimizer Automation ignores the account enrollment gate: five operations answer 404 or 200 instead of 403 OptInRequiredException

> _Note: this issue was identified against the code base at commit `f81229b87ab2500fc3bfee437baf8654766d98fe`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetAutomationEvent

## Description
This bundle has 5 causes, all one shared fault: the emulator has no notion of an account that is not enrolled (not opted in) to Compute Optimizer Automation, so the service-wide enrollment pre-check that AWS applies before any per-resource or per-filter logic is simply absent. Where AWS refuses the whole call with 403 OptInRequiredException, the emulator instead falls through to its normal resource lookup (answering 404 for the read/tag operations) or to its normal empty-list success path (answering 200 for the list operations). Each cause below is one operation that loses the gate; they are listed separately because each one is a distinct handler that must grow the pre-check, and a fixer must close all five.

Cause 1 of 5: GetAutomationEvent on a non-enrolled account.
AWS: OptInRequiredException is a service-COMMON error, bound to every operation of this service, and is modeled with httpError 403. Smithy: ComputeOptimizerAutomationService common_errors include OptInRequiredException; shape com.amazonaws.computeoptimizerautomation#OptInRequiredException errorKind=client httpError=403, documented "The account must be opted in to Compute Optimizer Automation before performing this action." GetAutomationEvent.errors also lists ResourceNotFoundException (404), but the enrollment check precedes resource resolution, so a non-enrolled account never reaches it. The recorded response is status 403 with __type 'com.amazonaws.computeoptimizerautomation#OptInRequiredException' (message, e.g. 'Aws account is not registered for recommendation.', is an example, not the requirement).
Emulator: status 404 with __type 'ResourceNotFoundException'.
Why: a client cannot distinguish "you must enroll this account first" from "that event id does not exist". The documented remediation for OptInRequiredException is to call UpdateEnrollmentConfiguration; on a 404 an SDK caller retries with different ids or treats the event as deleted, and onboarding code that branches on OptInRequiredException never fires. The 403/404 split also changes the SDK error class and its retry classification.
Fix: GetAutomationEvent's handler, ahead of event lookup — the enrollment state belongs in the service state alongside the enrollment configuration, with a shared pre-check used by every operation rather than per-handler logic.
Traces: compute-optimizer-automation-automation-event-error-paths#task#1.

Cause 2 of 5: ListAutomationEvents on a non-enrolled account.
AWS: same common error; Smithy ListAutomationEvents.errors includes OptInRequiredException (403). Note ListAutomationEvents models NO ResourceNotFoundException at all, so 403 is the only refusal available for this condition. Recorded: 403, __type 'com.amazonaws.computeoptimizerautomation#OptInRequiredException'.
Emulator: status 200 with no __type and no message — i.e. a successful (empty) listing.
Why: this is worse than a wrong error code: the call appears to SUCCEED. A client concludes the account is enrolled and has no automation events, so dashboards and enrollment-detection logic report a healthy, idle account instead of an un-onboarded one.
Fix: ListAutomationEvents' handler, same shared pre-check ahead of filter evaluation.
Traces: compute-optimizer-automation-automation-event-error-paths#task#3.

Cause 3 of 5: ListRecommendedActions on a non-enrolled account.
AWS: Smithy ListRecommendedActions.errors includes OptInRequiredException (403) and, like the other list operations, models no ResourceNotFoundException. Recorded: 403, __type 'com.amazonaws.computeoptimizerautomation#OptInRequiredException'.
Emulator: status 200 with no __type and no message.
Why: same false success. A caller enumerating recommended actions for a non-enrolled account is told there are none to take, which silently hides every recommendation the account would see once enrolled.
Fix: ListRecommendedActions' handler, same shared pre-check.
Traces: compute-optimizer-automation-automation-event-error-paths#task#5.

Cause 4 of 5: ListAutomationRules on a non-enrolled account.
AWS: Smithy ListAutomationRules.errors includes OptInRequiredException (403); no ResourceNotFoundException is modeled. Recorded: 403, __type 'com.amazonaws.computeoptimizerautomation#OptInRequiredException'.
Emulator: status 200 with no __type and no message.
Why: same false success, and here it is load-bearing for control flow — rule-management tooling that lists rules before creating one sees an empty but valid rule set and proceeds to CreateAutomationRule, which AWS would also refuse on enrollment grounds.
Fix: ListAutomationRules' handler, same shared pre-check.
Traces: compute-optimizer-automation-automation-event-error-paths#task#9.

Cause 5 of 5: ListTagsForResource on a non-enrolled account.
AWS: Smithy ListTagsForResource.errors includes both OptInRequiredException (403) and ResourceNotFoundException (404); the enrollment check precedes resolution of the rule ARN, so the non-enrolled account yields 403. Recorded: 403, __type 'com.amazonaws.computeoptimizerautomation#OptInRequiredException'.
Emulator: status 404 with __type 'ResourceNotFoundException'.
Why: as in cause 1, enrollment failure is reported as a missing automation rule, so tagging tooling retries or reports a bad ARN instead of surfacing the enrollment requirement.
Fix: ListTagsForResource's handler, same shared pre-check ahead of ARN resolution.
Traces: compute-optimizer-automation-automation-event-error-paths#task#10.

## Evidence
Replay of trace set simparity-wire-aws-v1 (3 fixtures, 23 steps, 6 passed, 17 mismatched; state_reset_failures 0, so these are not state-bleed artifacts).

Expected (recorded AWS) for all five steps: HTTP 403, body __type 'com.amazonaws.computeoptimizerautomation#OptInRequiredException', message e.g. 'Aws account is not registered for recommendation.' (message shown as an example).

Actual (emulator):
- task#1 GetAutomationEvent: 403 -> 404; $.__type 'com.amazonaws.computeoptimizerautomation#OptInRequiredException' -> 'ResourceNotFoundException'; $.message -> 'The specified resource could not be found'
- task#3 ListAutomationEvents: 403 -> 200; $.__type missing; $.message missing
- task#5 ListRecommendedActions: 403 -> 200; $.__type missing; $.message missing
- task#9 ListAutomationRules: 403 -> 200; $.__type missing; $.message missing
- task#10 ListTagsForResource: 403 -> 404; $.__type -> 'ResourceNotFoundException'; $.message -> 'The specified resource could not be found'

Model evidence: OptInRequiredException is listed in the service's common_errors and therefore bound to all 23 operations; its shape carries errorKind=client, httpError=403. The three list operations model no 404 error at all, which confirms 200 and 404 are both outside the modeled outcome set for this condition.

No divergence in this bundle carried a stub_path or an owned_by, so none is covered by an existing stub issue or an open issue's replay key. Two resolved issues touch this service's error payloads (20261022T000159Z-compute-optimizer-automation-error-payloads-diverge-from-aws.md, 20261022T000159Z-compute-optimizer-automation-startautomationevent-and-untagresource-answer-501.md); both are fixed and neither covers the enrollment gate.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay fixture compute-optimizer-automation-automation-event-error-paths from trace set simparity-wire-aws-v1.
2. The fixture drives the service with an account that has NOT been opted in to Compute Optimizer Automation.
3. Observe step task#1 (GetAutomationEvent): emulator answers 404 ResourceNotFoundException where the recording has 403 OptInRequiredException.
4. Observe steps task#3 (ListAutomationEvents), task#5 (ListRecommendedActions) and task#9 (ListAutomationRules): emulator answers 200 with a normal result body and no __type, where the recording has 403 OptInRequiredException.
5. Observe step task#10 (ListTagsForResource): emulator answers 404 ResourceNotFoundException where the recording has 403 OptInRequiredException.
6. Expected after fix: every one of the five steps returns HTTP 403 with __type 'com.amazonaws.computeoptimizerautomation#OptInRequiredException', with the enrollment check applied before any resource lookup or filter evaluation.
