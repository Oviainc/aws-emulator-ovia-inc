Issue-Source: trace-sweep-divergence
Issue-Trace: fsx-absent-file-system-lifecycle-guards
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: fsx-absent-file-system-lifecycle-guards fsx-resource-tagging-guards
Issue-Shape-Id: com.amazonaws.fsx#AWSSimbaAPIService_v20180301
Issue-Key: fsx|DescribeFileSystems|4xx->4xx|$.Message
Issue-Key: fsx|UpdateFileSystem|4xx->4xx|$.Message
Issue-Key: fsx|DeleteFileSystem|4xx->4xx|$.Message
Issue-Key: fsx|TagResource|4xx->4xx|$.Message
Issue-Key: fsx|UntagResource|4xx->4xx|$.Message
Issue-Related: 20261022T052124Z-fsx-constraint-violations-are-not-reported-as-badrequest.md
Issue-Commit: f2a9c91c50f7e55687472cf5b456a197c6c3ca83

# FSx not-found errors omit the requested identifier from Message

> _Note: this issue was identified against the code base at commit `f2a9c91c50f7e55687472cf5b456a197c6c3ca83`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeFileSystems

## Description
Across every FSx not-found path in the corpus the emulator gets the HTTP status (400) and the error code right but serializes a fixed, hand-written Message instead of one built from the request. AWS's Message is derived from the identifier the caller sent, which is why it is a requirement here and not just recorded prose; the emulator's text is a constant that either reformats the identifier or, for the tagging operations, drops it altogether. These are six instances of one serializer defect: the not-found Message is not templated from the request. 6 causes.

Cause 1 of 6: DescribeFileSystems on an absent file system (fixture fsx-absent-file-system-lifecycle-guards, step task#1).
AWS: DescribeFileSystems' modeled errors include FileSystemNotFound (client, httpError 400; structure with member Message, ErrorMessage length 1..256; doc "No Amazon FSx file systems were found based upon supplied parameters."). The recorded 400 carries a Message built from the requested FileSystemIds entry, in the form "File system '<file-system-id>' does not exist." - capitalised, the id single-quoted, terminated with a period.
Emulator: HTTP 400 with the right error code but Message 'file system <file-system-id> does not exist' - lower-cased, unquoted id, no period.
Fix: the FileSystemNotFound construction shared by the file-system handlers in the fsx service crate - build Message from the requested id using AWS's template instead of a hand-formatted string.

Cause 2 of 6: UpdateFileSystem on an absent file system (fixture fsx-absent-file-system-lifecycle-guards, step task#2).
AWS: UpdateFileSystem's modeled errors include FileSystemNotFound (client, 400). Recorded Message is the same request-derived template as Cause 1: "File system '<file-system-id>' does not exist."
Emulator: the same constant 'file system <file-system-id> does not exist'.
Fix: as Cause 1 - UpdateFileSystem's not-found path must use the shared, request-templated Message.

Cause 3 of 6: DeleteFileSystem on an absent file system (fixture fsx-absent-file-system-lifecycle-guards, step task#4).
AWS: recorded behavior only - DeleteFileSystem's modeled errors include FileSystemNotFound (client, 400), and the Smithy model does not state the Message text. The recording shows that on this operation the Message is the bare requested file system id ('<file-system-id>') and nothing else, i.e. a different template from Describe/Update above.
Emulator: 'file system <file-system-id> does not exist'.
Why (Causes 1-3): the identifier-bearing Message is what a client logs, shows an operator, or parses to learn which of the ids it asked about was missing. Hard-coding the sentence means the emulator cannot reproduce FSx's per-operation Message shape, and anything asserting on it diverges.
Fix: DeleteFileSystem's not-found path in the fsx service crate - emit the recorded bare-id Message rather than the shared sentence.

Cause 4 of 6: TagResource on an absent resource (fixture fsx-resource-tagging-guards, step task#1).
AWS: TagResource's modeled errors include ResourceNotFound (client, httpError 400; members ResourceARN (@required) and Message, ErrorMessage length 1..256; doc "The resource specified by the Amazon Resource Name (ARN) can't be found."). The recorded Message is built from the identifier in the request ARN: "Resource '<resource-id>' does not exist".
Emulator: the constant Message 'the resource does not exist', which names no resource at all.
Fix: the ResourceNotFound construction shared by the tagging handlers in the fsx service crate - template Message from the request's ResourceARN.

Cause 5 of 6: ListTagsForResource on an absent resource (fixture fsx-resource-tagging-guards, step task#2).
AWS: ListTagsForResource also models ResourceNotFound (client, 400). Recorded Message "Resource '<resource-id>' does not exist", here for an absent backup id - the same template over whatever resource the ARN names.
Emulator: the same constant 'the resource does not exist'.
Fix: as Cause 4.

Cause 6 of 6: UntagResource on an absent resource (fixture fsx-resource-tagging-guards, step task#4).
AWS: UntagResource also models ResourceNotFound (client, 400). Recorded Message "Resource '<resource-id>' does not exist".
Emulator: the same constant 'the resource does not exist'.
Why (Causes 4-6): the emulator's tagging not-found Message contains no identifier whatsoever. When a client tags several resources and one lookup fails, AWS's response tells it which one; the emulator's does not, so the diagnostic that makes the failure actionable is gone even though the status and error code look correct.
Fix: as Cause 4 - one shared templated Message for the three tagging operations' ResourceNotFound path.

Traces: fsx-absent-file-system-lifecycle-guards#task#1, #task#2, #task#4; fsx-resource-tagging-guards#task#1, #task#2, #task#4.

## Evidence
Replay of trace set simparity-wire-aws-v1 (27 steps, 0 passed; state_reset_failures 0). In all six clusters expected and actual status are both 400 and only $.Message diverges.

Cause 1 - fsx-absent-file-system-lifecycle-guards#task#1, DescribeFileSystems:
  $.Message: value_mismatch (expected "File system 'fs-0a71c3d9e5b4f8206' does not exist.", got 'file system fs-0a71c3d9e5b4f8206 does not exist')
Cause 2 - fsx-absent-file-system-lifecycle-guards#task#2, UpdateFileSystem:
  $.Message: value_mismatch (expected "File system 'fs-0a71c3d9e5b4f8206' does not exist.", got 'file system fs-0a71c3d9e5b4f8206 does not exist')
Cause 3 - fsx-absent-file-system-lifecycle-guards#task#4, DeleteFileSystem:
  $.Message: value_mismatch (expected 'fs-0a71c3d9e5b4f8206', got 'file system fs-0a71c3d9e5b4f8206 does not exist')
Cause 4 - fsx-resource-tagging-guards#task#1, TagResource:
  $.Message: value_mismatch (expected "Resource 'fs-0a71c3d9e5b4f8206' does not exist", got 'the resource does not exist')
Cause 5 - fsx-resource-tagging-guards#task#2, ListTagsForResource:
  $.Message: value_mismatch (expected "Resource 'backup-04c8e1b7fa63d9502' does not exist", got 'the resource does not exist')
Cause 6 - fsx-resource-tagging-guards#task#4, UntagResource:
  $.Message: value_mismatch (expected "Resource 'fs-0a71c3d9e5b4f8206' does not exist", got 'the resource does not exist')

The ids above are the recorded fixtures' values and are examples; the requirement is that Message be built from the identifier the caller sent, in the per-operation template the recordings attest (quoted-id sentence for Describe/Update file system, bare id for DeleteFileSystem, "Resource '<id>' does not exist" for the three tagging operations).

Model evidence (smithy_query on services/service-com-amazonaws-fsx-awssimbaapiservice-v20180301/smithy-model/com-amazonaws-fsx-awssimbaapiservice-v20180301.json): FileSystemNotFound (client/400, member Message) is modeled on DescribeFileSystems, UpdateFileSystem and DeleteFileSystem; ResourceNotFound (client/400, members ResourceARN @required and Message) is modeled on TagResource, ListTagsForResource and UntagResource.

## Steps to Reproduce
1. Boot the emulator from this checkout and point an FSx client at its data listener.
2. Causes 1-3: with no such file system in state, call DescribeFileSystems (step task#1), UpdateFileSystem (task#2) and DeleteFileSystem (task#4) with a well-formed but unknown fs- id. Each returns 400 with the correct error code but Message 'file system <id> does not exist'; AWS returns "File system '<id>' does not exist." for Describe/Update and the bare '<id>' for Delete. (Replay fixture fsx-absent-file-system-lifecycle-guards.)
3. Causes 4-6: with no such resource in state, call TagResource (step task#1), ListTagsForResource (task#2) and UntagResource (task#4) with a well-formed FSx ARN for an absent file system or backup. Each returns 400 with Message 'the resource does not exist'; AWS returns "Resource '<id>' does not exist", naming the resource. (Replay fixture fsx-resource-tagging-guards.)
