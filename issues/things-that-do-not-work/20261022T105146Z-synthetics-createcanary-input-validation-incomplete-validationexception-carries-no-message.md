Issue-Source: trace-sweep-divergence
Issue-Trace: synthetics-canary-create-validation-errors
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: synthetics-canary-create-validation-errors
Issue-Shape-Id: com.amazonaws.synthetics#Synthetics
Issue-Key: synthetics|CreateCanary|4xx->4xx|$.Message
Issue-Key: synthetics|CreateCanary|4xx->2xx|$.Message
Issue-Key: synthetics|CreateCanary|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: synthetics|CreateCanary|4xx->2xx|status
Issue-Commit: dcb14db06d6eb8eb19e965c6ac371e7edd3d0be4

# CreateCanary input validation incomplete: ValidationException carries no Message and malformed ArtifactS3Location is accepted

> _Note: this issue was identified against the code base at commit `dcb14db06d6eb8eb19e965c6ac371e7edd3d0be4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateCanary

## Description
CreateCanary does not produce AWS's rejection for two classes of bad input: the constraint-violation 400 is emitted with an empty error body, and a malformed artifact location is not rejected at all. 2 causes, both on fixture synthetics-canary-create-validation-errors.

Cause 1 of 2 — ValidationException body has no Message member (fixture synthetics-canary-create-validation-errors, step task#2).
AWS: CreateCanary.Name targets CanaryName with @length min 1 max 255 and @pattern ^[0-9a-z_\-]+$ (Smithy: services/service-com-amazonaws-synthetics-synthetics/smithy-model/com-amazonaws-synthetics-synthetics.json). A Name violating that pattern is refused with ValidationException, httpError 400, x-amzn-errortype: ValidationException. ValidationException models a single member Message (target ErrorMessage), and the recorded response populates it with the standard constraint-violation text built from the request — it names the rejected value, the member path and the violated constraint, e.g. "1 validation error detected: Value 'Invalid_Canary_NAME!' at 'name' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[0-9a-z_\-]+$". Because that text is derived from the offending member and its modeled constraint, the rule (not the exact recorded string) is the requirement.
Emulator: status 400 is correct, but $.Message is absent from the serialized body (replay: "$.Message: missing"). The error shape is emitted with no message member at all.
Why: SDKs and the CLI surface ValidationException via its Message; a client gets an error with an empty message and cannot tell which member failed which constraint. Operators scripting against the message, and any client logging it, see nothing. The modeled Message member is simply never populated for constraint failures raised at the request boundary.
Fix: the request-boundary constraint validation / error serializer path for CreateCanary — the generic constraint check must build a ValidationException whose Message names the rejected value, the member path and the violated constraint, instead of an error with no Message.

Cause 2 of 2 — malformed ArtifactS3Location is accepted with 200 instead of refused with 400 (fixture synthetics-canary-create-validation-errors, step task#3).
AWS: CreateCanary.ArtifactS3Location is required, target String, @length min 1 max 1024, and the model's own member documentation states "The location in Amazon S3 where Synthetics stores artifacts from the test runs of this canary ... The name of the Amazon S3 bucket can't include a period (.)". A location that is not a well-formed S3 location is refused with ValidationException (httpError 400, x-amzn-errortype: ValidationException); the recorded message is "ArtifactS3Location: The location is malformed." (example text — the requirement is the 400 plus the ValidationException error type).
Emulator: the request succeeds with status 200 and a Canary is created. Replay reports "status: expected 400, got 200", plus a missing $.Message and a missing x-amzn-errortype header. src/ops/create_canary.rs validates Code, RunConfig.MemoryInMB, ResourcesToReplicateTags and AddReplicaLocations, but performs no check on artifact_s3_location at all — the value is stored verbatim (artifact_s3_location: input.artifact_s3_location).
Why: a client that AWS would have stopped at create time instead gets a canary that exists in the emulator and will never be able to write artifacts. Negative tests pass where they should fail, and error-handling paths in the caller are never exercised; worse, the store now holds a canary AWS would not have.
Fix: CreateCanary's handler validation (the same block that already enforces the prose-only Code and RunConfig rules) must reject a malformed ArtifactS3Location — at minimum a value that is not a well-formed s3 location and a bucket component containing a period — with the modeled ValidationException (400).

Traces: synthetics-canary-create-validation-errors#task#2 (Cause 1), synthetics-canary-create-validation-errors#task#3 (Cause 2). No other synthetics issue is open against CreateCanary.

## Evidence
Cause 1 (step task#2): expected status 400, actual 400 — detail: $.Message: missing (expected "1 validation error detected: Value 'Invalid_Canary_NAME!' at 'name' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[0-9a-z_\-]+$", got None). Expected: 400 ValidationException with a populated Message naming member and constraint. Actual: 400 with no Message member in the body.

Cause 2 (step task#3): expected status 400, actual 200 — detail: status: expected 400, got 200; $.Message: missing (expected 'ArtifactS3Location: The location is malformed.', got None); $headers.x-amzn-errortype: missing (expected 'ValidationException', got None). Expected: 400, x-amzn-errortype: ValidationException. Actual: 200 with a created Canary.

Model: CreateCanary errors = InternalServerException(500), RequestEntityTooLargeException(413), ValidationException(400); ValidationException members = {Message: ErrorMessage}. Code: services/service-com-amazonaws-synthetics-synthetics/src/ops/create_canary.rs lines 240-262 (validation block; no ArtifactS3Location check), line 330 (artifact_s3_location stored unchecked), lines 413-419 (validation() helper).

## Steps to Reproduce
1. git_clone(); 2. replay_wire_corpus(); 3. Inspect fixture synthetics-canary-create-validation-errors, steps task#2 and task#3.
Equivalently, by hand against the emulator's synthetics endpoint:
- POST /canary with Name='Invalid_Canary_NAME!' (violates ^[0-9a-z_\-]+$) plus otherwise valid Code/ArtifactS3Location/ExecutionRoleArn/Schedule/RuntimeVersion: observe 400 with a body carrying no Message member (AWS returns ValidationException with a Message naming the member and the violated pattern).
- POST /canary with a valid Name and a malformed ArtifactS3Location (e.g. a non-s3 location, or a bucket component containing a period): observe 200 and a created canary (AWS returns 400 with x-amzn-errortype: ValidationException).
