Issue-Source: trace-sweep-divergence
Issue-Trace: bedrock-data-automation-blueprint-optimization-invoke
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: bedrock-data-automation-blueprint-optimization-invoke
Issue-Shape-Id: com.amazonaws.bedrockdataautomation#AmazonBedrockKeystoneBuildTimeService
Issue-Key: bedrock-data-automation|InvokeBlueprintOptimizationAsync|4xx->4xx|$.message
Issue-Key: bedrock-data-automation|InvokeBlueprintOptimizationAsync|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: bedrock-data-automation|InvokeBlueprintOptimizationAsync|4xx->4xx|status
Issue-Commit: 07e872c7593558b582731e73c173338530c40226

# InvokeBlueprintOptimizationAsync does not validate request S3 URIs, refusing a bad-S3-URI submission as a missing blueprint

> _Note: this issue was identified against the code base at commit `07e872c7593558b582731e73c173338530c40226`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
InvokeBlueprintOptimizationAsync

## Description
One net-new cause in the bedrock-data-automation (Keystone build-time) emulator, found by replaying the wire corpus simparity-wire-aws-v1. Every other divergence this replay produced for this service is already owned by an open issue or by a scaffold-stub issue and is NOT re-filed here.

=== Cause 1 of 1: an InvokeBlueprintOptimizationAsync submission carrying an unusable S3 location is refused 404 ResourceNotFoundException instead of 400 ValidationException, because the operation performs no S3-URI validation at all ===

AWS: Smithy (services/service-com-amazonaws-bedrockdataautomation-amazonbedrockkeystonebuildtimeservice/smithy-model/com-amazonaws-bedrockdataautomation-amazonbedrockkeystonebuildtimeservice.json): InvokeBlueprintOptimizationAsync is POST /invokeBlueprintOptimizationAsync, success 200; its input requires `blueprint` (BlueprintOptimizationObject), `samples` (BlueprintOptimizationSamples, each BlueprintOptimizationSample requiring assetS3Object and groundTruthS3Object), `outputConfiguration` (BlueprintOptimizationOutputConfiguration, requiring s3Object) and `dataAutomationProfileArn`. Every one of those S3 locations is an `S3Object` whose required `s3Uri` targets S3Uri @length(1..1024) @pattern ^s3://[a-z0-9][\.\-a-z0-9]{1,61}[a-z0-9](/.*)?$. The operation models both ValidationException (errorKind client, @httpError 400, doc "This exception is thrown when the request's input validation fails") and ResourceNotFoundException (errorKind client, @httpError 404, doc "This exception is thrown when a resource referenced by the operation does not exist"). Recorded behavior: a submission whose S3 URI satisfies the modelled pattern but is not a usable S3 location is refused with HTTP 400, error code ValidationException and the message "Request has invalid S3 URI" — the message is entirely FIXED text with NO variable part (AWS does not echo the offending URI, the member path, or the bucket/key), and the refusal is decided for the request's S3 locations ahead of resolving the referenced blueprint, so it is the answer even when the blueprint reference would also be in question. The recorded response also carries the header x-amzn-errortype as "ValidationException:http://internal.amazon.com/coral/com.amazon.bedrock.keystone.build/" — the shape name followed by this service's Coral namespace suffix; the shape name is the variable part, the ":http://internal.amazon.com/coral/com.amazon.bedrock.keystone.build/" suffix is fixed.

Emulator: the same submission is answered HTTP 404 with x-amzn-errortype "ResourceNotFoundException" (bare shape name, no Coral namespace suffix) and the body message "Blueprint Arn not found.". No S3-URI check exists anywhere on this path: the handler's only pre-mint refusals are a blueprint-resolution check and a job-count quota check, after which it mints an invocationArn and persists the job. So the S3 locations in `samples[*].assetS3Object`, `samples[*].groundTruthS3Object` and `outputConfiguration.s3Object` are never examined beyond the framework's modelled @pattern/@length walk, and a submission AWS refuses 400 is either mis-refused 404 (as recorded here) or — once the blueprint does resolve — ACCEPTED with 200 and a minted invocationArn, a false success for a job that can never produce output.

Why: the status class and the error code both change meaning. A client that reads 400/ValidationException as "my request is wrong, fix the S3 location and do not retry" and 404/ResourceNotFoundException as "the blueprint is gone, re-create or re-point it" is sent down the wrong branch entirely: it will go hunting for a blueprint that exists while the real fault — the output/sample bucket or key it passed — is never surfaced. Worse, in the accepting case the SDK returns success, so the caller records an in-flight optimization job and then polls GetBlueprintOptimizationStatus forever for an output object that was never writable. The bare x-amzn-errortype without the Coral namespace suffix additionally differs from what AWS puts on the wire for this service's errors.

Fix: the InvokeBlueprintOptimizationAsync handler's validation, which must check the request's S3 locations (`samples[*].assetS3Object.s3Uri`, `samples[*].groundTruthS3Object.s3Uri`, `outputConfiguration.s3Object.s3Uri`) and refuse with ValidationException / HTTP 400 / message "Request has invalid S3 URI" BEFORE the blueprint-resolution refusal and before anything is minted or persisted; plus the service's error serializer, for the x-amzn-errortype Coral-namespace suffix. Note the open behavioral issue 20261029T211335Z-bedrock-data-automation-ten-net-new-causes-create describes this operation as a 501 scaffold stub and asks for a dataAutomationProfileArn resolution refusal (its Cause 9); that description is stale against the current handler, it carries no wire replay key for this fixture step, and it does not name the S3-URI validation this issue is about, so it does not cover this cause.

Traces: bedrock-data-automation-blueprint-optimization-invoke#task#2.

## Evidence
Replay of simparity-wire-aws-v1 (trace set simparity-wire-aws-v1, emulator up, 15/70 steps passed, 24 divergence clusters — 23 of them carrying a non-empty owned_by/stub_path and skipped).

Cluster: service=bedrock-data-automation | InvokeBlueprintOptimizationAsync | status=400->404 | keys=$.message,$headers.x-amzn-errortype,status
Fixture: bedrock-data-automation-blueprint-optimization-invoke, step task#2 (owned_by: [], stub_path: "" — net-new).

  status:                   expected 400, got 404
  $.message:                expected 'Request has invalid S3 URI',
                            got 'Blueprint Arn not found.'
  $headers.x-amzn-errortype: expected 'ValidationException:http://internal.amazon.com/coral/com.amazon.bedrock.keystone.build/',
                            got 'ResourceNotFoundException'

Source confirmation — services/service-com-amazonaws-bedrockdataautomation-amazonbedrockkeystonebuildtimeservice/src/ops/invoke_blueprint_optimization_async.rs: handle() (line 175 onward) reaches the store, then its ONLY pre-mint refusals are store.names_unknown_blueprint(...) -> ResourceNotFoundException with crate::state::BLUEPRINT_ARN_NOT_FOUND_MESSAGE (lines 197-209) and the OPTIMIZATION_JOB_QUOTA check (lines 213-226); it then mints an invocationArn and inserts a BlueprintOptimizationJobRecord (lines 228-255). input.samples and input.output_configuration are moved straight into the record (lines 246-247) and no s3Uri is ever inspected, so no ValidationException is reachable from this handler.

Model: InvokeBlueprintOptimizationAsync errors include ValidationException (client, 400) and ResourceNotFoundException (client, 404); S3Object.s3Uri -> S3Uri @length(1..1024) @pattern ^s3://[a-z0-9][\.\-a-z0-9]{1,61}[a-z0-9](/.*)?$ (so a pattern-valid-but-unusable URI passes the framework's validation walk and reaches the handler). Protocol restJson1.

The ARNs, generated blueprint names and ids in the recording are that run's values and are not requirements; the requirements are the 400 status, the ValidationException code, the fixed message text "Request has invalid S3 URI", the precedence of this refusal over the blueprint-resolution refusal, and the Coral-namespace suffix on x-amzn-errortype.

## Steps to Reproduce
1. git_clone, then replay_wire_corpus (trace set simparity-wire-aws-v1) against the bedrock-data-automation emulator.
2. Cause 1: replay fixture bedrock-data-automation-blueprint-optimization-invoke, step task#2 — after the fixture's setup#1 CreateBlueprint, call InvokeBlueprintOptimizationAsync with that blueprintArn, a valid dataAutomationProfileArn, and samples/outputConfiguration whose s3Uri values satisfy the modelled S3Uri pattern but are not usable S3 locations.
3. Assert the response is HTTP 400 with error code ValidationException and body message exactly "Request has invalid S3 URI" (no echoed URI, no member path), and header x-amzn-errortype == "ValidationException:http://internal.amazon.com/coral/com.amazon.bedrock.keystone.build/" — not HTTP 404 / ResourceNotFoundException / "Blueprint Arn not found.".
4. Assert the same 400 ValidationException is returned when the referenced blueprint is ALSO unresolvable, i.e. the S3-URI refusal takes precedence over the blueprint-not-found refusal.
5. Assert no job was minted: no invocationArn is returned and GetBlueprintOptimizationStatus resolves nothing for the refused submission.
