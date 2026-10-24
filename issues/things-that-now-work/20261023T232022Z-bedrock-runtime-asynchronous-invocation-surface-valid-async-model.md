Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bedrock-runtime.async-invoke-appears-in-list-after-creation bedrock-runtime.async-invoke-client-token-reuse-never-conflicts bedrock-runtime.async-invoke-lifecycle-transitions bedrock-runtime.async-invoke-list-sorted-by-submission-time bedrock-runtime.async-invoke-output-uri-rewritten-with-invocation-id bedrock-runtime.async-invoke-output-written-to-configured-destination-serverside bedrock-runtime.async-invoke-status-filter-partitions-collection bedrock-runtime.async-invoke-timestamp-precision-round-trip bedrock-runtime.concurrent-start-async-invoke-distinct-tokens-unique-arns bedrock-runtime.get-async-invoke-malformed-arn-rejected-distinctly-from-absent-job bedrock-runtime.start-async-invoke-lifecycle-and-idempotency bedrock-runtime.start-async-invoke-rejects-destination-before-model-input bedrock-runtime.async-invoke-schema-gap-rejected-only-after-accept bedrock-runtime__s3.bedrock-runtime-async-invoke-with-a-nonexistent-output-bucket-rejected bedrock-runtime__s3.bedrock-runtime-async-invoke-writes-a-playable-video-artifact-to-s3 bedrock-runtime.async-invoke-invalid-duration-rejected bedrock-runtime.start-async-invoke-unsupported-model-wrong-state-rejected bedrock-runtime.async-invoke-output-retention-window bedrock-runtime.async-invoke-terminal-state-immutable bedrock-runtime.e2e-async-invoke-full-lifecycle bedrock-runtime.async-invoke-invalid-output-config-rejected
Issue-Key: bedrock-runtime|StartAsyncInvoke|ValidationException
Issue-Key: bedrock-runtime.start-async-invoke-rejects-destination-before-model-input|run#11|AssertionError|assert writable_message == _SCHEMA_MESSAGE, (
Issue-Key: bedrock-runtime.async-invoke-schema-gap-rejected-only-after-accept|run#11|AssertionError|assert message == (
Issue-Key: bedrock-runtime.async-invoke-invalid-duration-rejected|_error#9|AssertionError|raise AssertionError(f"call was expected to be refused but SUCCEEDED and returned {result!r}")
Issue-Key: bedrock-runtime.start-async-invoke-unsupported-model-wrong-state-rejected|_error_code#3|AssertionError|raise AssertionError(
Issue-Key: bedrock-runtime.async-invoke-output-retention-window|run#4|AssertionError|assert final["status"] == "Completed", (
Issue-Key: bedrock-runtime.async-invoke-terminal-state-immutable|run#4|AssertionError|assert final["status"] == "Completed", (
Issue-Key: bedrock-runtime.e2e-async-invoke-full-lifecycle|run#4|AssertionError|assert final["status"] == "Completed", (
Issue-Key: bedrock-runtime.async-invoke-invalid-output-config-rejected|run#6|AssertionError|assert message == "1 validation error detected: Value 'not-a-valid-s3-uri' at 'outputDataConfig.s3OutputDataConfig.s3Uri' failed to satisfy constraint: Member m
Issue-Commit: cd7a0e788c437c5439a918b59f215cef2324a883

# bedrock-runtime asynchronous-invocation surface: valid async model refused, no modelInput/capability validation, jobs always terminate Failed

> _Note: this issue was identified against the code base at commit `cd7a0e788c437c5439a918b59f215cef2324a883`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartAsyncInvoke

## Description
FIVE CAUSES on the StartAsyncInvoke / GetAsyncInvoke / ListAsyncInvokes surface of the Bedrock Runtime emulator. Each is a distinct contract a fixer must see answer correctly. (Smithy model read for all five: services/service-com-amazonaws-bedrockruntime-amazonbedrockfrontendservice/smithy-model/com-amazonaws-bedrockruntime-amazonbedrockfrontendservice.json.)

Cause 1 of 5 - StartAsyncInvoke refuses an async-capable model id as malformed.
AWS: Smithy StartAsyncInvoke -> POST /async-invoke; `modelId` targets AsyncInvokeIdentifier with @length 1..256 and @pattern ^[a-zA-Z_\.\-/0-9:]+$ - the id `luma.ray-v2:0` satisfies that pattern, and the recorded behavior of the suite's account is that luma.ray-v2:0 is the VIDEO-output model catalogued in the suite region (recorded behavior only; the checks state it was measured via ListFoundationModels). The modeled success is 200 with `invocationArn` matching ^arn:aws(-[^:]+)?:bedrock:[a-z0-9-]{1,20}:[0-9]{12}:async-invoke/[a-z0-9]{12}$.
Emulator: 400 ValidationException, code `ValidationException`, message example 'The provided model identifier is invalid.' for every StartAsyncInvoke that names luma.ray-v2:0 - i.e. the emulator's async model catalogue does not contain the one model the region offers, so NO asynchronous invocation can be created at all.
Why: a client cannot start any async job; every async workflow (video generation, batch inference) is unreachable, and the refusal is a terminal 400 so the client will not retry.
Fix: the StartAsyncInvoke handler's model-identifier resolution / the async-capable model catalogue it consults.
Traces: bedrock-runtime.async-invoke-appears-in-list-after-creation, bedrock-runtime.async-invoke-client-token-reuse-never-conflicts, bedrock-runtime.async-invoke-lifecycle-transitions, bedrock-runtime.async-invoke-list-sorted-by-submission-time, bedrock-runtime.async-invoke-output-uri-rewritten-with-invocation-id, bedrock-runtime.async-invoke-output-written-to-configured-destination-serverside, bedrock-runtime.async-invoke-status-filter-partitions-collection, bedrock-runtime.async-invoke-timestamp-precision-round-trip, bedrock-runtime.concurrent-start-async-invoke-distinct-tokens-unique-arns, bedrock-runtime.get-async-invoke-malformed-arn-rejected-distinctly-from-absent-job, bedrock-runtime.start-async-invoke-lifecycle-and-idempotency, bedrock-runtime.start-async-invoke-rejects-destination-before-model-input, bedrock-runtime.async-invoke-schema-gap-rejected-only-after-accept, bedrock-runtime__s3.bedrock-runtime-async-invoke-with-a-nonexistent-output-bucket-rejected, bedrock-runtime__s3.bedrock-runtime-async-invoke-writes-a-playable-video-artifact-to-s3.

Cause 2 of 5 - StartAsyncInvoke does not validate `modelInput` against the model's own schema (OVER-PERMISSIVE).
AWS: `modelInput` is a required @sensitive document; the provider validates it and refuses a value outside the model's enum with ValidationException (Smithy: ValidationException errorKind client, httpError 400). Recorded message example for Nova Reel with durationSeconds=7: 'Malformed input request: #/videoGenerationConfig/durationSeconds: 7 is not a valid enum value, reformat your input and try again.' The status (400) and the code (ValidationException) are the requirement; the text is an example built from the offending member.
Emulator: 200 with a freshly minted `invocationArn` - the out-of-enum duration is accepted and a job is created.
Why: a caller's malformed job is accepted, billed and only discovered to be wrong much later (or never), instead of being refused synchronously; and a rejected request must mint no job, so ListAsyncInvokes now contains a job AWS would never have created.
Fix: StartAsyncInvoke input validation (modelInput schema check, before any job is minted).
Traces: bedrock-runtime.async-invoke-invalid-duration-rejected.

Cause 3 of 5 - StartAsyncInvoke does not check that the model supports asynchronous invocation (OVER-PERMISSIVE).
AWS: a plain on-demand text model is ineligible for StartAsyncInvoke and is refused with ValidationException (400); recorded message example 'The provided model doesn't support async inference.' (status + code are the requirement).
Emulator: 200 with invocationArn example 'arn:aws:bedrock:us-west-2:000000000000:async-invoke/4b925499ed88' for modelId amazon.nova-lite-v1:0 - a text-only model. Note the inversion against cause 1: the model that IS async-capable is refused, and a model that is NOT async-capable is accepted.
Why: the capability boundary is invisible to clients, so an unsupported model silently enters the async pipeline instead of being rejected at submit time.
Fix: StartAsyncInvoke's model capability gate (output-modality / async-inference eligibility), in the same handler as cause 1.
Traces: bedrock-runtime.start-async-invoke-unsupported-model-wrong-state-rejected.

Cause 4 of 5 - every accepted async invocation terminates `Failed`; `Completed` is unreachable and no output object is written.
AWS: Smithy GetAsyncInvoke returns required `status` (AsyncInvokeStatus) plus required `outputDataConfig`; a well-formed job progresses InProgress -> Completed and the service writes its artifact under the configured s3Uri (recorded behavior: the checks measured ~71 s to Completed for this model). Terminal state is then immutable.
Emulator: status goes to `Failed` with failureMessage example 'This Amazon Bedrock Runtime emulator does not run model inference for asynchronous invocations, so no output was written to the configured S3 location.' The divergence filed here is the STATE MACHINE and the presence of an output object, not the artifact's content - the generated bytes are model output and are not the requirement.
Why: any client that polls GetAsyncInvoke/ListAsyncInvokes for Completed loops forever or takes its failure path; a job that AWS completes is reported as a permanent failure.
Fix: the async-invoke state machine / its worker (status transition to Completed and the write to outputDataConfig.s3OutputDataConfig.s3Uri).
Traces: bedrock-runtime.async-invoke-output-retention-window, bedrock-runtime.async-invoke-terminal-state-immutable, bedrock-runtime.e2e-async-invoke-full-lifecycle.

Cause 5 of 5 - the s3Uri constraint-violation message renders the regex WITH ^...$ anchors (LOW).
AWS: the refusal is ValidationException 400 and its message is built from the request and the member's @pattern, so the RULE is the requirement: "1 validation error detected: Value '<value>' at 'outputDataConfig.s3OutputDataConfig.s3Uri' failed to satisfy constraint: Member must satisfy regular expression pattern: s3://[a-z0-9][\.\-a-z0-9]{1,61}[a-z0-9](/.*)?" - the pattern is rendered unanchored. (The same rule is already recorded in open wire issue 20261022T073319Z-bedrock-runtime-guardrail-async-invoke-and-count-tokens.md, cause 6 of 10; that issue covers the router returning 500 for this call, NOT the message rendering, so it does not cover this.)
Emulator: correct status 400 and correct code ValidationException, but the message renders '^s3://[a-z0-9][\.\-a-z0-9]{1,61}[a-z0-9](/.*)?$' - the anchors leak into the client-facing text.
Why: a client or test that matches the documented constraint text does not match; the member and constraint are otherwise correct, hence LOW.
Fix: the constraint-violation message formatter (strip regex anchors when rendering a @pattern into a validation message).
Traces: bedrock-runtime.async-invoke-invalid-output-config-rejected.

## Evidence
Behavioral suite simparity-behavioral-ga-prod-v1, 13/92 passed. The emulator was serving (bootstrap_signal false; other bedrock-runtime checks got 200s), so these are fidelity findings.

Cause 1 - repeated verbatim across 15 checks:
  botocore.errorfactory.ValidationException: An error occurred (ValidationException) when calling the StartAsyncInvoke operation: The provided model identifier is invalid.
  (modelId sent: luma.ray-v2:0; also surfaced as 'synchronous schema rejection carried an unexpected message: The provided model identifier is invalid.' in bedrock-runtime.async-invoke-schema-gap-rejected-only-after-accept and as "positive control: expected the model's schema complaint, got 'The provided model identifier is invalid.'" in bedrock-runtime.start-async-invoke-rejects-destination-before-model-input)

Cause 2 - bedrock-runtime.async-invoke-invalid-duration-rejected:
  AssertionError: call was expected to be refused but SUCCEEDED and returned {'ResponseMetadata': {'HTTPStatusCode': 200, ...}, 'invocationArn': 'arn:aws:bedrock:us-east-1:000000000000:async-invoke/7bad962d3d39'}
  (request: modelId amazon.nova-reel-v1:0, modelInput.videoGenerationConfig.durationSeconds = 7, a valid S3 destination; the only broken input was the duration)

Cause 3 - bedrock-runtime.start-async-invoke-unsupported-model-wrong-state-rejected:
  AssertionError: the call was expected to be refused but SUCCEEDED, which is an over-permissiveness finding, not a code mismatch; response={'ResponseMetadata': {'HTTPStatusCode': 200, ...}, 'invocationArn': 'arn:aws:bedrock:us-west-2:000000000000:async-invoke/4b925499ed88'}
  (request: modelId amazon.nova-lite-v1:0 - text-only)

Cause 4 - three checks, identical:
  AssertionError: expected terminal Completed status, got Failed (This Amazon Bedrock Runtime emulator does not run model inference for asynchronous invocations, so no output was written to the configured S3 location.)

Cause 5 - bedrock-runtime.async-invoke-invalid-output-config-rejected:
  expected message "... Member must satisfy regular expression pattern: s3://[a-z0-9][\.\-a-z0-9]{1,61}[a-z0-9](/.*)?"
  observed message "... Member must satisfy regular expression pattern: ^s3://[a-z0-9][\.\-a-z0-9]{1,61}[a-z0-9](/.*)?$"

Model evidence: smithy_query operation StartAsyncInvoke/GetAsyncInvoke on services/service-com-amazonaws-bedrockruntime-amazonbedrockfrontendservice/smithy-model/com-amazonaws-bedrockruntime-amazonbedrockfrontendservice.json - StartAsyncInvoke input {clientRequestToken, modelId (required, pattern ^[a-zA-Z_\.\-/0-9:]+$), modelInput (required document), outputDataConfig (required), tags}, output {invocationArn required}, errors include ValidationException(400) and ConflictException(400); GetAsyncInvoke output requires {invocationArn, modelArn, status, submitTime, outputDataConfig}.

## Steps to Reproduce
1. git_clone the repo and let the behavioral suite build and boot the emulator.
2. Run the behavioral suite and re-replay the check ids in trace_ids.
3. Cause 1: call StartAsyncInvoke with modelId='luma.ray-v2:0', modelInput={'prompt': '...'}, outputDataConfig={'s3OutputDataConfig':{'s3Uri':'s3://<bucket>/out/'}}. Expected 200 with an invocationArn matching the modeled ARN pattern; observed 400 ValidationException 'The provided model identifier is invalid.'
4. Cause 2: call StartAsyncInvoke with modelId='amazon.nova-reel-v1:0' in us-east-1 and modelInput.videoGenerationConfig.durationSeconds=7. Expected 400 ValidationException naming the bad member; observed 200 and a new invocationArn. Then ListAsyncInvokes must show no new job.
5. Cause 3: call StartAsyncInvoke with modelId='amazon.nova-lite-v1:0' (text-only). Expected 400 ValidationException; observed 200 and a new invocationArn.
6. Cause 4: start any accepted async job and poll GetAsyncInvoke. Expected status to reach 'Completed' with an object written under the configured s3Uri; observed 'Failed'.
7. Cause 5: call StartAsyncInvoke with outputDataConfig.s3OutputDataConfig.s3Uri='not-a-valid-s3-uri'. Expected 400 ValidationException whose message renders the pattern unanchored; observed the same status/code with '^...$' in the rendered pattern.
