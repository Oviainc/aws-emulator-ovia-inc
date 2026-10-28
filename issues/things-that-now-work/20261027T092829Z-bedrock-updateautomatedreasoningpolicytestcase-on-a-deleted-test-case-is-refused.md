Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bedrock.automated-reasoning-test-case-lifecycle
Issue-Shape-Id: com.amazonaws.bedrock#AmazonBedrockControlPlaneService
Issue-Key: bedrock.automated-reasoning-test-case-lifecycle|run#17|AssertionError|assert update_code == "ResourceNotFoundException", (
Issue-Commit: f3fa3b136afa9998cb8eb67e7cd37cdcb81775a2

# UpdateAutomatedReasoningPolicyTestCase on a deleted test case is refused 400 ValidationException instead of 404 ResourceNotFoundException

> _Note: this issue was identified against the code base at commit `f3fa3b136afa9998cb8eb67e7cd37cdcb81775a2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateAutomatedReasoningPolicyTestCase

## Description
Cause 1 of 1: a mutation addressed to an Automated Reasoning policy test case that has already been deleted is misclassified as a malformed request instead of a missing resource.

Failing check ids: bedrock.automated-reasoning-test-case-lifecycle

AWS: Smithy (services/service-com-amazonaws-bedrock-amazonbedrockcontrolplaneservice/smithy-model/com-amazonaws-bedrock-amazonbedrockcontrolplaneservice.json) models UpdateAutomatedReasoningPolicyTestCase as PATCH /automated-reasoning-policies/{policyArn}/test-cases/{testCaseId}, with required members policyArn (@pattern ^arn:aws(-[^:]+)?:bedrock:[a-z0-9-]{1,20}:[0-9]{12}:automated-reasoning-policy/[a-z0-9]{12}(:([1-9][0-9]{0,11}))?$), testCaseId (@pattern ^[0-9A-Z]{12}$), guardContent, lastUpdatedAt and expectedAggregatedFindingsResult. Among its modeled errors are ResourceNotFoundException (HTTP 404, documented "The specified resource Amazon Resource Name (ARN) was not found. Check the Amazon Resource Name (ARN) and try your request again.") and ValidationException (HTTP 400, documented "Input validation failed. Check your request parameters and retry the request."). The modeled split is the requirement: a request that satisfies every modeled constraint but names an id that no longer resolves is a resource-identity failure, so it is ResourceNotFoundException/404; ValidationException/400 is reserved for a request whose parameters are themselves bad. The recorded behavior for this lifecycle is that, once DeleteAutomatedReasoningPolicyTestCase has succeeded, BOTH the read and the mutation on the dead id answer ResourceNotFoundException with the same message. Required message template (fixed text plus values taken from the request): "The TestCase with testCaseId: <testCaseId> and policyArn: <policyArn> does not exist" — the fixed text is "The TestCase with testCaseId: ", " and policyArn: " and " does not exist"; <testCaseId> and <policyArn> are the values the caller sent, never literals from any one run.

Emulator: GetAutomatedReasoningPolicyTestCase on the deleted id behaves correctly (ResourceNotFoundException with the message above — that assertion passes). The matching UpdateAutomatedReasoningPolicyTestCase call, built from the same policyArn, the same testCaseId and the lastUpdatedAt returned by the earlier Get, is refused with Error.Code 'ValidationException' instead. The replay fails at:

  assert update_code == "ResourceNotFoundException"
  AssertionError: expected ResourceNotFoundException updating a deleted test case, got 'ValidationException'

Note the refusal is NOT the scaffold's 501 NotImplemented: the request is rejected before it reaches services/service-com-amazonaws-bedrock-amazonbedrockcontrolplaneservice/src/ops/update_automated_reasoning_policy_test_case.rs:handle (which still returns Scaffolded::not_implemented), so the 400 comes from the request walk's validate stage ahead of the handler, not from the unimplemented body. The deleted-resource branch is therefore absent in BOTH places: the pre-handler refusal shadows it, and the handler has no resource lookup at all.

Why: a client that distinguishes the two error classes gets the wrong answer. ValidationException/400 tells a caller its parameters are malformed, so a retry/reconciliation path edits or re-derives the request, or treats it as a permanent client bug and alerts; ResourceNotFoundException/404 tells it the test case is simply gone, which is the normal, idempotent outcome of a delete that has already landed and is swallowed. Teardown and converge loops that key on ResourceNotFoundException (the harness's own _ignore_not_found pattern does exactly this) will instead propagate a hard failure. The accompanying message is also wrong for the class, so a caller cannot recover the distinction from the text either.

Fix: the operation's handler and its validation in services/service-com-amazonaws-bedrock-amazonbedrockcontrolplaneservice/src/ops/update_automated_reasoning_policy_test_case.rs, plus the test-case record lookup it must consult in that crate's state. The pre-handler refusal that currently answers ValidationException for a request meeting every modeled constraint must stop firing, and the handler must resolve (policyArn, testCaseId) against stored test cases and return ResourceNotFoundException/404 with the message template above when the record is absent — the same identity check GetAutomatedReasoningPolicyTestCase already performs correctly, so the two should share it. lastUpdatedAt remains the concurrency token and only becomes relevant (ConflictException) once the record exists.

Traces: bedrock.automated-reasoning-test-case-lifecycle — CreateAutomatedReasoningPolicy, CreateAutomatedReasoningPolicyTestCase, GetAutomatedReasoningPolicyTestCase, DeleteAutomatedReasoningPolicyTestCase, then Get (passes) and Update (fails) on the deleted id.

## Evidence
Check: bedrock.automated-reasoning-test-case-lifecycle (verdict: mismatch)

From the replay:
  File "check_bedrock_automated_reasoning_test_case_lifecycle.py", line 69, in run
    assert update_code == "ResourceNotFoundException", (
  AssertionError: expected ResourceNotFoundException updating a deleted test case, got 'ValidationException'

Expected (AWS, per the Smithy model's error set and the recorded lifecycle):
  HTTP 404
  Error.Code    = ResourceNotFoundException
  Error.Message = "The TestCase with testCaseId: <testCaseId> and policyArn: <policyArn> does not exist"
                  (fixed text: "The TestCase with testCaseId: ", " and policyArn: ", " does not exist";
                   <testCaseId> and <policyArn> are the request's own values)

Actual (emulator):
  HTTP 400
  Error.Code    = ValidationException

Corroborating evidence that this is a deleted-resource classification bug and not a bad request: the immediately preceding GetAutomatedReasoningPolicyTestCase on the SAME policyArn and testCaseId returns ResourceNotFoundException with exactly the expected message (that assertion passes), and the Update request reuses those identical values plus the lastUpdatedAt that Get returned before the delete. It is also not the scaffold 501 — the stub handler in update_automated_reasoning_policy_test_case.rs returns Scaffolded::not_implemented, so the observed 400 was produced upstream of the handler.

## Steps to Reproduce
1. CreateAutomatedReasoningPolicy with a minimal policyDefinition; keep the returned policyArn.
2. CreateAutomatedReasoningPolicyTestCase(policyArn=<arn>, guardContent=<any text>, expectedAggregatedFindingsResult="VALID"); keep testCaseId.
3. GetAutomatedReasoningPolicyTestCase(policyArn, testCaseId); keep testCase.updatedAt as lastUpdatedAt.
4. DeleteAutomatedReasoningPolicyTestCase(policyArn, testCaseId, lastUpdatedAt=<from step 3>).
5. Poll GetAutomatedReasoningPolicyTestCase(policyArn, testCaseId) until it is refused — observe ResourceNotFoundException with message "The TestCase with testCaseId: <testCaseId> and policyArn: <policyArn> does not exist" (correct today).
6. Call UpdateAutomatedReasoningPolicyTestCase(policyArn, testCaseId, guardContent=<any text>, lastUpdatedAt=<from step 3>, expectedAggregatedFindingsResult="VALID").
   Observed: HTTP 400, Error.Code 'ValidationException'.
   Expected: HTTP 404, Error.Code 'ResourceNotFoundException' with the same message as step 5.
