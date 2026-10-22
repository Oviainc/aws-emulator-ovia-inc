Issue-Source: trace-sweep-divergence
Issue-Trace: fsx-create-file-system-validation-precedence
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: fsx-create-file-system-validation-precedence fsx-data-repository-association-guards fsx-resource-tagging-guards
Issue-Shape-Id: com.amazonaws.fsx#AWSSimbaAPIService_v20180301
Issue-Key: fsx|CreateFileSystem|4xx->4xx|$.Message
Issue-Key: fsx|CreateFileSystem|4xx->4xx|$.__type
Issue-Key: fsx|UpdateDataRepositoryAssociation|4xx->4xx|$.Message
Issue-Key: fsx|UpdateDataRepositoryAssociation|4xx->4xx|$.__type
Issue-Key: fsx|ListTagsForResource|4xx->4xx|$.Message
Issue-Key: fsx|ListTagsForResource|4xx->4xx|$.__type
Issue-Commit: f2a9c91c50f7e55687472cf5b456a197c6c3ca83

# FSx constraint violations are not reported as BadRequest with a validation Message

> _Note: this issue was identified against the code base at commit `f2a9c91c50f7e55687472cf5b456a197c6c3ca83`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateFileSystem

## Description
When an FSx request violates a modeled input constraint, AWS answers with the FSx-specific error BadRequest at HTTP 400 and a Message describing the violated constraint. The emulator instead emits a foreign error code (a generic ValidationException, or a resource-dispatch error raised because the constraint was never checked at all) and in two of the three cases omits the modeled Message member entirely. The HTTP status happens to be 400 throughout, so the status is not the problem - the error code on the wire is. 3 causes.

Cause 1 of 3: CreateFileSystem reports ValidationException instead of BadRequest and drops Message (fixture fsx-create-file-system-validation-precedence, step task#1).
AWS: CreateFileSystem's modeled client errors are ActiveDirectoryError, BadRequest, IncompatibleParameterError, InvalidExportPath, InvalidImportPath, InvalidNetworkSettings, InvalidPerUnitStorageThroughput, MissingFileSystemConfiguration and ServiceLimitExceeded - all client/httpError 400. ValidationException is not among them and is not a shape in the FSx model. Smithy: BadRequest is a structure with a single member Message (ErrorMessage, length 1..256), doc "A generic error indicating a failure with a client request." The recorded response is HTTP 400 with __type BadRequest and a Message (recorded example: "Request failed validation").
Emulator: HTTP 400 with __type ValidationException and no Message member at all.
Why: an SDK matches the error by code. BadRequest is a modeled FSx shape with its own catch block and its own Message field; ValidationException matches nothing in the FSx client, so the SDK degrades it to an unmodeled service error, the caller's `except BadRequest` branch never runs, and the modeled Message a client logs or surfaces to a user is null.
Fix: CreateFileSystem's error mapping in the fsx service crate - map input-constraint failures onto the modeled BadRequest shape and always serialize its Message member, rather than emitting a generic cross-service ValidationException.

Cause 2 of 3: UpdateDataRepositoryAssociation range violation reports ValidationException and drops Message (fixture fsx-data-repository-association-guards, step task#4).
AWS: UpdateDataRepositoryAssociation.ImportedFileChunkSize targets Megabytes with @range min 1, max 512000 (Smithy). A value below the minimum fails with the modeled error BadRequest (client, 400 - UpdateDataRepositoryAssociation's errors are BadRequest, DataRepositoryAssociationNotFound, IncompatibleParameterError, ServiceLimitExceeded and InternalServerError), and the Message is the standard constraint-violation text built from the request and the model: "1 validation error detected: Value at 'importedFileChunkSize' failed to satisfy constraint: Member must have value greater than or equal to 1". That text is a requirement here because its rule is known - the member name and the @range bound both come from the model.
Emulator: HTTP 400 with __type ValidationException and no Message, so neither the error code nor the constraint diagnostic reaches the client.
Why: same code-matching failure as Cause 1, plus the client loses the only statement of WHICH member and WHICH bound it violated - with Message absent there is nothing to show a user or a log.
Fix: UpdateDataRepositoryAssociation's validation in the fsx service crate - enforce the Megabytes @range on ImportedFileChunkSize and report it as BadRequest (400) with the standard "1 validation error detected: Value at '<member>' failed to satisfy constraint: ..." Message.

Cause 3 of 3: ListTagsForResource dispatches on a malformed ARN instead of failing its pattern constraint (fixture fsx-resource-tagging-guards, step task#3).
AWS: ListTagsForResource.ResourceARN is @required with @length 8..512 and the @pattern ^arn:(?=[^:]+:fsx:[^:]+:\d{12}:)((|(?=[a-z0-9-.]{1,63})(?!\d{1,3}(\.\d{1,3}){3})(?![^:]*-{2})(?![^:]*-\.)(?![^:]*\.-)[a-z0-9].*(?<!-)):){4}(?!/).{0,1024}$ (Smithy). Input-constraint validation runs BEFORE the resource is interpreted, so an ARN that does not satisfy the pattern fails with BadRequest (400) and the standard Message "1 validation error detected: Value at 'resourceARN' failed to satisfy constraint: Member must satisfy regular expression pattern: <that pattern>". NotServiceResourceError (also modeled on this operation, client/400, members ResourceARN and Message) is for a well-formed ARN that names a resource of some other service - it is reached only after the pattern check passes.
Emulator: HTTP 400 with __type NotServiceResourceError and Message 'the resource is not a type owned by Amazon FSx' - it never applies the pattern constraint and goes straight to resource-type dispatch, inverting FSx's validation precedence.
Why: the emulator reports the wrong failure for a malformed ARN. A client testing its own ARN construction sees "that resource belongs to another service" instead of "your ARN is syntactically invalid", so the real bug (a bad ARN string) is misdiagnosed, and any caller branching on BadRequest vs NotServiceResourceError branches the wrong way.
Fix: ListTagsForResource's input validation in the fsx service crate (shared with TagResource/UntagResource, which take the same constrained ResourceARN) - apply the ResourceARN @length/@pattern constraints before resource-type dispatch and emit BadRequest (400) with the standard pattern-violation Message.

Traces: fsx-create-file-system-validation-precedence#task#1, fsx-data-repository-association-guards#task#4, fsx-resource-tagging-guards#task#3.

## Evidence
Replay of trace set simparity-wire-aws-v1 (27 steps, 0 passed; state_reset_failures 0).

Cause 1 - fsx-create-file-system-validation-precedence#task#1, CreateFileSystem, expected 400 / actual 400:
  $.__type: value_mismatch (expected 'BadRequest', got 'ValidationException')
  $.Message: missing (expected 'Request failed validation', got None)

Cause 2 - fsx-data-repository-association-guards#task#4, UpdateDataRepositoryAssociation, expected 400 / actual 400:
  $.__type: value_mismatch (expected 'BadRequest', got 'ValidationException')
  $.Message: missing (expected "1 validation error detected: Value at 'importedFileChunkSize' failed to satisfy constraint: Member must have value greater than or equal to 1", got None)

Cause 3 - fsx-resource-tagging-guards#task#3, ListTagsForResource, expected 400 / actual 400:
  $.__type: value_mismatch (expected 'BadRequest', got 'NotServiceResourceError')
  $.Message: value_mismatch (expected "1 validation error detected: Value at 'resourceARN' failed to satisfy constraint: Member must satisfy regular expression pattern: ^arn:(?=[^:]+:fsx:[^:]+:\\d{12}:)((|(?=[a-z0-9-.]{1,63})(?!\\d{1,3}(\\.\\d{1,3}){3})(?![^:]*-{2})(?![^:]*-\\.)(?![^:]*\\.-)[a-z0-9].*(?<!-)):){4}(?!/).{0,1024}$", got 'the resource is not a type owned by Amazon FSx')

Model evidence (smithy_query on services/service-com-amazonaws-fsx-awssimbaapiservice-v20180301/smithy-model/com-amazonaws-fsx-awssimbaapiservice-v20180301.json): BadRequest = client/400 structure with member Message (ErrorMessage, length 1..256); it is a modeled error of CreateFileSystem, UpdateDataRepositoryAssociation and ListTagsForResource. No ValidationException shape exists in the FSx model. UpdateDataRepositoryAssociation.ImportedFileChunkSize -> Megabytes @range{min:1,max:512000}. ListTagsForResource.ResourceARN @required @length{8,512} @pattern as quoted above.

## Steps to Reproduce
1. Boot the emulator from this checkout and point an FSx client at its data listener.
2. Cause 1: send a CreateFileSystem request that violates an input constraint. Expected: HTTP 400, __type BadRequest, Message present. Actual: HTTP 400, __type ValidationException, no Message. (Replay fsx-create-file-system-validation-precedence step task#1.)
3. Cause 2: call UpdateDataRepositoryAssociation on an existing association with ImportedFileChunkSize below the Megabytes @range minimum of 1. Expected: HTTP 400, __type BadRequest, Message "1 validation error detected: Value at 'importedFileChunkSize' failed to satisfy constraint: Member must have value greater than or equal to 1". Actual: HTTP 400, __type ValidationException, no Message. (Replay fsx-data-repository-association-guards step task#4.)
4. Cause 3: call ListTagsForResource with a ResourceARN that does not satisfy the modeled FSx ARN pattern. Expected: HTTP 400, __type BadRequest, Message reporting the resourceARN pattern violation. Actual: HTTP 400, __type NotServiceResourceError with Message 'the resource is not a type owned by Amazon FSx'. (Replay fsx-resource-tagging-guards step task#3.)
