Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bedrock.automated-reasoning-test-case-stale-token-rejected bedrock.delete-provisioned-throughput-missing-conflict-not-notfound bedrock.enforced-guardrail-configuration-unknown-guardrail-rejected bedrock.get-use-case-for-model-access-not-found bedrock.guardrail-regexes-over-max-count-rejected bedrock.start-test-workflow-while-in-use-rejected bedrock.update-automated-reasoning-policy-stale-version-rejected
Issue-Shape-Id: com.amazonaws.bedrock#AmazonBedrockControlPlaneService
Issue-Key: bedrock.automated-reasoning-test-case-stale-token-rejected|run#11|AssertionError|assert message == expected_conflict, f"expected message {expected_conflict!r}, got {message!r}"
Issue-Key: bedrock.delete-provisioned-throughput-missing-conflict-not-notfound|run#5|AssertionError|assert got == expected, f"expected {expected!r}, got {got!r}"
Issue-Key: bedrock.enforced-guardrail-configuration-unknown-guardrail-rejected|run#6|AssertionError|assert message == "The guardrail provided in the request does not exist.", (
Issue-Key: bedrock|GetUseCaseForModelAccess|ResourceNotFoundException
Issue-Key: bedrock|CreateGuardrail|ValidationException
Issue-Key: bedrock.start-test-workflow-while-in-use-rejected|run#15|AssertionError|assert message == "Testing workflow is currently in progress", f"unexpected rejection message: {message!r}"
Issue-Key: bedrock.update-automated-reasoning-policy-stale-version-rejected|run#20|AssertionError|assert stale_message == "Provided definition hash does not match the DRAFT policy definition hash.", f"unexpected rejection message: {stale_message!r}"
Issue-Commit: d7c92a359e65b119da339df847b0735e9ef403e3

# bedrock control plane: seven refusals are worded by the emulator instead of AWS (ConflictException / ResourceNotFoundException / ValidationException texts rewritten) (7 causes)

> _Note: this issue was identified against the code base at commit `d7c92a359e65b119da339df847b0735e9ef403e3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteAutomatedReasoningPolicyTestCase

## Description
Seven bedrock control-plane operations pick the right error CODE but emit the emulator's own prose instead of the message AWS returns for that error class. An error message is part of the contract: callers branch on it, docs and golden tests pin it, and operators read it. Each cause below is a distinct refusal site with its own handler; they are bundled only because they are one service and one kind of defect. The sibling open issue 20261026T233704Z-bedrock-batchdeleteevaluationjob-s-non-terminal-status-refusal-uses.md records the same defect for BatchDeleteEvaluationJob and does not cover any of the seven below.

=== Cause 1 of 7: DeleteAutomatedReasoningPolicyTestCase's stale-token ConflictException is re-worded ===
AWS: Smithy (com.amazonaws.bedrock) DeleteAutomatedReasoningPolicyTestCase = DELETE /automated-reasoning-policies/{policyArn}/test-cases/{testCaseId}, success 202, required input members policyArn (httpLabel), testCaseId (httpLabel) and lastUpdatedAt (httpQuery 'updatedAt'); modelled errors include ConflictException (httpError 400) and ResourceNotFoundException (404). The model does not state the message, so for the text: recorded behavior only — AWS refuses a delete carrying a lastUpdatedAt older than the test case's current updatedAt with ConflictException whose message is the fixed text "Conflict with deleting TestCase with testCaseId: " + the request's testCaseId + " and policyArn: " + the request's policyArn. The two variable parts are request values; there are no timestamps in it.
Emulator: same code, different message — "test <testCaseId> of automated reasoning policy <policyArn> was last updated at <stored updatedAt>, not at <submitted updatedAt>; read the test again and retry the delete with the updatedAt that read reports". It is the emulator's own sentence and additionally exposes both timestamps, which AWS's message never carries.
Why: a client that matches the documented conflict text to tell "someone else edited this test case" apart from a validation error sees no match and falls through to generic error handling; a retry wrapper keyed on that text never fires.
Fix: the ConflictException construction in DeleteAutomatedReasoningPolicyTestCase's handler/state check.
Traces: bedrock.automated-reasoning-test-case-stale-token-rejected.

=== Cause 2 of 7: DeleteProvisionedModelThroughput's not-found message is re-worded ===
AWS: Smithy DeleteProvisionedModelThroughput = DELETE /provisioned-model-throughput/{provisionedModelId}, success 200, errors include ResourceNotFoundException (404). Message text: recorded behavior only — AWS answers the fixed text "Provisioned model with the provided name " + the request's provisionedModelId + " was not found."
Emulator: ResourceNotFoundException with "no Provisioned Throughput <provisionedModelId> exists in this account" — emulator wording, and it adds an account scope AWS's message does not mention.
Why: tests and tooling that assert the AWS sentence (a very common pattern for distinguishing a never-created provisioned model from a conflicting state) fail against the emulator.
Fix: the ResourceNotFoundException construction in DeleteProvisionedModelThroughput's handler.
Traces: bedrock.delete-provisioned-throughput-missing-conflict-not-notfound.

=== Cause 3 of 7: PutEnforcedGuardrailConfiguration's unknown-guardrail message is re-worded ===
AWS: Smithy PutEnforcedGuardrailConfiguration = PUT /enforcedGuardrailsConfiguration, success 200, required member guardrailInferenceConfig, errors include ResourceNotFoundException (404). Message: recorded behavior only — the wholly fixed text "The guardrail provided in the request does not exist." (no request values interpolated).
Emulator: ResourceNotFoundException "This guardrail does not exist."
Why: same class — exact-text assertions and operator runbooks diverge, and the shortened sentence no longer says the identifier came from the request.
Fix: the not-found branch of PutEnforcedGuardrailConfiguration's handler.
Traces: bedrock.enforced-guardrail-configuration-unknown-guardrail-rejected.

=== Cause 4 of 7: GetUseCaseForModelAccess's no-form message is re-worded ===
AWS: Smithy GetUseCaseForModelAccess = GET /use-case-for-model-access, no input members, output member formData required; errors ResourceNotFoundException (404), ValidationException (400), ThrottlingException, InternalServerException. Message: recorded behavior only — for a caller that never submitted the form AWS answers 404 ResourceNotFoundException with the wholly fixed text "You have not filled out the request form. Fill out the form before getting access."
Emulator: ResourceNotFoundException "no use case for model access has been submitted for this account".
Why: AWS's text tells the caller what to DO (fill out the form); the emulator's does not, and any UI or CLI wrapper that surfaces the message verbatim shows different guidance than production.
Fix: the not-found branch of GetUseCaseForModelAccess's handler.
Traces: bedrock.get-use-case-for-model-access-not-found.

=== Cause 5 of 7: CreateGuardrail's regex-count refusal uses the generic length-constraint envelope instead of the quota message ===
AWS: Smithy CreateGuardrail = POST /guardrails, success 202, errors include ValidationException (400) and ServiceQuotaExceededException (400); sensitiveInformationPolicyConfig.regexesConfig is a list. Message: recorded behavior only — submitting more regexes than the sensitive-information policy admits is refused with the wholly fixed text "Number of regexes in sensitive information policy exceeds quota limit.", which names the regex-count quota and carries no value, so a caller can tell it apart from a single malformed regex entry.
Emulator: the framework's constraint envelope instead — "1 validation error detected: Value '31 items' at 'sensitiveInformationPolicyConfig.regexesConfig' failed to satisfy constraint: Member must have length less than or equal to 30". It also synthesizes a "'31 items'" value token for a collection member (the same synthetic-token defect already fixed for bedrock-agent-runtime Rerank in 20261025T012611Z).
Why: a client cannot distinguish "you exceeded the regex quota" from "one of your regexes is invalid", and the quota the message should name is absent.
Fix: the sensitive-information-policy validation in CreateGuardrail's handler/validation — this boundary must be refused by a quota check that owns its own message, before the generic list-length walk reports it.
Traces: bedrock.guardrail-regexes-over-max-count-rejected.

=== Cause 6 of 7: StartAutomatedReasoningPolicyTestWorkflow's in-progress refusal is re-worded ===
AWS: Smithy StartAutomatedReasoningPolicyTestWorkflow = POST /automated-reasoning-policies/{policyArn}/build-workflows/{buildWorkflowId}/test-workflows, success 200, errors include ResourceInUseException (400), ValidationException (400), ResourceNotFoundException (404). Message: recorded behavior only — starting a test workflow while one is already running is refused with the wholly fixed text "Testing workflow is currently in progress" (no identifiers, no status name).
Emulator: "Test workflow for build workflow: <buildWorkflowId> is already in SCHEDULED status" — emulator wording that additionally leaks the build-workflow id and an internal status name into a message AWS keeps constant.
Why: callers keying on the AWS sentence to back off and retry do not recognize the refusal; the leaked status name also couples clients to the emulator's state machine.
Fix: the in-use branch of StartAutomatedReasoningPolicyTestWorkflow's handler.
Traces: bedrock.start-test-workflow-while-in-use-rejected.

=== Cause 7 of 7: UpdateAutomatedReasoningPolicy's stale-definition-hash refusal is re-worded ===
AWS: Smithy UpdateAutomatedReasoningPolicy = PATCH /automated-reasoning-policies/{policyArn}, success 200, required members policyArn and policyDefinition, output definitionHash required; errors include ConflictException (400) and ValidationException (400). Message: recorded behavior only — an update submitted against a definition hash that is no longer the DRAFT's current hash is refused with the wholly fixed text "Provided definition hash does not match the DRAFT policy definition hash." (no ARN, no hashes).
Emulator: "the definition of automated reasoning policy <policyArn> has changed since it was last retrieved; retrieve the policy again and retry with its current definition hash".
Why: optimistic-concurrency clients match the AWS sentence to decide "re-read and retry"; the rewritten text breaks that, and it interpolates the policy ARN where AWS interpolates nothing.
Fix: the stale-hash branch of UpdateAutomatedReasoningPolicy's handler.
Traces: bedrock.update-automated-reasoning-policy-stale-version-rejected.

## Evidence
Behavioral suite simparity-behavioral-aws-v1, service bedrock 91/199 passed.

[mismatch] bedrock.automated-reasoning-test-case-stale-token-rejected
  expected message 'Conflict with deleting TestCase with testCaseId: 00000184KRLP and policyArn: arn:aws:bedrock:us-west-2:<account>:automated-reasoning-policy/000001800paj'
  got 'test 00000184KRLP of automated reasoning policy arn:aws:bedrock:us-west-2:<account>:automated-reasoning-policy/000001800paj was last updated at <t1>, not at <t2>; read the test again and retry the delete with the updatedAt that read reports'
  (ids/timestamps above are this run's values; the requirement is the template, not these bytes)

[mismatch] bedrock.delete-provisioned-throughput-missing-conflict-not-notfound
  expected ('ResourceNotFoundException', 'Provisioned model with the provided name <name> was not found.')
  got ('ResourceNotFoundException', 'no Provisioned Throughput <name> exists in this account')

[mismatch] bedrock.enforced-guardrail-configuration-unknown-guardrail-rejected
  expected ResourceNotFoundException message 'The guardrail provided in the request does not exist.'
  got 'This guardrail does not exist.'

[mismatch] bedrock.get-use-case-for-model-access-not-found
  expected message 'You have not filled out the request form. Fill out the form before getting access.'
  got 'no use case for model access has been submitted for this account'

[mismatch] bedrock.guardrail-regexes-over-max-count-rejected
  expected 'Number of regexes in sensitive information policy exceeds quota limit.'
  got "1 validation error detected: Value '31 items' at 'sensitiveInformationPolicyConfig.regexesConfig' failed to satisfy constraint: Member must have length less than or equal to 30"

[mismatch] bedrock.start-test-workflow-while-in-use-rejected
  expected 'Testing workflow is currently in progress'
  got 'Test workflow for build workflow: <buildWorkflowId> is already in SCHEDULED status'

[mismatch] bedrock.update-automated-reasoning-policy-stale-version-rejected
  expected 'Provided definition hash does not match the DRAFT policy definition hash.'
  got 'the definition of automated reasoning policy <policyArn> has changed since it was last retrieved; retrieve the policy again and retry with its current definition hash'

Model facts used (services/service-com-amazonaws-bedrock-amazonbedrockcontrolplaneservice/smithy-model/com-amazonaws-bedrock-amazonbedrockcontrolplaneservice.json): DeleteAutomatedReasoningPolicyTestCase DELETE .../test-cases/{testCaseId} code 202, lastUpdatedAt required httpQuery 'updatedAt', errors ConflictException 400 / ResourceInUseException 400 / ResourceNotFoundException 404 / ValidationException 400; DeleteProvisionedModelThroughput DELETE code 200, ResourceNotFoundException 404; PutEnforcedGuardrailConfiguration PUT /enforcedGuardrailsConfiguration code 200, ResourceNotFoundException 404; GetUseCaseForModelAccess GET /use-case-for-model-access code 200, errors ResourceNotFoundException 404 / ValidationException 400; CreateGuardrail POST /guardrails code 202, errors ValidationException 400 / ServiceQuotaExceededException 400; StartAutomatedReasoningPolicyTestWorkflow POST code 200, ResourceInUseException 400; UpdateAutomatedReasoningPolicy PATCH code 200, ConflictException 400 / ValidationException 400.

## Steps to Reproduce
Boot the emulator from this checkout and replay the seven checks named in trace_ids (suite simparity-behavioral-aws-v1). Per cause:
1. CreateAutomatedReasoningPolicy, CreateAutomatedReasoningPolicyTestCase, read the test case's updatedAt, update it so updatedAt advances, then DeleteAutomatedReasoningPolicyTestCase with the OLD updatedAt -> expect ConflictException 'Conflict with deleting TestCase with testCaseId: <id> and policyArn: <arn>'.
2. DeleteProvisionedModelThroughput(provisionedModelId=<never-created name>) -> expect 'Provisioned model with the provided name <name> was not found.'
3. PutEnforcedGuardrailConfiguration with guardrailInferenceConfig.guardrailIdentifier naming a guardrail that does not exist -> expect 'The guardrail provided in the request does not exist.'
4. GetUseCaseForModelAccess on an account that never submitted the form -> expect 404 'You have not filled out the request form. Fill out the form before getting access.'
5. CreateGuardrail with sensitiveInformationPolicyConfig.regexesConfig holding one more entry than the quota admits -> expect ValidationException 'Number of regexes in sensitive information policy exceeds quota limit.'
6. StartAutomatedReasoningPolicyTestWorkflow twice in a row on the same policy/build workflow -> the second expects 'Testing workflow is currently in progress'.
7. GetAutomatedReasoningPolicy for its definitionHash, mutate the DRAFT, then UpdateAutomatedReasoningPolicy with the stale hash -> expect 'Provided definition hash does not match the DRAFT policy definition hash.'
