Issue-Source: trace-sweep-divergence
Issue-Trace: bedrock-runtime-apply-guardrail-error-paths
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: bedrock-runtime-apply-guardrail-error-paths bedrock-runtime-apply-guardrail-error-recovery bedrockruntime-apply-guardrail-output-scope bedrock-runtime-async-invoke-lifecycle-and-errors bedrock-runtime-list-async-invokes-basic bedrockruntime-async-invoke-read-and-rejections bedrock-runtime-get-async-invoke-malformed-arn bedrock-runtime-get-async-invoke-unknown-but-well-formed-arn bedrock-runtime-start-async-invoke-invalid-s3-uri bedrockruntime-identity-vs-capability-rejections bedrock-runtime-count-tokens-before-invoke bedrock-runtime-count-tokens-error-unknown-model bedrock-runtime-guardrail-checks-inline bedrock-runtime-invoke-guardrail-checks-combined-multi-role bedrock-runtime-invoke-guardrail-checks-prompt-attack bedrock-runtime-invoke-guardrail-checks-sensitive-information-multi-message bedrockruntime-guardrail-checks-decisions bedrock-runtime-invoke-guardrail-checks-error-paths
Issue-Shape-Id: com.amazonaws.bedrockruntime#AmazonBedrockFrontendService
Issue-Key: bedrock-runtime|ApplyGuardrail|4xx->5xx|$.message
Issue-Key: bedrock-runtime|ApplyGuardrail|4xx->5xx|$headers.content-type
Issue-Key: bedrock-runtime|ApplyGuardrail|4xx->5xx|$headers.x-amzn-errortype
Issue-Key: bedrock-runtime|ApplyGuardrail|4xx->5xx|status
Issue-Key: bedrock-runtime|ListAsyncInvokes|2xx->5xx|$.asyncInvokeSummaries
Issue-Key: bedrock-runtime|ListAsyncInvokes|2xx->5xx|$headers.content-type
Issue-Key: bedrock-runtime|ListAsyncInvokes|2xx->5xx|status
Issue-Key: bedrock-runtime|ListAsyncInvokes|4xx->5xx|$.message
Issue-Key: bedrock-runtime|ListAsyncInvokes|4xx->5xx|$headers.content-type
Issue-Key: bedrock-runtime|ListAsyncInvokes|4xx->5xx|$headers.x-amzn-errortype
Issue-Key: bedrock-runtime|ListAsyncInvokes|4xx->5xx|status
Issue-Key: bedrock-runtime|GetAsyncInvoke|4xx->5xx|$.message
Issue-Key: bedrock-runtime|GetAsyncInvoke|4xx->5xx|$headers.content-type
Issue-Key: bedrock-runtime|GetAsyncInvoke|4xx->5xx|$headers.x-amzn-errortype
Issue-Key: bedrock-runtime|GetAsyncInvoke|4xx->5xx|status
Issue-Key: bedrock-runtime|GetAsyncInvoke|4xx->5xx|$.Message
Issue-Key: bedrock-runtime|StartAsyncInvoke|4xx->5xx|$.message
Issue-Key: bedrock-runtime|StartAsyncInvoke|4xx->5xx|$headers.content-type
Issue-Key: bedrock-runtime|StartAsyncInvoke|4xx->5xx|$headers.x-amzn-errortype
Issue-Key: bedrock-runtime|StartAsyncInvoke|4xx->5xx|status
Issue-Key: bedrock-runtime|CountTokens|4xx->5xx|$.message
Issue-Key: bedrock-runtime|CountTokens|4xx->5xx|$headers.content-type
Issue-Key: bedrock-runtime|CountTokens|4xx->5xx|$headers.x-amzn-errortype
Issue-Key: bedrock-runtime|CountTokens|4xx->5xx|status
Issue-Key: bedrock-runtime|InvokeGuardrailChecks|2xx->5xx|$.results
Issue-Key: bedrock-runtime|InvokeGuardrailChecks|2xx->5xx|$.usage
Issue-Key: bedrock-runtime|InvokeGuardrailChecks|2xx->5xx|$headers.content-type
Issue-Key: bedrock-runtime|InvokeGuardrailChecks|2xx->5xx|status
Issue-Key: bedrock-runtime|InvokeGuardrailChecks|4xx->5xx|$.message
Issue-Key: bedrock-runtime|InvokeGuardrailChecks|4xx->5xx|$headers.content-type
Issue-Key: bedrock-runtime|InvokeGuardrailChecks|4xx->5xx|$headers.x-amzn-errortype
Issue-Key: bedrock-runtime|InvokeGuardrailChecks|4xx->5xx|status
Issue-Commit: ceabbcc456fe2a783c33070041a4243561c3aa32

# Bedrock Runtime guardrail, async-invoke and count-tokens operations refused 500 AmbiguousService by sigv4-name routing

> _Note: this issue was identified against the code base at commit `ceabbcc456fe2a783c33070041a4243561c3aa32`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ApplyGuardrail

## Description
SHARED FAULT: the emulator's request router resolves a request to an emulated service by its SigV4 signing name. Smithy (com-amazonaws-bedrockruntime-amazonbedrockfrontendservice): sdkId 'Bedrock Runtime', endpointPrefix 'bedrock-runtime', sigv4Name 'bedrock', protocols ['restJson1']. Six emulated Bedrock-family services declare sigv4Name 'bedrock', so resolution is ambiguous and the router answers HTTP 500 with x-amzn-errortype 'AmbiguousService' and body "6 emulated services all claim requests for 'bedrock' ...". Secondary defect in every cause: the refusal is serialized as 'application/x-amz-json-1.1' although restJson1 responses — including errors — are 'application/json'.

This issue covers the guardrail, asynchronous-invocation and token-counting surface. TEN CAUSES; each is a distinct operation+status contract a fixer must see answer correctly.

Cause 1 of 10 — ApplyGuardrail, input in incorrect format (400).
AWS: Smithy ApplyGuardrail -> POST /guardrail/{guardrailIdentifier}/version/{guardrailVersion}/apply; ValidationException is a service-common error (errorKind client, httpError 400, member `message`). Recorded: 400, content-type application/json, x-amzn-errortype 'ValidationException:http://internal.amazon.com/coral/com.amazon.bedrock/'; recorded message example 'Guardrail was enabled but input is in incorrect format.' (status and error code are the requirement; the text is an example).
Emulator: 500, x-amzn-errortype 'AmbiguousService', content-type application/x-amz-json-1.1.
Why: a client sees a retryable server fault instead of a terminal input rejection, so it retries a request that can never succeed.
Fix: the request router / service-resolution layer.
Traces: bedrock-runtime-apply-guardrail-error-paths#task#1 (cluster also covers bedrock-runtime-apply-guardrail-error-paths#task#2, #task#3, bedrock-runtime-apply-guardrail-error-recovery#task#1, #task#2 and bedrockruntime-apply-guardrail-output-scope#task#5).

Cause 2 of 10 — ListAsyncInvokes success listing (200).
AWS: Smithy ListAsyncInvokes -> GET /async-invoke, output ListAsyncInvokesResponse. Recorded: 200, content-type application/json, body `asyncInvokeSummaries` with per-invocation invocationArn, modelArn, clientRequestToken, status, submitTime, lastModifiedTime, endTime and outputDataConfig.s3OutputDataConfig.s3Uri. (ARNs, ids, bucket names and timestamps are environment-specific examples; the member set and status are the requirement.)
Emulator: 500 / AmbiguousService; $.asyncInvokeSummaries absent.
Why: a client can never enumerate its asynchronous invocations, so no async workflow can poll for completion.
Fix: the request router / service-resolution layer (then the ListAsyncInvokes handler and state).
Traces: bedrock-runtime-async-invoke-lifecycle-and-errors#task#2 (cluster also covers bedrock-runtime-list-async-invokes-basic#task#1 and bedrockruntime-async-invoke-read-and-rejections#task#1).

Cause 3 of 10 — ListAsyncInvokes, maxResults above its modeled ceiling (400).
AWS: ValidationException, httpError 400, content-type application/json. The recorded message is built from the request and its constraint, so the RULE is the requirement: "1 validation error detected: Value '<value>' at 'maxResults' failed to satisfy constraint: Member must have value less than or equal to 1000" (recorded with value 5000).
Emulator: 500 / AmbiguousService, generic router text, no per-member constraint report.
Why: a paginating client cannot discover its page-size limit; it gets a 500 instead of a constraint violation naming maxResults.
Fix: the request router / service-resolution layer (then ListAsyncInvokes input validation).
Traces: bedrock-runtime-async-invoke-lifecycle-and-errors#task#3.

Cause 4 of 10 — GetAsyncInvoke, malformed invocation ARN (400).
AWS: Smithy GetAsyncInvoke -> GET /async-invoke/{invocationArn}; ValidationException, httpError 400, content-type application/json; recorded message example 'The provided Async Invoke arn is not valid'.
Emulator: 500 / AmbiguousService.
Why: a syntactically bad ARN is reported as a server failure rather than a client error, so clients retry instead of correcting the ARN.
Fix: the request router / service-resolution layer (then GetAsyncInvoke ARN validation).
Traces: bedrock-runtime-async-invoke-lifecycle-and-errors#task#4 (cluster also covers bedrock-runtime-get-async-invoke-malformed-arn#task#1, bedrock-runtime-get-async-invoke-unknown-but-well-formed-arn#task#1, bedrockruntime-async-invoke-read-and-rejections#task#2 and #task#3).

Cause 5 of 10 — GetAsyncInvoke, ARN belonging to a different account (403).
AWS: AccessDeniedException (Smithy: errorKind client, httpError 403, member `message`); it is a bedrock-runtime service-common error. Recorded: 403, content-type application/json, body member `Message` present (recorded example 'The provided resource ARN is from a different account.').
Emulator: 500 / AmbiguousService; $.Message absent.
Why: a cross-account access denial is indistinguishable from a transient outage, and no authorization gate is observable at all.
Fix: the request router / service-resolution layer (then the GetAsyncInvoke authorization check).
Traces: bedrock-runtime-async-invoke-lifecycle-and-errors#task#5.

Cause 6 of 10 — StartAsyncInvoke, s3Uri failing its modeled pattern (400).
AWS: Smithy StartAsyncInvoke -> POST /async-invoke; ValidationException, httpError 400, content-type application/json. Message is built from the request and the member's @pattern, so the RULE is the requirement: "1 validation error detected: Value '<value>' at 'outputDataConfig.s3OutputDataConfig.s3Uri' failed to satisfy constraint: Member must satisfy regular expression pattern: s3://[a-z0-9][\.\-a-z0-9]{1,61}[a-z0-9](/.*)?" (recorded with value 'not-a-valid-s3-uri').
Emulator: 500 / AmbiguousService, generic router text, no member-level pattern report.
Why: a caller submitting a bad output location gets a server error and no indication of which member or pattern failed.
Fix: the request router / service-resolution layer (then StartAsyncInvoke input validation).
Traces: bedrock-runtime-start-async-invoke-invalid-s3-uri#task#1 (cluster also covers bedrockruntime-async-invoke-read-and-rejections#task#4, bedrockruntime-identity-vs-capability-rejections#task#3 and #task#4).

Cause 7 of 10 — CountTokens against a model that does not support counting (400).
AWS: Smithy CountTokens -> POST /model/{modelId}/count-tokens; ValidationException, httpError 400, content-type application/json, x-amzn-errortype 'ValidationException:http://internal.amazon.com/coral/com.amazon.bedrock/'; recorded message example "The provided model doesn't support counting tokens."
Emulator: 500 / AmbiguousService.
Why: a capability rejection is reported as a server fault, so the client cannot tell 'this model cannot do that' from 'try again later'.
Fix: the request router / service-resolution layer (then the CountTokens capability check).
Traces: bedrock-runtime-count-tokens-before-invoke#task#1 (cluster also covers bedrock-runtime-count-tokens-before-invoke#task#2, bedrockruntime-identity-vs-capability-rejections#task#1 and #task#2).

Cause 8 of 10 — CountTokens against an unknown / end-of-life model (404).
AWS: Smithy CountTokens errors include ResourceNotFoundException (errorKind client, httpError 404). Recorded: 404, content-type application/json, x-amzn-errortype 'ResourceNotFoundException:http://internal.amazon.com/coral/com.amazon.bedrock/'; recorded message example 'This model version has reached the end of its life. Please refer to the AWS documentation for more details.'
Emulator: 500 / AmbiguousService.
Why: identity failures (unknown model, 404) and capability failures (400, cause 7) are both collapsed into 500, so neither is distinguishable.
Fix: the request router / service-resolution layer.
Traces: bedrock-runtime-count-tokens-error-unknown-model#task#1.

Cause 9 of 10 — InvokeGuardrailChecks success envelope (200).
AWS: Smithy InvokeGuardrailChecks -> POST /guardrail-checks/invoke, output InvokeGuardrailChecksResponse. Recorded: 200, content-type application/json, body members `results` (contentFilter.results[].category/severityScore, promptAttack.results[], sensitiveInformation.results[] with beginOffset/endOffset/confidenceScore/type and `truncated`, toolUse) and `usage` (per-check textUnits). (Scores, offsets and unit counts are examples; the member set and status are the requirement.)
Emulator: 500 / AmbiguousService; $.results and $.usage absent.
Why: inline guardrail evaluation returns no verdict, so a client cannot decide whether to block content — a safety-relevant path silently fails open or hard-fails.
Fix: the request router / service-resolution layer (then the InvokeGuardrailChecks handler and serializer).
Traces: bedrock-runtime-guardrail-checks-inline#task#1 (cluster also covers bedrock-runtime-invoke-guardrail-checks-combined-multi-role#task#1, bedrock-runtime-invoke-guardrail-checks-prompt-attack#task#1, bedrock-runtime-invoke-guardrail-checks-sensitive-information-multi-message#task#1, bedrockruntime-guardrail-checks-decisions#task#1, #task#2 and #task#4).

Cause 10 of 10 — InvokeGuardrailChecks, category outside its modeled enum (400).
AWS: ValidationException, httpError 400, content-type application/json. Message is built from the request and the enum constraint, so the RULE is the requirement: "1 validation error detected: Value '<value>' at 'checks.contentFilter.categories.<i>.member.category' failed to satisfy constraint: Member must satisfy enum value set: [INSULTS, HATE, MISCONDUCT, SEXUAL, VIOLENCE]" (recorded with value 'NOT_A_REAL_CATEGORY').
Emulator: 500 / AmbiguousService, generic router text, no enum-constraint report.
Why: a misspelled category yields an un-actionable 500 instead of a validation error naming the member and the permitted values.
Fix: the request router / service-resolution layer (then InvokeGuardrailChecks input validation).
Traces: bedrock-runtime-guardrail-checks-inline#task#2 (cluster also covers bedrock-runtime-invoke-guardrail-checks-error-paths#task#1, #task#2, #task#3 and bedrockruntime-guardrail-checks-decisions#task#3).

RELATED OPEN ISSUES (same resolver fault, other services — none covers bedrock-runtime): 20261022T042609Z-bedrock-data-automation-runtime-sigv4-name-routing-makes-all-bedrock-data-automation.md, 20261022T062300Z-forecastquery-is-entirely-unreachable-router-refuses-every-request.md, 20261022T071206Z-kinesis-analytics-v2-is-entirely-unreachable-router-refuses.md.

## Evidence
Replay of trace set simparity-wire-aws-v1: 65 steps, 2 passed, 55 mismatched, score 3.1%. emulator_up=true (/__admin/health 200, :41535 listener accepting), bootstrap_signal=false, state_reset_failures=0 — the service answers, and answers wrongly.

All ten causes produced the same actual response:
  actual status 500 (expected 400 on causes 1,3,4,6,7,10; 403 on cause 5; 404 on cause 8; 200 on causes 2 and 9)
  header x-amzn-errortype: 'AmbiguousService' (expected 'ValidationException:http://internal.amazon.com/coral/com.amazon.bedrock/' or 'ResourceNotFoundException:http://internal.amazon.com/coral/com.amazon.bedrock/' on the error causes)
  header content-type: 'application/x-amz-json-1.1' (expected 'application/json' on all ten)
  body $.message: "6 emulated services all claim requests for 'bedrock': bedrock, bedrock, bedrock, bedrock, bedrock, bedrock; the request is rejected rather than served by an arbitrary one"

Per-cause missing members reported by the replay:
  GetAsyncInvoke 403 (cause 5): $.Message missing (expected 'The provided resource ARN is from a different account.')
  ListAsyncInvokes 200 (cause 2): $.asyncInvokeSummaries missing
  InvokeGuardrailChecks 200 (cause 9): $.results and $.usage both missing

Model evidence: smithy_query digest/service_info on services/service-com-amazonaws-bedrockruntime-amazonbedrockfrontendservice/smithy-model/com-amazonaws-bedrockruntime-amazonbedrockfrontendservice.json — sigv4Name 'bedrock', endpointPrefix 'bedrock-runtime', protocols ['restJson1'], common_errors [AccessDeniedException, InternalServerException, ThrottlingException, ValidationException]; shape query gives ValidationException httpError 400, ResourceNotFoundException httpError 404, AccessDeniedException httpError 403. The control-plane model (services/service-com-amazonaws-bedrock-amazonbedrockcontrolplaneservice/...) declares the SAME sigv4Name 'bedrock' with endpointPrefix 'bedrock' — so the signing name cannot key service resolution, while the endpoint prefix can.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it.
2. Replay trace set simparity-wire-aws-v1.
3. Representative step: fixture 'bedrock-runtime-apply-guardrail-error-paths', step 'task#1' — POST /guardrail/{guardrailIdentifier}/version/{guardrailVersion}/apply, signed for sigv4 service name 'bedrock', with input the guardrail cannot accept. Expected HTTP 400, content-type application/json, x-amzn-errortype 'ValidationException:http://internal.amazon.com/coral/com.amazon.bedrock/'; observed HTTP 500, content-type application/x-amz-json-1.1, x-amzn-errortype 'AmbiguousService'.
4. Repeat for the other nine causes, listed in trace_ids as fixture#step: ListAsyncInvokes 200 (bedrock-runtime-async-invoke-lifecycle-and-errors#task#2), ListAsyncInvokes maxResults 400 (#task#3), GetAsyncInvoke malformed ARN 400 (#task#4), GetAsyncInvoke cross-account 403 (#task#5), StartAsyncInvoke s3Uri 400 (bedrock-runtime-start-async-invoke-invalid-s3-uri#task#1), CountTokens unsupported-model 400 (bedrock-runtime-count-tokens-before-invoke#task#1), CountTokens unknown-model 404 (bedrock-runtime-count-tokens-error-unknown-model#task#1), InvokeGuardrailChecks 200 (bedrock-runtime-guardrail-checks-inline#task#1), InvokeGuardrailChecks enum 400 (bedrock-runtime-guardrail-checks-inline#task#2).
5. All ten answer 500 AmbiguousService. Resolve requests by endpoint prefix / host and URI path instead of the SigV4 signing name, then re-replay all ten keys — each must reach its own handler and produce the status, error code and modeled members above.
