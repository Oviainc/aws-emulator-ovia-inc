Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: compute-optimizer-automation.start-automation-event-nonexistent-action-not-found compute-optimizer-automation.untag-resource-invalid-key-rejected compute-optimizer-automation.get-automation-rule-malformed-arn-rejected
Issue-Shape-Id: com.amazonaws.computeoptimizerautomation#ComputeOptimizerAutomationService
Issue-Key: compute-optimizer-automation.start-automation-event-nonexistent-action-not-found|run#5|AssertionError|assert code == "ResourceNotFoundException", (
Issue-Key: compute-optimizer-automation.untag-resource-invalid-key-rejected|run#10|AssertionError|assert code == "InvalidParameterValueException", (
Issue-Key: compute-optimizer-automation.get-automation-rule-malformed-arn-rejected|run#10|AssertionError|assert code == "AccessDeniedException", f"unexpected code: {code!r}"
Issue-Commit: 668d747e62a7a896ef2863c29b158e984b097a40

# Compute Optimizer Automation: StartAutomationEvent and UntagResource answer 501 NotImplemented, and a malformed ruleArn is rejected as InvalidParameterValueException instead of AccessDeniedException

> _Note: this issue was identified against the code base at commit `668d747e62a7a896ef2863c29b158e984b097a40`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartAutomationEvent

## Description
Three independent root causes in the compute-optimizer-automation emulator. All three are reachable with the service's modeled, documented request shapes; the service IS serving (other operations in this suite answer with real state), so these are fidelity gaps, not a bootstrap gap.

Cause 1 of 3: StartAutomationEvent is a scaffold stub and answers 501 NotImplemented.
AWS: StartAutomationEvent is a modeled operation of ComputeOptimizerAutomationService (Smithy: StartAutomationEvent, input StartAutomationEventRequest, errors IdempotencyTokenInUseException, IdempotentParameterMismatchException, ResourceNotFoundException, ServiceQuotaExceededException; plus the service-common AccessDeniedException / InvalidParameterValueException / ThrottlingException). For a well-formed but nonexistent recommendedActionId the recorded behavior is a rejection with error code ResourceNotFoundException (Smithy: ResourceNotFoundException @httpError 404, doc "The specified resource was not found.").
Emulator: returns HTTP 501 with error code NotImplemented and message "operation StartAutomationEvent is scaffolded but not implemented in this emulator".
Why: a client that starts a one-time on-demand automation can neither drive the happy path nor distinguish a missing recommended action (a retriable/user-fixable 404) from a server fault; 501/NotImplemented is not a modeled error of this operation, so SDK error handling and retry classification both go wrong.
Fix: implement the operation handler (its state lookup of the recommended action and its ResourceNotFoundException branch). Traces below cite the file.
Traces: check compute-optimizer-automation.start-automation-event-nonexistent-action-not-found; handler services/service-com-amazonaws-computeoptimizerautomation-computeoptimizerautomationservice/src/ops/start_automation_event.rs (SCAFFOLDED marker at line 4; the stub Err(...not_implemented) in Operation::handle). NOTE: this check is NOT one of the suite's "ALREADY FILED" stub clusters — the stub reply was caught by the check as a code mismatch, so no issue exists for StartAutomationEvent.

Cause 2 of 3: UntagResource is a scaffold stub and answers 501 NotImplemented.
AWS: UntagResource is modeled (Smithy: UntagResourceRequest.resourceArn RuleArn @required pattern ^arn:aws:compute-optimizer::[0-9]{12}:automation-rule/[a-zA-Z0-9_-]+$, ruleRevision Long @required, tagKeys TagKeyList @required; errors IdempotencyTokenInUseException, IdempotentParameterMismatchException, ResourceNotFoundException + common InvalidParameterValueException @httpError 400). Recorded behavior for a tagKeys entry that violates the AWS tag-key form ("not a valid key!") is a 400 rejection with error code InvalidParameterValueException and the tags left intact (recorded message example: "Tag validation failed").
Emulator: returns HTTP 501 with error code NotImplemented for every UntagResource call, valid or invalid.
Why: tags cannot be removed at all, and a malformed tag key is reported as a server-side gap rather than a client validation failure — a client cannot tell "fix your key" from "service broken", and tag-lifecycle code paths (create-tag-untag) cannot run.
Fix: implement the operation handler, including tag-key validation (reject with InvalidParameterValueException), the ruleRevision optimistic-lock check and the state mutation that removes the keys.
Traces: check compute-optimizer-automation.untag-resource-invalid-key-rejected; handler services/service-com-amazonaws-computeoptimizerautomation-computeoptimizerautomationservice/src/ops/untag_resource.rs (SCAFFOLDED marker at line 4; Err(crate::scaffolded::Scaffolded::not_implemented("UntagResource")) at line 179).

Cause 3 of 3: GetAutomationRule with a ruleArn that is not an ARN at all returns InvalidParameterValueException (400) where AWS returns AccessDeniedException (403).
AWS: recorded behavior for GetAutomationRule(ruleArn="not-an-arn") is error code AccessDeniedException (Smithy: AccessDeniedException @httpError 403, a service-common error of this operation, doc "You do not have sufficient permissions to perform this action."), because authorization is evaluated against the supplied resource string before the service parses it; the recorded message is of the standard IAM form "User: <caller-arn> is not authorized to perform: aco-automation:GetAutomationRule on resource: not-an-arn" (that text embeds the caller's account-specific ARN and is only an example — the requirement is the 403/AccessDeniedException classification).
Emulator: rejects the request with error code InvalidParameterValueException and HTTP 400, produced by the framework's modeled @pattern check on RuleArn before the handler runs; GetAutomationRule's handler has no ARN/authorization branch at all.
Why: a client (or a permissions-probing tool) that distinguishes "not allowed" from "bad input" mis-classifies the failure: 400 is treated as a request bug to fix locally, while AWS's 403 means the principal lacks aco-automation:GetAutomationRule on that resource. Error-handling and alarming on AccessDenied never fires.
Fix: GetAutomationRule's handler/validation ordering — an unparseable resource identifier must be answered with the modeled AccessDeniedException (403) rather than falling to the generic modeled-pattern rejection.
Traces: check compute-optimizer-automation.get-automation-rule-malformed-arn-rejected; handler services/service-com-amazonaws-computeoptimizerautomation-computeoptimizerautomationservice/src/ops/get_automation_rule.rs (Operation::handle, lines 179-205 — no ARN-shape/authorization branch).

## Evidence
Behavioral suite (simparity-behavioral-aws-v1), compute-optimizer-automation 4/20 passed.

Cause 1 — compute-optimizer-automation.start-automation-event-nonexistent-action-not-found [mismatch]:
  expected: code == "ResourceNotFoundException"
  actual:   AssertionError: StartAutomationEvent(recommendedActionId='AAAAAAAAAAAAAAAA') on a nonexistent action should raise ResourceNotFoundException, got code='NotImplemented'

Cause 2 — compute-optimizer-automation.untag-resource-invalid-key-rejected [mismatch]:
  call: untag_resource(resourceArn=<rule arn>, ruleRevision=<current>, tagKeys=["not a valid key!"])
  expected: code == "InvalidParameterValueException" (HTTP 400), existing tag {"key":"team","value":"sim2real"} intact
  actual:   AssertionError: expected InvalidParameterValueException for a malformed tag key, got 'NotImplemented' (HTTP 501)

Cause 3 — compute-optimizer-automation.get-automation-rule-malformed-arn-rejected [mismatch]:
  call: get_automation_rule(ruleArn="not-an-arn") (positive control on a real rule passed first)
  expected: code == "AccessDeniedException" (HTTP 403)
  actual:   AssertionError: unexpected code: 'InvalidParameterValueException' (HTTP 400)

## Steps to Reproduce
1. Boot the emulator and point a botocore client for service 'compute-optimizer-automation' at it.
2. Enroll: compute-optimizer UpdateEnrollmentStatus(status=Active), then compute-optimizer-automation UpdateEnrollmentConfiguration(status=Active) until GetEnrollmentConfiguration reports Active.
3. Cause 3: create_automation_rule(name=<valid>, ruleType=AccountRule, status=Inactive, recommendedActionTypes=[SnapshotAndDeleteUnattachedEbsVolume], schedule=<any>), confirm get_automation_rule(ruleArn=<arn>) succeeds, then call get_automation_rule(ruleArn="not-an-arn") -> observe InvalidParameterValueException/400 instead of AccessDeniedException/403.
4. Cause 2: create a rule with tags=[{"key":"team","value":"sim2real"}], then untag_resource(resourceArn=<arn>, ruleRevision=<created revision>, tagKeys=["not a valid key!"]) -> observe NotImplemented/501 instead of InvalidParameterValueException/400.
5. Cause 1: start_automation_event(recommendedActionId="AAAAAAAAAAAAAAAA") -> observe NotImplemented/501 instead of ResourceNotFoundException/404.

Replay: run_behavioral_suite and re-check the three ids in trace_ids.
