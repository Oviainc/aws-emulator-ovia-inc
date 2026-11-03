Issue-Source: trace-sweep-divergence
Issue-Trace: bedrock-advanced-prompt-optimization-jobs
Issue-Step: task#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: bedrock-advanced-prompt-optimization-jobs bedrock-automated-reasoning-policy-test-case
Issue-Shape-Id: com.amazonaws.bedrock#AmazonBedrockControlPlaneService
Issue-Key: bedrock|GetAdvancedPromptOptimizationJob|4xx->4xx|$.message
Issue-Key: bedrock|StopAdvancedPromptOptimizationJob|4xx->4xx|$.message
Issue-Key: bedrock|BatchDeleteAdvancedPromptOptimizationJob|4xx->4xx|$.message
Issue-Key: bedrock|GetAutomatedReasoningPolicyTestCase|4xx->4xx|$.message
Issue-Commit: 95ee1f215215607c18ab877072afcef7ad3742b1

# bedrock ValidationException messages echo the rejected value instead of AWS's "Value at '<member>'" wording

> _Note: this issue was identified against the code base at commit `95ee1f215215607c18ab877072afcef7ad3742b1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetAdvancedPromptOptimizationJob

## Description
FOUR causes, one shared fault: the emulator's constraint-violation message builder interpolates the rejected value into the message for members whose value AWS deliberately omits. AWS's ValidationException text for these members is 'N validation error detected: Value at '<member>' failed to satisfy constraint: ...' — no value is quoted, because these members (job identifiers / ARNs / test-case ids) are echoed back only as the member name. The emulator instead emits 'Value '<the rejected value>' at '<member>' failed ...'. Status (400) and error code (ValidationException) are correct in every cause; only the message diverges. Fixed text of the requirement: the literal prefix '1 validation error detected: Value at ', the member name in single quotes, ' failed to satisfy constraint: Member must satisfy regular expression pattern: ' and the modelled pattern verbatim. The variable parts are the error count, the member name and the pattern taken from the model — never the caller's value.

Cause 1 of 4: GetAdvancedPromptOptimizationJob with a jobIdentifier that violates the modelled pattern.
AWS: 400 ValidationException, message "1 validation error detected: Value at 'jobIdentifier' failed to satisfy constraint: Member must satisfy regular expression pattern: ((arn:aws(-[^:]+)?:bedrock:[a-z0-9-]{1,20}:[0-9]{12}:advanced-prompt-optimization-job/)?[a-z0-9]{12})". Smithy: GetAdvancedPromptOptimizationJob input jobIdentifier is a pattern-constrained @httpLabel; errors include ValidationException (httpError 400).
Emulator: 400 ValidationException, but message "1 validation error detected: Value '<jobIdentifier as sent>' at 'jobIdentifier' failed to satisfy constraint: Member must satisfy regular expression pattern: (...)".
Why: clients and test suites that assert on AWS's exact validation text (and log scrubbers that rely on AWS not echoing identifiers) see a message that never matches; an identifier the service is documented not to reflect is reflected.
Fix: the shared ValidationException message formatter used by the control-plane input validation layer — it must not interpolate the offending value for these members.
Traces: bedrock-advanced-prompt-optimization-jobs#task#2.

Cause 2 of 4: StopAdvancedPromptOptimizationJob with the same malformed jobIdentifier.
AWS: 400 ValidationException, message "1 validation error detected: Value at 'jobIdentifier' failed to satisfy constraint: Member must satisfy regular expression pattern: ((arn:aws(-[^:]+)?:bedrock:[a-z0-9-]{1,20}:[0-9]{12}:advanced-prompt-optimization-job/)?[a-z0-9]{12})".
Emulator: same 400/ValidationException but the message carries "Value '<jobIdentifier as sent>' at 'jobIdentifier'".
Why: as cause 1 — message-level assertions fail and the identifier is echoed.
Fix: same formatter.
Traces: bedrock-advanced-prompt-optimization-jobs#task#3.

Cause 3 of 4: BatchDeleteAdvancedPromptOptimizationJob with a jobIdentifiers list whose member violates the pattern.
AWS: 400 ValidationException, message "1 validation error detected: Value at 'jobIdentifiers' failed to satisfy constraint: Member must satisfy constraint: [Member must have length less than or equal to 1011, Member must have length greater than or equal to 0, Member must satisfy regular expression pattern: ((arn:aws(-[^:]+)?:bedrock:[a-z0-9-]{1,20}:[0-9]{12}:advanced-prompt-optimization-job/)?[a-z0-9]{12})]" — the aggregate member constraint list, with no value quoted.
Emulator: emits "Value '[<the list rendered>]' at 'jobIdentifiers' failed to satisfy constraint: ...", i.e. it renders the whole list into the message.
Why: same as above, aggravated because a long list is pasted into the error body.
Fix: same formatter, list-member branch.
Traces: bedrock-advanced-prompt-optimization-jobs#task#5.

Cause 4 of 4: GetAutomatedReasoningPolicyTestCase with a testCaseId that violates the modelled pattern.
AWS: 400 ValidationException, message "1 validation error detected: Value at 'testCaseId' failed to satisfy constraint: Member must satisfy regular expression pattern: [0-9A-Z]{12}". Smithy: AutomatedReasoningPolicyTestCaseId @pattern ^[0-9A-Z]{12}$, @length max 12.
Emulator: 400 ValidationException with "Value '<testCaseId as sent>' at 'testCaseId' failed ...".
Why: as cause 1.
Fix: same formatter.
Traces: bedrock-automated-reasoning-policy-test-case#task#9.

Related open issue (does NOT cover these steps — it carries no replay key for them): 20261029T172312Z-bedrock-control-plane-response-message-representation-pattern-violation.md, which reports the same echoing fault on guardrail pattern violations.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from mainline.

Cause 1 — bedrock-advanced-prompt-optimization-jobs#task#2 GetAdvancedPromptOptimizationJob: expected status 400, actual 400; $.message expected "1 validation error detected: Value at 'jobIdentifier' failed to satisfy constraint: Member must satisfy regular expression pattern: ((arn:aws(-[^:]+)?:bedrock:[a-z0-9-]{1,20}:[0-9]{12}:advanced-prompt-optimization-job/)?[a-z0-9]{12})", got "1 validation error detected: Value 'arn:aws:bedrock:us-east-1:000000000000:advanced-prompt-optimization-job/<job-name>' at 'jobIdentifier' failed to satisfy constraint: Member must satisfy regular expression pattern: (same pattern)".

Cause 2 — bedrock-advanced-prompt-optimization-jobs#task#3 StopAdvancedPromptOptimizationJob: identical expected/actual message divergence on 'jobIdentifier'.

Cause 3 — bedrock-advanced-prompt-optimization-jobs#task#5 BatchDeleteAdvancedPromptOptimizationJob: expected "... Value at 'jobIdentifiers' failed to satisfy constraint: Member must satisfy constraint: [Member must have length less than or equal to 1011, Member must have length greater than or equal to 0, Member must satisfy regular expression pattern: (...)]", got "... Value '[<arn>]' at 'jobIdentifiers' failed to satisfy constraint: ...".

Cause 4 — bedrock-automated-reasoning-policy-test-case#task#9 GetAutomatedReasoningPolicyTestCase: expected "... Value at 'testCaseId' failed to satisfy constraint: Member must satisfy regular expression pattern: [0-9A-Z]{12}", got "... Value '<id>' at 'testCaseId' failed ...".

In all four the HTTP status and x-amzn-errortype matched; only $.message diverged.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it.
2. Replay fixture bedrock-advanced-prompt-optimization-jobs, steps task#2, task#3 and task#5, and fixture bedrock-automated-reasoning-policy-test-case step task#9.
3. Inspect $.message of each 400 response: it contains "Value '<value>' at '<member>'" where AWS emits "Value at '<member>'".
4. Equivalently: call GetAdvancedPromptOptimizationJob / StopAdvancedPromptOptimizationJob with any jobIdentifier that violates the modelled pattern, BatchDeleteAdvancedPromptOptimizationJob with such an identifier in jobIdentifiers, and GetAutomatedReasoningPolicyTestCase with a testCaseId that is not 12 chars of [0-9A-Z].
