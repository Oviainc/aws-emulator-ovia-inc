Issue-Source: trace-sweep-divergence
Issue-Trace: bedrock-advanced-prompt-optimization-jobs
Issue-Step: task#2
Issue-Severity: LOW
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: bedrock-advanced-prompt-optimization-jobs bedrock-evaluation-job-lifecycle bedrock-guardrail-lifecycle-and-versioning
Issue-Shape-Id: com.amazonaws.bedrock#AmazonBedrockControlPlaneService
Issue-Key: bedrock|GetAdvancedPromptOptimizationJob|4xx->4xx|$.message
Issue-Key: bedrock|StopAdvancedPromptOptimizationJob|4xx->4xx|$.message
Issue-Key: bedrock|BatchDeleteAdvancedPromptOptimizationJob|4xx->4xx|$.message
Issue-Key: bedrock|GetEvaluationJob|4xx->4xx|$.message
Issue-Key: bedrock|StopEvaluationJob|4xx->4xx|$.message
Issue-Key: bedrock|BatchDeleteEvaluationJob|4xx->4xx|$.message
Issue-Key: bedrock|GetGuardrail|4xx->4xx|$.message
Issue-Commit: 242e2a822ca6eb3fec2aad3ba89160f0a6f40669

# Bedrock validation-error messages render model regex with ^ and $ anchors and mis-shape list-member failures

> _Note: this issue was identified against the code base at commit `242e2a822ca6eb3fec2aad3ba89160f0a6f40669`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetAdvancedPromptOptimizationJob

## Description
Seven causes, all one fault in the shared constraint-violation message builder for bedrock @pattern/@length members. The HTTP status (400) and the error code (ValidationException) are correct everywhere; what diverges is the message text, and the message text here IS a requirement because it is built from the request and the model (it names the offending member and quotes the model's constraint), so the rule that produces it is known.

Two distinct rule breaks run through all seven:
(a) ANCHOR STRIPPING. AWS renders the regex with the leading '^' and trailing '$' of the modeled @pattern removed; the emulator echoes the model pattern verbatim, anchors included. This holds for six different patterns across three fixtures, so it is the rule, not one recording's text.
(b) LIST AGGREGATION. For a list input, AWS reports the failure against the list member name ('jobIdentifiers') with the member's whole constraint set aggregated ('Member must satisfy constraint: [Member must have length less than or equal to 1011, Member must have length greater than or equal to 0, Member must satisfy regular expression pattern: ...]'); the emulator reports an indexed element path ('jobIdentifiers.1.member') carrying only the pattern constraint and dropping the two length constraints.

Cause 1 of 7: GetAdvancedPromptOptimizationJob, jobIdentifier.
AWS: Smithy AdvancedPromptOptimizationJobIdentifier @pattern ^((arn:aws(-[^:]+)?:bedrock:[a-z0-9-]{1,20}:[0-9]{12}:advanced-prompt-optimization-job/)?[a-z0-9]{12})$, @length max 1011; error ValidationException httpError 400. Recorded 400 message quotes the pattern with no anchors and, for this resource, does not echo the submitted value ('Value at 'jobIdentifier' failed to satisfy constraint: Member must satisfy regular expression pattern: ((arn:aws...)?[a-z0-9]{12})').
Emulator: 400 with the anchored pattern and the value echoed.
Why: a client or test that compares the reported constraint against the model's documented pattern sees a pattern it never published; tooling that re-uses the reported regex to pre-validate input compiles a different expression.
Fix: the service-wide validation-message formatter (constraint rendering) for bedrock, not this handler.
Traces: bedrock-advanced-prompt-optimization-jobs#task#2.
Hypothesis: the value omission on this resource is value redaction for job identifiers; the evaluation-job and guardrail recordings below DO echo the value, so only the anchor rule is asserted as a requirement here.

Cause 2 of 7: StopAdvancedPromptOptimizationJob, same member, same anchored-pattern divergence.
Traces: bedrock-advanced-prompt-optimization-jobs#task#3.

Cause 3 of 7: BatchDeleteAdvancedPromptOptimizationJob, jobIdentifiers.
AWS: Smithy list member @pattern (as above) + @length max 1011; recorded 400 aggregates the constraints under 'jobIdentifiers'.
Emulator: 400 reporting 'jobIdentifiers.1.member' with the anchored pattern only.
Why: a client cannot map the emulator's indexed path onto the member path AWS reports, and the length constraints AWS lists are absent.
Fix: list/collection branch of the validation-message formatter.
Traces: bedrock-advanced-prompt-optimization-jobs#task#5.

Cause 4 of 7: GetEvaluationJob, jobIdentifier.
AWS: Smithy EvaluationJobIdentifier @pattern ^(arn:aws(-[^:]+)?:bedrock:[a-z0-9-]{1,20}:[0-9]{12}:evaluation-job/[a-z0-9]{12})$, @length max 1011, @sensitive; error ValidationException 400. Recorded message echoes the value and quotes the pattern WITHOUT anchors.
Emulator: identical except the pattern is anchored.
Fix: same formatter.
Traces: bedrock-evaluation-job-lifecycle#task#3.

Cause 5 of 7: StopEvaluationJob, same member, same anchor divergence.
Traces: bedrock-evaluation-job-lifecycle#task#4.

Cause 6 of 7: BatchDeleteEvaluationJob, jobIdentifiers.
AWS: Smithy EvaluationJobIdentifiers list, member @pattern/@length as above; recorded 400 reports the whole list value against 'jobIdentifiers' with the aggregated constraint set.
Emulator: 'jobIdentifiers.1.member' with the anchored pattern only.
Fix: list branch of the formatter.
Traces: bedrock-evaluation-job-lifecycle#task#5.

Cause 7 of 7: GetGuardrail, guardrailIdentifier.
AWS: Smithy GetGuardrail.guardrailIdentifier @pattern ^(([a-z0-9]+)|(arn:aws(-[^:]+)?:bedrock:[a-z0-9-]{1,20}:[0-9]{12}:guardrail/[a-z0-9]+))$; recorded 400 quotes it unanchored.
Emulator: anchored.
Fix: same formatter.
Traces: bedrock-guardrail-lifecycle-and-versioning#task#7.

Not filed alongside these, for the record: PutAccountDataRetention task#3 (bedrock-account-data-retention) differs only in enum-set ordering and value echoing, and the model's DataRetentionMode order matches the emulator, so no rule fixes AWS's order.

## Evidence
Replay of simparity-wire-aws-v1 against the emulator built from mainline.

Cause 1 (bedrock-advanced-prompt-optimization-jobs#task#2, GetAdvancedPromptOptimizationJob): status 400 expected and 400 actual; $.message expected "1 validation error detected: Value at 'jobIdentifier' failed to satisfy constraint: Member must satisfy regular expression pattern: ((arn:aws(-[^:]+)?:bedrock:[a-z0-9-]{1,20}:[0-9]{12}:advanced-prompt-optimization-job/)?[a-z0-9]{12})", actual "1 validation error detected: Value 'arn:aws:bedrock:us-east-1:000000000000:advanced-prompt-optimization-job/sim2real-probe-bedrock-nonexistent-apo-job-681564ea' at 'jobIdentifier' failed to satisfy constraint: Member must satisfy regular expression pattern: ^((arn:aws(-[^:]+)?:bedrock:[a-z0-9-]{1,20}:[0-9]{12}:advanced-prompt-optimization-job/)?[a-z0-9]{12})$".

Cause 3 (bedrock-advanced-prompt-optimization-jobs#task#5, BatchDeleteAdvancedPromptOptimizationJob): expected "... Value at 'jobIdentifiers' failed to satisfy constraint: Member must satisfy constraint: [Member must have length less than or equal to 1011, Member must have length greater than or equal to 0, Member must satisfy regular expression pattern: ((arn:aws...)?[a-z0-9]{12})]"; actual "... at 'jobIdentifiers.1.member' failed to satisfy constraint: Member must satisfy regular expression pattern: ^((arn:aws...)?[a-z0-9]{12})$".

Cause 4/5 (bedrock-evaluation-job-lifecycle#task#3, #task#4): expected pattern text "(arn:aws(-[^:]+)?:bedrock:[a-z0-9-]{1,20}:[0-9]{12}:evaluation-job/[a-z0-9]{12})"; actual the same string wrapped in ^...$.

Cause 6 (bedrock-evaluation-job-lifecycle#task#5): expected "Value '[arn:aws:bedrock:us-east-1:000000000000:evaluation-job/nonexistent]' at 'jobIdentifiers' ... Member must satisfy constraint: [Member must have length less than or equal to 1011, Member must have length greater than or equal to 0, Member must satisfy regular expression pattern: ...]"; actual indexed 'jobIdentifiers.1.member' with pattern only.

Cause 7 (bedrock-guardrail-lifecycle-and-versioning#task#7): expected "... Member must satisfy regular expression pattern: (([a-z0-9]+)|(arn:aws(-[^:]+)?:bedrock:[a-z0-9-]{1,20}:[0-9]{12}:guardrail/[a-z0-9]+))"; actual the same wrapped in ^...$.

Model evidence (smithy_query on services/service-com-amazonaws-bedrock-amazonbedrockcontrolplaneservice/smithy-model/com-amazonaws-bedrock-amazonbedrockcontrolplaneservice.json): GetGuardrail.guardrailIdentifier, GetEvaluationJob.jobIdentifier, EvaluationJobIdentifiers.member and AdvancedPromptOptimizationJobIdentifier all carry anchored @pattern plus @length, and every one of these operations models ValidationException at httpError 400.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from mainline.
2. Replay the simparity-wire-aws-v1 corpus (replay_wire_corpus), or send the individual requests:
   a. GET /advanced-prompt-optimization-jobs/arn%3Aaws%3Abedrock%3Aus-east-1%3A000000000000%3Aadvanced-prompt-optimization-job%2Fsim2real-probe-bedrock-nonexistent-apo-job-681564ea (and the POST .../stop form) -> 400 ValidationException.
   b. POST /advanced-prompt-optimization-jobs/batch-delete with jobIdentifiers=[that ARN] -> 400.
   c. GET /evaluation-jobs/<cross-region evaluation-job ARN>, POST .../stop, POST /evaluation-jobs/batch-delete -> 400.
   d. GET /guardrails/nonexistent-guardrail-id-00000 -> 400.
3. Compare $.message against the recorded response: the pattern must appear with the model's ^ and $ stripped, and a list failure must be reported against the list member name with its full aggregated constraint set, not against '<member>.<index>.member'.
