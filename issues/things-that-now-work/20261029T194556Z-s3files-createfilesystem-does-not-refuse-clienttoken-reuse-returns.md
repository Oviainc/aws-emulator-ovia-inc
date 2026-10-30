Issue-Source: trace-sweep-divergence
Issue-Trace: s3files-file-system-create-errors
Issue-Step: task#10
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: s3files-file-system-create-errors
Issue-Shape-Id: com.amazonaws.s3files#S3Files
Issue-Key: s3files|CreateFileSystem|4xx->2xx|$.errorCode
Issue-Key: s3files|CreateFileSystem|4xx->2xx|$.message
Issue-Key: s3files|CreateFileSystem|4xx->2xx|$.resourceId
Issue-Key: s3files|CreateFileSystem|4xx->2xx|$.resourceType
Issue-Key: s3files|CreateFileSystem|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: s3files|CreateFileSystem|4xx->2xx|status
Issue-Commit: 5b71589754f95f05f88c1a0ee9ccd691bbc5f701

# s3files CreateFileSystem does not refuse clientToken reuse — returns 201 instead of 409 ConflictException

> _Note: this issue was identified against the code base at commit `5b71589754f95f05f88c1a0ee9ccd691bbc5f701`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateFileSystem

## Description
One root cause, exercised on the wire: CreateFileSystem treats `clientToken` as a free-form echo field instead of a single-use creation guard, so a repeat create with an already-consumed token provisions a second file system rather than being refused with a modeled ConflictException.

--- Cause 1 of 1: clientToken reuse on CreateFileSystem is not detected, so no ConflictException is ever produced ---

AWS: Smithy (services/service-com-amazonaws-s3files-s3files/smithy-model/com-amazonaws-s3files-s3files.json): CreateFileSystem is PUT /file-systems with success code 201 and models `ConflictException` (errorKind client, httpError 409). CreateFileSystemRequest.clientToken targets CreationToken (@idempotencyToken, @length 1..64, @pattern ^(.+)$) and is documented as "A unique, case-sensitive identifier that you provide to ensure idempotent creation". ConflictException is documented as "The request conflicts with the current state of the resource. This can occur when trying to create a resource that already exists..." and its members are `errorCode` (required, @length min 1), `message`, `resourceId` ("The identifier of the resource that caused the conflict") and `resourceType` ("The type of the resource that caused the conflict"). Recorded behavior: once a clientToken has been consumed by a successful CreateFileSystem, a later CreateFileSystem carrying that same token is rejected with HTTP 409, a serialized ConflictException whose errorCode is "ConflictException", resourceType "FileSystem", resourceId the OWNING file system's id, and the standard awsJson/restJson error-type header x-amzn-errortype set to the shape name. The message is a fixed sentence with one variable slot filled from the emulator's own state — the id of the file system that already holds the token: "File system '<fileSystemId>' already exists with this clientToken." Fixed text: "File system '" ... "' already exists with this clientToken."; variable part: the owning fileSystemId (the recording's fs-941f2233fc96e35bb is one run's value, not part of the requirement, and the same holds for resourceId).

Emulator: the replayed step returns HTTP 201 and a successful CreateFileSystemResponse — a brand-new file system is created against the reused token. No error body at all: $.errorCode, $.message, $.resourceId and $.resourceType are absent and the x-amzn-errortype response header is not emitted.

Why: a client that retries or duplicates a create (SDK retry, replayed workflow step, re-run of an IaC task) relies on the token making the call safe; against this emulator every retry silently provisions another file system over the same bucket, leaking resources and handing back a fileSystemId different from the one the caller already recorded. Code that branches on 409 ConflictException to mean "already created, adopt the existing resource" never takes that branch, and code that reads resourceId out of the conflict to recover the real id gets nothing.

Fix: CreateFileSystem's handler/state in the s3files service must record consumed creation tokens per account and, on reuse, return the modeled ConflictException (409) populated with errorCode="ConflictException", resourceType="FileSystem" and resourceId=<owning fileSystemId>, serialized through the service's error serializer so the x-amzn-errortype header is set.

Related open issue (does NOT cover this): '20261022T204614Z-s3files-six-control-plane-fidelity-defects-missing-nested.md' Cause 2 describes the same missing clientToken dedupe from the BEHAVIORAL suite, but it carries no wire replay key for this fixture step and does not state the ConflictException body members (resourceId/resourceType) or the x-amzn-errortype header that the recorded wire response requires, so this divergence is filed with its own replay binding.

Traces: s3files-file-system-create-errors#task#10

## Evidence
Replay of simparity-wire-aws-v1, fixture s3files-file-system-create-errors, step task#10 (service s3files, operation CreateFileSystem), verdict mismatch:

  status: expected 409, got 201
  $.errorCode: expected 'ConflictException', got None (absent)
  $.message: expected "File system 'fs-941f2233fc96e35bb' already exists with this clientToken.", got None (absent)
  $.resourceId: expected 'fs-941f2233fc96e35bb', got None (absent)
  $.resourceType: expected 'FileSystem', got None (absent)
  $headers.x-amzn-errortype: expected present (shape name, value not asserted), got None (absent)

The expected message's fixed text is "File system '" + "' already exists with this clientToken."; the embedded id (and resourceId) is the id of the file system created by the FIRST call that consumed the token, i.e. a value from emulator state, not a literal to hard-code. The 201 the emulator returned is CreateFileSystem's modeled success code, confirming the duplicate create succeeded rather than erroring.

Model references used: smithy_query operation=CreateFileSystem and shapes ConflictException, CreateFileSystemRequest in services/service-com-amazonaws-s3files-s3files/smithy-model/com-amazonaws-s3files-s3files.json.

## Steps to Reproduce
1. Boot the emulator built from this checkout and replay fixture 's3files-file-system-create-errors' from trace set simparity-wire-aws-v1; step 'task#10' diverges.
2. Manually: pick a unique token T. PUT /file-systems with {bucket: <existing bucket ARN>, roleArn: <well-formed s3files role ARN>, clientToken: T} -> 201, note the returned fileSystemId F.
3. Repeat the same CreateFileSystem call with clientToken T (either identical parameters or a different bucket/roleArn).
4. Observed: HTTP 201 and a NEW fileSystemId, with T echoed back.
   Expected: HTTP 409 with a ConflictException body — errorCode 'ConflictException', resourceType 'FileSystem', resourceId F, message "File system 'F' already exists with this clientToken." — and the x-amzn-errortype header naming the error shape.
