Issue-Source: trace-sweep-divergence
Issue-Trace: bedrock-automated-reasoning-policy-lifecycle
Issue-Step: task#8
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: bedrock-automated-reasoning-policy-lifecycle bedrock-automated-reasoning-policy-test-case bedrock-guardrail-and-tag-error-paths bedrock-custom-model-read-and-errors
Issue-Shape-Id: com.amazonaws.bedrock#AmazonBedrockControlPlaneService
Issue-Key: bedrock|GetAutomatedReasoningPolicy|4xx->4xx|$.message
Issue-Key: bedrock|UpdateAutomatedReasoningPolicyTestCase|4xx->4xx|$.message
Issue-Key: bedrock|StartAutomatedReasoningPolicyTestWorkflow|4xx->4xx|$.message
Issue-Key: bedrock|GetAutomatedReasoningPolicyTestResult|4xx->4xx|$.message
Issue-Key: bedrock|DeleteAutomatedReasoningPolicyTestCase|4xx->4xx|$.message
Issue-Key: bedrock|ListTagsForResource|4xx->4xx|$.message
Issue-Key: bedrock|TagResource|4xx->4xx|$.message
Issue-Key: bedrock|CreateCustomModel|4xx->4xx|$.message
Issue-Commit: 95ee1f215215607c18ab877072afcef7ad3742b1

# bedrock not-found, conflict and validation refusals are worded by the emulator instead of AWS

> _Note: this issue was identified against the code base at commit `95ee1f215215607c18ab877072afcef7ad3742b1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetAutomatedReasoningPolicy

## Description
EIGHT causes, one shared fault: the HTTP status and the error code are right, but each refusal carries the emulator's own prose rather than AWS's message for that error class. AWS's messages here are entirely fixed text (or fixed text plus ids taken from the request), so they are a requirement, not a per-run value; the emulator's versions are longer, differently cased, and in three causes explain internal state (timestamps, ARNs) AWS never mentions.

Cause 1 of 8: GetAutomatedReasoningPolicy for a policy ARN that does not exist.
AWS: 404 ResourceNotFoundException, message 'Policy does not exist' (fixed text, no variable part). Smithy: GetAutomatedReasoningPolicy models ResourceNotFoundException httpError 404.
Emulator: 404 ResourceNotFoundException but message 'no automated reasoning policy <policyArn> exists in this account'.
Why: callers that match AWS's message never match; the emulator also echoes the ARN AWS omits.
Fix: the ResourceNotFoundException raised by the automated-reasoning-policy read handler.
Traces: bedrock-automated-reasoning-policy-lifecycle#task#8.

Cause 2 of 8: UpdateAutomatedReasoningPolicyTestCase with a stale lastUpdatedAt.
AWS: 400 ConflictException. Message template: 'Conflict with updating TestCase with testCaseId: <testCaseId> and policyArn: <policyArn>'. Fixed text: 'Conflict with updating TestCase with testCaseId: ' and ' and policyArn: '; variable: the testCaseId and policyArn from the request. Smithy: UpdateAutomatedReasoningPolicyTestCase requires lastUpdatedAt (@timestampFormat date-time) and models ConflictException httpError 400.
Emulator: 400 with 'test <testCaseId> of automated reasoning policy <policyArn> was last updated at <stored updatedAt>, not at <supplied lastUpdatedAt>; read the test again and retry the update with the updatedAt that read reports' — emulator prose that additionally leaks the stored timestamp.
Why: optimistic-concurrency handling keyed on AWS's conflict message breaks; the message exposes internal state AWS does not return.
Fix: the ConflictException raised by UpdateAutomatedReasoningPolicyTestCase's concurrency check.
Traces: bedrock-automated-reasoning-policy-test-case#task#5.

Cause 3 of 8: StartAutomatedReasoningPolicyTestWorkflow when the referenced build workflow does not exist.
AWS: 404 ResourceNotFoundException, message 'Build workflow not found' (fixed text).
Emulator: 404 with 'Build WorkflowExecution not found' — an internal type name ('WorkflowExecution') in place of AWS's words.
Why: message assertions fail and the text exposes an implementation type.
Fix: the not-found message in the test-workflow handler's build-workflow lookup.
Traces: bedrock-automated-reasoning-policy-test-case#task#6.

Cause 4 of 8: GetAutomatedReasoningPolicyTestResult when the referenced build workflow does not exist.
AWS: 404 ResourceNotFoundException, message 'Build workflow not found' (fixed text).
Emulator: 404 with 'Build WorkflowExecution not found'.
Why: as cause 3.
Fix: same shared lookup/message.
Traces: bedrock-automated-reasoning-policy-test-case#task#8.

Cause 5 of 8: DeleteAutomatedReasoningPolicyTestCase with a stale lastUpdatedAt.
AWS: 400 ConflictException. Message template: 'Conflict with deleting TestCase with testCaseId: <testCaseId> and policyArn: <policyArn>'. Fixed text: 'Conflict with deleting TestCase with testCaseId: ' and ' and policyArn: '; variable: the two ids from the request.
Emulator: 400 with 'test <testCaseId> of automated reasoning policy <policyArn> was last updated at <stored updatedAt>, not at <supplied lastUpdatedAt>; read the test again and retry the delete with the updatedAt that read reports'.
Why: as cause 2.
Fix: the ConflictException raised by DeleteAutomatedReasoningPolicyTestCase's concurrency check.
Traces: bedrock-automated-reasoning-policy-test-case#task#11.

Cause 6 of 8: ListTagsForResource for a guardrail ARN that does not exist.
AWS: 404 ResourceNotFoundException, message 'Guardrail not found' (fixed text, no ARN echoed). Smithy: ListTagsForResource resourceARN is pattern-constrained and includes the ':guardrail/[a-z0-9]+' form; ResourceNotFoundException httpError 404 is modelled.
Emulator: 404 with 'no resource <resourceARN> exists in this account' — a generic resource-agnostic message that neither names the resource type AWS names nor omits the ARN AWS omits.
Why: callers cannot tell a missing guardrail from any other missing resource, and message assertions fail.
Fix: the tagging handlers' not-found path — select the per-resource-type message from the ARN's resource segment.
Traces: bedrock-guardrail-and-tag-error-paths#task#1.

Cause 7 of 8: TagResource for a guardrail ARN that does not exist.
AWS: 404 ResourceNotFoundException, message 'Guardrail not found' (fixed text).
Emulator: 404 with 'no resource <resourceARN> exists in this account'.
Why: as cause 6.
Fix: same shared tagging not-found path.
Traces: bedrock-guardrail-and-tag-error-paths#task#2.

Cause 8 of 8: CreateCustomModel with a roleArn that is not usable for the request's modelSourceConfig.
AWS: 400 ValidationException, message 'The provided role ARN is invalid.' (fixed text). Smithy: CreateCustomModel roleArn is optional with pattern ^arn:aws(-[^:]+)?:iam::([0-9]{12})?:role/.+$; ValidationException httpError 400 is modelled; success code is 202.
Emulator: 400 with 'roleArn is required when the model artifacts are named by modelSourceConfig' — the emulator's own requiredness rule and wording in place of AWS's invalid-role-ARN message.
Why: the caller is told the member is missing when AWS says it is invalid, so the remediation (supply a valid role) is mis-signalled; message assertions fail.
Fix: CreateCustomModel's roleArn validation message.
Traces: bedrock-custom-model-read-and-errors#task#3.

Related open issues that do NOT cover these steps (no replay key for them): 20261029T172312Z-bedrock-control-plane-seven-refusals-are-worded-by.md and 20261026T233704Z-bedrock-batchdeleteevaluationjob-s-non-terminal-status-refusal-uses.md.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from mainline. In all eight the status and x-amzn-errortype matched; only $.message diverged.

Cause 1 — bedrock-automated-reasoning-policy-lifecycle#task#8 GetAutomatedReasoningPolicy (404/404): expected 'Policy does not exist', got 'no automated reasoning policy <policyArn> exists in this account'.

Cause 2 — bedrock-automated-reasoning-policy-test-case#task#5 UpdateAutomatedReasoningPolicyTestCase (400/400): expected 'Conflict with updating TestCase with testCaseId: <id> and policyArn: <arn>', got 'test <id> of automated reasoning policy <arn> was last updated at <ts>, not at <ts>; read the test again and retry the update with the updatedAt that read reports'.

Cause 3 — bedrock-automated-reasoning-policy-test-case#task#6 StartAutomatedReasoningPolicyTestWorkflow (404/404): expected 'Build workflow not found', got 'Build WorkflowExecution not found'.

Cause 4 — bedrock-automated-reasoning-policy-test-case#task#8 GetAutomatedReasoningPolicyTestResult (404/404): expected 'Build workflow not found', got 'Build WorkflowExecution not found'.

Cause 5 — bedrock-automated-reasoning-policy-test-case#task#11 DeleteAutomatedReasoningPolicyTestCase (400/400): expected 'Conflict with deleting TestCase with testCaseId: <id> and policyArn: <arn>', got 'test <id> of automated reasoning policy <arn> was last updated at <ts>, not at <ts>; read the test again and retry the delete with the updatedAt that read reports'.

Cause 6 — bedrock-guardrail-and-tag-error-paths#task#1 ListTagsForResource (404/404): expected 'Guardrail not found', got 'no resource <guardrail arn> exists in this account'.

Cause 7 — bedrock-guardrail-and-tag-error-paths#task#2 TagResource (404/404): expected 'Guardrail not found', got 'no resource <guardrail arn> exists in this account'.

Cause 8 — bedrock-custom-model-read-and-errors#task#3 CreateCustomModel (400/400): expected 'The provided role ARN is invalid.', got 'roleArn is required when the model artifacts are named by modelSourceConfig'.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it.
2. Replay bedrock-automated-reasoning-policy-lifecycle#task#8; bedrock-automated-reasoning-policy-test-case#task#5, #task#6, #task#8 and #task#11; bedrock-guardrail-and-tag-error-paths#task#1 and #task#2; bedrock-custom-model-read-and-errors#task#3.
3. Compare $.message of each refusal against the recorded AWS text.
4. Equivalently: GetAutomatedReasoningPolicy on a nonexistent policy ARN; Update/DeleteAutomatedReasoningPolicyTestCase with a stale lastUpdatedAt; Start...TestWorkflow and GetAutomatedReasoningPolicyTestResult naming a nonexistent build workflow; ListTagsForResource/TagResource on a nonexistent guardrail ARN; CreateCustomModel with an unusable roleArn.
