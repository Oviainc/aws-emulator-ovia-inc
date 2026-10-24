Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bedrock-runtime.async-invoke-appears-in-list-after-creation bedrock-runtime.async-invoke-invalid-output-config-rejected bedrock-runtime.async-invoke-output-uri-rewritten-with-invocation-id bedrock-runtime.async-invoke-status-filter-partitions-collection bedrock-runtime.get-async-invoke-malformed-arn-rejected-distinctly-from-absent-job bedrock-runtime.async-invoke-schema-gap-rejected-only-after-accept bedrock-runtime.start-async-invoke-rejects-destination-before-model-input bedrock-runtime.async-invoke-lifecycle-transitions bedrock-runtime.async-invoke-output-retention-window bedrock-runtime.async-invoke-output-written-to-configured-destination-serverside bedrock-runtime.async-invoke-terminal-state-immutable bedrock-runtime.async-invoke-timestamp-precision-round-trip bedrock-runtime.e2e-async-invoke-full-lifecycle bedrock-runtime__s3.bedrock-runtime-async-invoke-writes-a-playable-video-artifact-to-s3 bedrock-runtime__s3.bedrock-runtime-async-invoke-with-a-nonexistent-output-bucket-rejected bedrock-runtime.async-invoke-client-token-reuse-never-conflicts bedrock-runtime.start-async-invoke-lifecycle-and-idempotency bedrock-runtime.start-async-invoke-token-reuse-conflicting-params-blocked bedrock-runtime.cross-account-async-invoke-not-visible
Issue-Key: bedrock-runtime|StartAsyncInvoke|ValidationException
Issue-Key: bedrock-runtime.async-invoke-schema-gap-rejected-only-after-accept|run#11|AssertionError|assert message == (
Issue-Key: bedrock-runtime.start-async-invoke-rejects-destination-before-model-input|run#11|AssertionError|assert writable_message == _SCHEMA_MESSAGE, (
Issue-Key: bedrock-runtime.async-invoke-lifecycle-transitions|run#8|AssertionError|assert final["status"] == "Completed", (
Issue-Key: bedrock-runtime.async-invoke-output-retention-window|run#4|AssertionError|assert final["status"] == "Completed", (
Issue-Key: bedrock-runtime.async-invoke-output-written-to-configured-destination-serverside|run#13|AssertionError|assert final["status"] == "Completed", f"video generation did not complete: {final}"
Issue-Key: bedrock-runtime.async-invoke-terminal-state-immutable|run#4|AssertionError|assert final["status"] == "Completed", (
Issue-Key: bedrock-runtime.async-invoke-timestamp-precision-round-trip|run#17|AssertionError|assert final is not None and final["status"] == "Completed", f"job did not complete: {final}"
Issue-Key: bedrock-runtime.e2e-async-invoke-full-lifecycle|run#4|AssertionError|assert final["status"] == "Completed", (
Issue-Key: bedrock-runtime__s3.bedrock-runtime-async-invoke-writes-a-playable-video-artifact-to-s3|run#18|AssertionError|assert job["status"] == "Completed", (
Issue-Key: bedrock-runtime__s3.bedrock-runtime-async-invoke-with-a-nonexistent-output-bucket-rejected|run#17|AssertionError|assert job["status"] == "Completed", (
Issue-Key: bedrock-runtime|StartAsyncInvoke|ConflictException
Issue-Key: bedrock-runtime.start-async-invoke-token-reuse-conflicting-params-blocked|run#13|AssertionError|assert message == (
Issue-Key: bedrock-runtime.cross-account-async-invoke-not-visible|run#21|AssertionError|assert own_message == 'jobArn: {} is not found.', f"unexpected refusal message {own_message!r}"
Issue-Commit: a61b7e3a8f074f2acfd74e18fad3041c10471e17

# bedrock-runtime async invoke: StartAsyncInvoke over-validates modelInput, jobs never reach Completed, clientRequestToken reuse wrongly conflicts

> _Note: this issue was identified against the code base at commit `a61b7e3a8f074f2acfd74e18fad3041c10471e17`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartAsyncInvoke

## Description
This issue bundles 6 distinct root causes in the bedrock-runtime asynchronous-invocation surface (StartAsyncInvoke / GetAsyncInvoke / ListAsyncInvokes). A fixer must close all 6 before submitting.

Smithy consulted (services/service-com-amazonaws-bedrockruntime-amazonbedrockfrontendservice/smithy-model/com-amazonaws-bedrockruntime-amazonbedrockfrontendservice.json): StartAsyncInvoke POST /async-invoke -> 200, input modelId/modelInput(document, @sensitive)/outputDataConfig required, clientRequestToken @idempotencyToken length 1..256 pattern ^[!-~]*$; errors AccessDeniedException 403, ConflictException 400, InternalServerException 500, ResourceNotFoundException 404, ServiceQuotaExceededException 400, ServiceUnavailableException 503, ThrottlingException 429, ValidationException 400; operation is @idempotent. GetAsyncInvoke GET /async-invoke/{invocationArn} -> 200 with required status (enum AsyncInvokeStatus = InProgress|Completed|Failed), optional failureMessage, required outputDataConfig; its modeled errors are ONLY AccessDeniedException/InternalServerException/ThrottlingException/ValidationException (no ResourceNotFoundException).

---
Cause 1 of 6: StartAsyncInvoke synchronously rejects a modelInput that merely OMITS a declared required key.
AWS: recorded behavior only (the model types modelInput as an opaque `document`, so the per-model JSON schema is not in the Smithy model). For luma.ray-v2:0 a modelInput missing the declared `prompt` key (including an empty document `{}`) is ACCEPTED with HTTP 200 and a minted invocationArn; the refusal surfaces later as GetAsyncInvoke status=Failed with a failureMessage. Only an UNDECLARED key is refused synchronously.
Emulator: the request is refused synchronously with HTTP 400 ValidationException, message e.g. "Malformed input request: #: required key [prompt] not found, reformat your input and try again." No job is minted, so every downstream read (GetAsyncInvoke, ListAsyncInvokes) has nothing to observe.
Why: a client that relies on StartAsyncInvoke accepting the job and reporting the fault asynchronously never gets an invocationArn to poll; any caller or test harness that mints a job and then exercises list/get/filter semantics cannot proceed at all. It also inverts the two rejection surfaces of one operation, so callers attribute an async model-side fault to their own request syntax.
Fix: the StartAsyncInvoke handler's modelInput validation for the async model-input schema — it must distinguish "undeclared property" (synchronous ValidationException) from "missing required property" (accept, then terminal Failed + failureMessage).
Traces: bedrock-runtime.async-invoke-appears-in-list-after-creation, bedrock-runtime.async-invoke-invalid-output-config-rejected, bedrock-runtime.async-invoke-output-uri-rewritten-with-invocation-id, bedrock-runtime.async-invoke-status-filter-partitions-collection, bedrock-runtime.get-async-invoke-malformed-arn-rejected-distinctly-from-absent-job, bedrock-runtime.async-invoke-schema-gap-rejected-only-after-accept.

---
Cause 2 of 6: the ValidationException raised for an UNDECLARED modelInput key names the wrong defect.
AWS: recorded behavior only. HTTP 400, code ValidationException, and the message is built from the request — it names the offending property and the additional-properties rule, e.g. "Invalid JSON: : property 'sim2realBogusKey' is not defined in the schema and the schema does not allow additional properties ". The requirement is that rule (offending member + additionalProperties violation), not the recorded text.
Emulator: HTTP 400 ValidationException with "Malformed input request: #: required key [prompt] not found, reformat your input and try again." — it reports a missing-required-key violation for a request whose actual fault is an undeclared key, and never names the member the caller sent.
Why: the caller is told to add a key it did not get wrong instead of being told which key it sent is not in the schema, so the request cannot be corrected from the response.
Fix: the StartAsyncInvoke handler's modelInput schema validator and its error-message construction.
Traces: bedrock-runtime.start-async-invoke-rejects-destination-before-model-input, bedrock-runtime.async-invoke-schema-gap-rejected-only-after-accept.

---
Cause 3 of 6: output-destination validation does not short-circuit ahead of modelInput validation.
AWS: recorded behavior only. With one request carrying two faults (a schema-invalid modelInput AND an s3Uri whose bucket does not exist), AWS reports only the destination fault: HTTP 400 ValidationException with message "Invalid S3 credentials". With the same modelInput and a writable destination it reports the model's schema complaint instead — so destination validation runs first.
Emulator: both requests are answered with the same modelInput schema complaint; the unwritable destination is invisible in the response.
Why: a caller whose only real problem is a missing or unwritable output bucket is sent to debug its prompt payload, and an operator cannot distinguish a permissions/destination problem from a payload problem.
Fix: ordering inside the StartAsyncInvoke handler's validation stage — resolve/authorize outputDataConfig.s3OutputDataConfig before validating modelInput.
Traces: bedrock-runtime.start-async-invoke-rejects-destination-before-model-input.

---
Cause 4 of 6: asynchronous invocations never run — every job goes terminal Failed and nothing is written to the configured S3 destination.
AWS: GetAsyncInvoke.status is the modeled enum InProgress|Completed|Failed. For an accepted luma.ray-v2:0 job AWS transitions InProgress -> Completed (recorded wall clock ~71 s) and writes the invocation's artifacts under the configured s3Uri at out/<invocationId>/ (a manifest.json plus the output artifact); failureMessage is absent on the success path.
Emulator: the job is minted and then immediately moves to status='Failed' with failureMessage='This Amazon Bedrock Runtime emulator does not run model inference for asynchronous invocations, so no output was written to the configured S3 location.' endTime is set within ~0.5 s of submitTime and no object appears under the configured s3Uri.
Why: Completed is never reachable, so the whole async state machine is unobservable: lifecycle transitions, terminal-state immutability, output-retention windows, status partitioning in ListAsyncInvokes, submit/lastModified/end timestamp ordering, and the server-side write to the caller's bucket all collapse. Any client that waits for Completed or reads the output object fails against the emulator while succeeding on AWS, and a check that uses an accepted job as its control cannot attribute its real assertion.
Fix: the async-invoke job state machine and its output writer (bedrock-runtime async-invoke state + the worker that advances a job and writes to outputDataConfig). A synthetic artifact plus a Completed transition is sufficient; the status enum and the S3 write are the requirement, not the artifact's bytes.
Traces: bedrock-runtime.async-invoke-lifecycle-transitions, bedrock-runtime.async-invoke-output-retention-window, bedrock-runtime.async-invoke-output-written-to-configured-destination-serverside, bedrock-runtime.async-invoke-terminal-state-immutable, bedrock-runtime.async-invoke-timestamp-precision-round-trip, bedrock-runtime.e2e-async-invoke-full-lifecycle, bedrock-runtime__s3.bedrock-runtime-async-invoke-writes-a-playable-video-artifact-to-s3, bedrock-runtime__s3.bedrock-runtime-async-invoke-with-a-nonexistent-output-bucket-rejected.

---
Cause 5 of 6: clientRequestToken reuse raises ConflictException where AWS accepts the call.
AWS: clientRequestToken is @idempotencyToken and StartAsyncInvoke is @idempotent. Recorded behavior (three independent replays): reusing a token with a DIFFERENT modelInput is accepted with HTTP 200 every time and never raises ConflictException; the invocationArn it returns resolves on an independent GetAsyncInvoke and is InProgress or Completed, and the original job remains resolvable. Reusing a token with an IDENTICAL modelInput is a true idempotent replay that returns the ORIGINAL job's invocationArn without minting a second job. ConflictException is modeled (400) but is not what this path produces.
Emulator: the second StartAsyncInvoke on a reused token is refused with HTTP 400 ConflictException, message e.g. 'The clientRequestToken <token> was already used for a different request.' — raised even for the mismatched-input case AWS accepts.
Why: an idempotent retry path (the normal client-side reaction to a timeout or a retry wrapper) turns into a hard 400 instead of either a replay or a fresh accepted job, so a correct client that supplies its own token cannot start work at all.
Where a ConflictException IS correct AWS's message is the fixed string "Received a request with a client request token that has been used for a different input."; the emulator's differs, which is recorded behavior only and secondary to the wrong outcome above.
Fix: clientRequestToken handling in the StartAsyncInvoke handler and the async-invoke state's token index — same token + same input must replay the original invocationArn; same token + different input must be accepted, not conflicted.
Traces: bedrock-runtime.async-invoke-client-token-reuse-never-conflicts, bedrock-runtime.start-async-invoke-lifecycle-and-idempotency, bedrock-runtime.start-async-invoke-token-reuse-conflicting-params-blocked.

---
Cause 6 of 6: the absent/foreign-job refusal from GetAsyncInvoke carries a different message.
AWS: recorded behavior only. GetAsyncInvoke on an invocationArn that does not resolve for the caller (including a well-formed ARN belonging to another account) is refused with the same error code the emulator already returns, and the recorded message is the fixed template 'jobArn: {} is not found.' (AWS emits the literal braces). Note GetAsyncInvoke models no ResourceNotFoundException, so the refusal is a 400-class ValidationException per the model.
Emulator: the error code matches, but the message is 'The invocation ARN arn:aws:bedrock:<region>:<account>:async-invoke/<id> does not exist.' — a different template that also echoes the requested ARN back.
Why: lowest-impact of this bundle; a client or test that pins the not-found message (or treats the echoed ARN as confirmation the ARN was parsed) reads a different response. Status and code are unaffected, so this is reported as a recorded-message divergence, not a status/code defect.
Fix: the not-found error construction in the GetAsyncInvoke handler.
Traces: bedrock-runtime.cross-account-async-invoke-not-visible.

## Evidence
Cause 1 — StartAsyncInvoke(modelId='luma.ray-v2:0', modelInput minimal/empty, outputDataConfig=s3://<bucket>/out/)
  Expected (AWS): HTTP 200, {"invocationArn":"arn:aws:bedrock:<region>:<acct>:async-invoke/<12 lowercase alnum>"}; fault reported later via GetAsyncInvoke status=Failed + failureMessage.
  Actual: botocore.errorfactory.ValidationException: An error occurred (ValidationException) when calling the StartAsyncInvoke operation: Malformed input request: #: required key [prompt] not found, reformat your input and try again.

Cause 2 — StartAsyncInvoke(modelInput={"sim2realBogusKey": 1}, writable destination)
  Expected (AWS): 400 ValidationException naming the undeclared member, recorded example "Invalid JSON: : property 'sim2realBogusKey' is not defined in the schema and the schema does not allow additional properties ".
  Actual: 400 ValidationException, message 'Malformed input request: #: required key [prompt] not found, reformat your input and try again.'

Cause 3 — same modelInput={"sim2realBogusKey":1} but s3Uri pointing at a bucket that does not exist
  Expected (AWS): 400 ValidationException, message 'Invalid S3 credentials' (destination fault pre-empts the schema fault; the two legs must NOT report the same message).
  Actual: identical schema message as the writable-destination leg — 'Malformed input request: #: required key [prompt] not found, reformat your input and try again.'

Cause 4 — GetAsyncInvoke on an accepted luma.ray-v2:0 job
  Expected (AWS): status transitions InProgress -> 'Completed', endTime > submitTime by the render duration, and objects written under the configured s3Uri (manifest + artifact).
  Actual: {'invocationArn': 'arn:aws:bedrock:us-west-2:000000000000:async-invoke/16c7f70f55b0', 'status': 'Failed', 'failureMessage': 'This Amazon Bedrock Runtime emulator does not run model inference for asynchronous invocations, so no output was written to the configured S3 location.', 'submitTime': 2026-10-24T09:41:45.305Z, 'endTime': 2026-10-24T09:41:45.805Z, 'outputDataConfig': {'s3OutputDataConfig': {'s3Uri': 's3://sim2real-behav-async-out-.../video-out'}}} — and zero objects under that prefix.

Cause 5 — StartAsyncInvoke twice with the same clientRequestToken
  Expected (AWS): different modelInput -> HTTP 200, a resolvable invocationArn (InProgress|Completed), original job untouched; identical modelInput -> HTTP 200 returning the ORIGINAL invocationArn.
  Actual: botocore.errorfactory.ConflictException: An error occurred (ConflictException) when calling the StartAsyncInvoke operation: The clientRequestToken sim2real-behav-async-a4cea87e5fd9 was already used for a different request.
  Also, where a conflict is correct: expected message 'Received a request with a client request token that has been used for a different input.'; actual 'The clientRequestToken sim2real-conflict-7e6a46681c7a was already used for a different request.'

Cause 6 — GetAsyncInvoke on an ARN that does not resolve for the caller
  Expected (AWS): message 'jobArn: {} is not found.' (code matches and is unchanged).
  Actual: message 'The invocation ARN arn:aws:bedrock:us-east-1:000000000000:async-invoke/aaaaaaaaaaaa does not exist.'

## Steps to Reproduce
1. git_clone, then run_behavioral_suite against the booted emulator.
2. Cause 1: s3.create_bucket(<out-bucket>) then bedrock-runtime.start_async_invoke(modelId='luma.ray-v2:0', modelInput={}, outputDataConfig={'s3OutputDataConfig':{'s3Uri':'s3://<out-bucket>/out/'}}). Observe a synchronous 400 ValidationException instead of a 200 with an invocationArn.
3. Cause 2/3: call start_async_invoke with modelInput={'sim2realBogusKey': 1} twice — once with the writable bucket above and once with an s3Uri naming a bucket that does not exist. Compare the two Error.Message values: AWS gives the additionalProperties complaint then 'Invalid S3 credentials'; the emulator gives the same required-key message both times.
4. Cause 4: start a well-formed job (modelInput={'prompt':'A single blue cube rotating slowly on a white background.','aspect_ratio':'16:9','loop':False,'duration':'5s','resolution':'540p'}), poll get_async_invoke(invocationArn=...) to a terminal state, and list_objects_v2 on the output bucket. Observe status='Failed' with the emulator's failureMessage and zero objects.
5. Cause 5: call start_async_invoke twice with the same clientRequestToken — once with a different modelInput, once with an identical one. Observe ConflictException in both cases instead of 200 / idempotent replay.
6. Cause 6: get_async_invoke(invocationArn='arn:aws:bedrock:us-east-1:000000000000:async-invoke/aaaaaaaaaaaa') and read Error.Message.
