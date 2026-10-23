Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: compute-optimizer-automation.delete-automation-rule-stale-revision-rejected compute-optimizer-automation.get-automation-rule-malformed-arn-rejected
Issue-Shape-Id: com.amazonaws.computeoptimizerautomation#ComputeOptimizerAutomationService
Issue-Key: compute-optimizer-automation.delete-automation-rule-stale-revision-rejected|run#25|AssertionError|assert mutate_message == expected_mutate_message, (
Issue-Key: compute-optimizer-automation.get-automation-rule-malformed-arn-rejected|run#13|AssertionError|assert message == expected_message, f"unexpected malformed-ARN message: {message!r}"
Issue-Commit: a59989a9f63c403b66485a4e93f54ab6df22c2a6

# compute-optimizer-automation: error messages not built from the request — generic not-found text on UpdateAutomationRule, generic access-denied text for a malformed ruleArn

> _Note: this issue was identified against the code base at commit `a59989a9f63c403b66485a4e93f54ab6df22c2a6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateAutomationRule

## Description
The Compute Optimizer Automation emulator returns the right error CODES for two rejection classes but emits fixed, canned message text where AWS builds the message from the request. Both failing checks assert on message content that is derivable from the request (the ruleArn the caller passed, the caller identity and the IAM action), so the message is a requirement here, not an incidental recording value. Two distinct root causes, in one service:

Cause 1 of 2 — UpdateAutomationRule on a deleted/unknown rule returns the generic resource-not-found text instead of a message naming the rule ARN.
AWS: Smithy (com-amazonaws-computeoptimizerautomation-computeoptimizerautomationservice.json): UpdateAutomationRule input member ruleArn (required, pattern ^arn:aws:compute-optimizer::[0-9]{12}:automation-rule/[a-zA-Z0-9_-]+$) and modeled error ResourceNotFoundException (client, httpError 404). Recorded behavior: a mutation (UpdateAutomationRule) against an ARN that no longer exists is rejected 404 ResourceNotFoundException with a message built from the request ARN — of the form 'Rule with ARN <ruleArn> does not exist' (example: 'Rule with ARN arn:aws:compute-optimizer::000000000000:automation-rule/42d39f82e52fa35c does not exist'). The read path (GetAutomationRule) in the same recording uses the generic 'The specified resource could not be found', and the emulator matches AWS there — so AWS distinguishes the two, and the mutation path must name the ARN it was given.
Emulator: UpdateAutomationRule on the deleted rule returns ResourceNotFoundException (correct code) with the generic read-path text 'The specified resource could not be found'.
Why: a client cannot tell WHICH rule ARN a batched or retried mutation failed on. Callers and tooling that key remediation/logging off the ARN embedded in the not-found message (common in Compute Optimizer automation rule reconcilers) see no ARN at all, and sim-vs-real assertions on the mutation rejection diverge.
Fix: UpdateAutomationRule's handler / its not-found error construction — the mutation path must format the not-found message from the requested ruleArn rather than reusing the shared generic not-found constant used by the read path (services/service-com-amazonaws-computeoptimizerautomation-computeoptimizerautomationservice/src/ops/update_automation_rule.rs and the shared error helper it calls).
Traces: compute-optimizer-automation.delete-automation-rule-stale-revision-rejected (all earlier assertions in that check pass: the stale-revision DeleteAutomationRule is correctly rejected InvalidParameterValueException with the current-revision message, the rule survives untouched, the correct-revision delete succeeds, and the post-delete GetAutomationRule returns the generic not-found text as AWS does; only the final UpdateAutomationRule message diverges).

Cause 2 of 2 — GetAutomationRule with a ruleArn that is not an ARN returns a canned access-denied sentence instead of the authorization message naming the caller, the action and the resource.
AWS: Smithy: GetAutomationRule input ruleArn (required, pattern ^arn:aws:compute-optimizer::[0-9]{12}:automation-rule/[a-zA-Z0-9_-]+$), modeled errors include AccessDeniedException (client, httpError 403) and InvalidParameterValueException (client, 400). Recorded behavior: a value that does not match the automation-rule ARN pattern at all fails authorization/resource-pattern matching BEFORE resource lookup and is rejected 403 AccessDeniedException (not InvalidParameterValueException) with the standard IAM denial message assembled from the request: 'User: <caller ARN from the signing credential> is not authorized to perform: aco-automation:GetAutomationRule on resource: <the ruleArn exactly as sent>' (example resource: 'not-an-arn'). Every component of that message comes from the request or the operation's IAM action name, so the construction rule is known.
Emulator: returns AccessDeniedException with HTTP 403 (correct code/status) but the fixed text 'You do not have sufficient permissions to perform this action.' — it names neither the caller, nor the action aco-automation:GetAutomationRule, nor the offending resource string.
Why: clients and operators diagnosing a denial rely on the action and resource embedded in the message to tell a malformed identifier apart from a genuinely missing IAM permission; with a constant sentence that signal is gone, and policy-debugging tooling that parses 'is not authorized to perform: <action> on resource: <arn>' finds nothing to parse.
Fix: GetAutomationRule's handler / its authorization-rejection path — build the AccessDeniedException message from the signing principal, the operation's IAM action (aco-automation:GetAutomationRule) and the supplied ruleArn, instead of returning the shared canned access-denied string (services/service-com-amazonaws-computeoptimizerautomation-computeoptimizerautomationservice/src/ops/get_automation_rule.rs and the shared access-denied helper).
Traces: compute-optimizer-automation.get-automation-rule-malformed-arn-rejected (the positive control — create then get a real rule — passes, and the error CODE is already AccessDeniedException; only the message diverges).

Note: the three other failures in this service's suite run (list-automation-rule-preview-structure, list-recommended-actions-and-summaries-structure, tag-untag-automation-rule-round-trip) are blocked by unimplemented scaffold stubs (ListAutomationRulePreviewSummaries, ListRecommendedActionSummaries, TagResource) and are already filed; they are deliberately excluded from this issue.

## Evidence
Suite: simparity-behavioral-aws-v1 — compute-optimizer-automation 15/20 (75%), 5 failed (3 of them already filed as unimplemented stubs).

Cause 1 — [mismatch] compute-optimizer-automation.delete-automation-rule-stale-revision-rejected
  check_compute_optimizer_automation_delete_automation_rule_stale_revision_rejected.py:93
  assert mutate_message == expected_mutate_message
  AssertionError: expected message 'Rule with ARN arn:aws:compute-optimizer::000000000000:automation-rule/42d39f82e52fa35c does not exist', got 'The specified resource could not be found'
  Expected (AWS, built from request): ResourceNotFoundException / 404, message 'Rule with ARN <ruleArn> does not exist'
  Actual (emulator):                 ResourceNotFoundException / 404, message 'The specified resource could not be found'
  Same check shows the emulator DOES match AWS on the GetAutomationRule not-found message ('The specified resource could not be found'), so the two paths are conflated.

Cause 2 — [mismatch] compute-optimizer-automation.get-automation-rule-malformed-arn-rejected
  check_compute_optimizer_automation_get_automation_rule_malformed_arn_rejected.py:60
  assert message == expected_message
  AssertionError: unexpected malformed-ARN message: 'You do not have sufficient permissions to perform this action.'
  Request: GetAutomationRule(ruleArn='not-an-arn')
  Expected (AWS, built from request): AccessDeniedException / 403, message 'User: <caller ARN> is not authorized to perform: aco-automation:GetAutomationRule on resource: not-an-arn'
  Actual (emulator):                  AccessDeniedException / 403, message 'You do not have sufficient permissions to perform this action.'

Smithy evidence (com-amazonaws-computeoptimizerautomation-computeoptimizerautomationservice.json): GetAutomationRule/UpdateAutomationRule/DeleteAutomationRule all take ruleArn with pattern ^arn:aws:compute-optimizer::[0-9]{12}:automation-rule/[a-zA-Z0-9_-]+$ and model both AccessDeniedException (403) and ResourceNotFoundException (404) and InvalidParameterValueException (400).

## Steps to Reproduce
Cause 1:
1. EnableAutomationEnrollment / wait for the account's Compute Optimizer Automation enrollment to reach Active.
2. r = CreateAutomationRule(name='sim2real-rule-<unique>', status='Inactive'); keep r.ruleArn and r.ruleRevision.
3. u = UpdateAutomationRule(ruleArn=r.ruleArn, ruleRevision=r.ruleRevision, description='bumped-before-delete-attempt') -> revision bumps.
4. DeleteAutomationRule(ruleArn=r.ruleArn, ruleRevision=u.ruleRevision) -> succeeds.
5. UpdateAutomationRule(ruleArn=r.ruleArn, ruleRevision=u.ruleRevision, description='should-never-apply').
   Observe: ResourceNotFoundException with message 'The specified resource could not be found'.
   Expected: ResourceNotFoundException with message 'Rule with ARN <r.ruleArn> does not exist'.

Cause 2:
1. Ensure enrollment Active as above; CreateAutomationRule and GetAutomationRule on the real ARN (positive control, succeeds).
2. GetAutomationRule(ruleArn='not-an-arn').
   Observe: 403 AccessDeniedException, message 'You do not have sufficient permissions to perform this action.'
   Expected: 403 AccessDeniedException, message 'User: <sts:GetCallerIdentity Arn> is not authorized to perform: aco-automation:GetAutomationRule on resource: not-an-arn'.

Replay: run_behavioral_suite and re-run the two check ids in trace_ids.
