Issue-Source: trace-sweep-divergence
Issue-Trace: datasync-s3-to-s3-task-execution-copies-object
Issue-Step: task#11
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: datasync-s3-to-s3-task-execution-copies-object
Issue-Shape-Id: com.amazonaws.datasync#FmrsService
Issue-Key: datasync|DescribeTaskExecution|2xx->2xx|$.BytesTransferred
Issue-Key: datasync|DescribeTaskExecution|2xx->2xx|$.BytesWritten
Issue-Key: datasync|DescribeTaskExecution|2xx->2xx|$.EndTime
Issue-Key: datasync|DescribeTaskExecution|2xx->2xx|$.EstimatedBytesToTransfer
Issue-Key: datasync|DescribeTaskExecution|2xx->2xx|$.EstimatedFilesToTransfer
Issue-Key: datasync|DescribeTaskExecution|2xx->2xx|$.Excludes
Issue-Key: datasync|DescribeTaskExecution|2xx->2xx|$.FilesTransferred
Issue-Key: datasync|DescribeTaskExecution|2xx->2xx|$.FilesVerified
Issue-Key: datasync|DescribeTaskExecution|2xx->2xx|$.Includes
Issue-Key: datasync|DescribeTaskExecution|2xx->2xx|$.LaunchTime
Issue-Key: datasync|DescribeTaskExecution|2xx->2xx|$.Options.Gid
Issue-Key: datasync|DescribeTaskExecution|2xx->2xx|$.Options.PosixPermissions
Issue-Key: datasync|DescribeTaskExecution|2xx->2xx|$.Options.PreserveNfsAcls
Issue-Key: datasync|DescribeTaskExecution|2xx->2xx|$.Options.SecurityDescriptorCopyFlags
Issue-Key: datasync|DescribeTaskExecution|2xx->2xx|$.Options.Uid
Issue-Key: datasync|DescribeTaskExecution|2xx->2xx|$.StartTime
Issue-Commit: 122d2e23ed306dec374f1768a21e001304c75e9d

# DataSync DescribeTaskExecution reports a no-op transfer: zero counters, omitted Excludes/Includes and default-valued Options

> _Note: this issue was identified against the code base at commit `122d2e23ed306dec374f1768a21e001304c75e9d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeTaskExecution

## Description
One root cause, grounded in one wire divergence cluster.

=== Cause 1 of 1: a terminal S3->S3 task execution is described as having moved nothing, with the always-present filter lists missing and Options reported as service defaults instead of the execution's effective options ===

AWS: Smithy (services/service-com-amazonaws-datasync-fmrsservice/smithy-model/com-amazonaws-datasync-fmrsservice.json, DescribeTaskExecution) models EstimatedFilesToTransfer, EstimatedBytesToTransfer, FilesTransferred, BytesTransferred, BytesWritten and FilesVerified as `long` with default 0 — they are the work the execution actually performed, not constants — and models Excludes/Includes as FilterList (@length min 0, max 1) and Options as the Options structure. Real AWS populates every one of them on a terminal execution: for a transfer of N source objects totalling B bytes it returns EstimatedFilesToTransfer=N, EstimatedBytesToTransfer=B, FilesTransferred=N, BytesTransferred=B, BytesWritten=B and FilesVerified=N; it always emits Excludes and Includes as lists (the empty list `[]` when the execution has no filters, never an omitted member); and it echoes the Options actually in force for that execution — the values carried over from the task (or overridden on StartTaskExecution) — not the documented service defaults. The recorded response for a one-object, 40-byte S3-to-S3 execution shows exactly that: counters 1/40/1/40/40/1, Excludes=[], Includes=[], and Options members Uid=NONE, Gid=NONE, PosixPermissions=NONE, SecurityDescriptorCopyFlags=NONE as configured, plus PreserveNfsAcls=NONE. (Those numbers are the fixture's values filling the template, not the requirement; the requirement is that each counter equals the work done and that the Options echo the execution's own configuration.) Options.PreserveNfsAcls is not a member of the Options shape in the checked-out model — recorded behavior only: AWS's DescribeTaskExecution response carries it alongside the other metadata options, and the recording shows the value NONE.

Emulator: returns HTTP 200 (status matches) but describes an execution that did no work: BytesTransferred=0, BytesWritten=0, EstimatedBytesToTransfer=0, EstimatedFilesToTransfer=0, FilesTransferred=0, FilesVerified=0 — i.e. the modeled defaults, never written by the transfer phase. Excludes and Includes are absent from the payload (null) rather than `[]`. Options is serialized from the service defaults documented on the Options shape rather than from the execution's configuration: Gid=INT_VALUE, Uid=INT_VALUE, PosixPermissions=PRESERVE, SecurityDescriptorCopyFlags=OWNER_DACL where the recording has NONE for all four, and PreserveNfsAcls is omitted entirely.

Not part of this finding: the same step also diverges on StartTime, LaunchTime and EndTime, which the fixture marks '<shaped-not-asserted>'. Those are per-run clock values and are deliberately not claimed as a defect here.

Why: every client observation of a DataSync run is wrong. Progress and volume reporting (the Estimated*/Transferred/Written/Verified counters) read zero for an execution the emulator reports as finished, so a caller cannot tell a successful transfer from one that moved nothing, and throughput/accounting logic computes zero. SDK code that iterates response.Excludes / response.Includes gets a null where AWS guarantees a list and either throws or has to special-case the emulator. A caller that reads the execution's Options back to confirm which metadata-handling rules were applied (the normal way to verify a per-execution override took effect) is told PRESERVE/INT_VALUE/OWNER_DACL for an execution configured with NONE, i.e. the response contradicts the request.

Fix: the task-execution state machine/state that advances an execution through PREPARING/TRANSFERRING/VERIFYING must record the work it did (enumerate the source location's objects to populate the Estimated* counters, then set FilesTransferred/BytesTransferred/BytesWritten/FilesVerified from the copy it performed), must carry the task's/execution's effective Options onto the execution record instead of re-deriving defaults, and the DescribeTaskExecution serializer must always emit Excludes and Includes as lists and include PreserveNfsAcls.

Related but not covering this: open issue 20261023T110503Z-datasync-createtask-createlocations3-cross-member-and-access-validation.md (behavioral corpus) describes the same stubbed execution engine in its Cause 6, but it carries no wire replay key, so replaying it does not exercise this fixture step; the replay report lists this cluster as unowned.

Traces: datasync-s3-to-s3-task-execution-copies-object#task#11

## Evidence
Fixture datasync-s3-to-s3-task-execution-copies-object, step task#11, operation DescribeTaskExecution, verdict mismatch. Expected status 200, actual status 200.

Field-level divergences reported by the replay:
  $.BytesTransferred: expected 40, got 0
  $.BytesWritten: expected 40, got 0
  $.EstimatedBytesToTransfer: expected 40, got 0
  $.EstimatedFilesToTransfer: expected 1, got 0
  $.FilesTransferred: expected 1, got 0
  $.FilesVerified: expected 1, got 0
  $.Excludes: missing (expected [], got None)
  $.Includes: missing (expected [], got None)
  $.Options.Gid: expected 'NONE', got 'INT_VALUE'
  $.Options.Uid: expected 'NONE', got 'INT_VALUE'
  $.Options.PosixPermissions: expected 'NONE', got 'PRESERVE'
  $.Options.SecurityDescriptorCopyFlags: expected 'NONE', got 'OWNER_DACL'
  $.Options.PreserveNfsAcls: missing (expected 'NONE', got None)
  $.StartTime / $.LaunchTime / $.EndTime: expected '<shaped-not-asserted>' (per-run clock values — NOT claimed as a defect)

Model references: DescribeTaskExecution output members EstimatedFilesToTransfer, EstimatedBytesToTransfer, FilesTransferred, BytesWritten, BytesTransferred, FilesVerified -> long, default 0; Excludes/Includes -> FilterList with @length {min:0,max:1}; Options -> Options structure whose documented defaults are Uid=INT_VALUE, Gid=INT_VALUE, PosixPermissions=PRESERVE, SecurityDescriptorCopyFlags=OWNER_DACL (the values the emulator emits); Options has no PreserveNfsAcls member in the checked-out model, so that member is recorded behavior only.

## Steps to Reproduce
Replay fixture datasync-s3-to-s3-task-execution-copies-object and inspect step task#11.

Manually: create a source S3 bucket holding a single small object and an empty destination bucket; CreateLocationS3 for each; CreateTask over the two locations with Options setting Uid=NONE, Gid=NONE, PosixPermissions=NONE, SecurityDescriptorCopyFlags=NONE and no Includes/Excludes; StartTaskExecution; poll DescribeTaskExecution to a terminal status. Observe HTTP 200 with every transfer counter 0 (instead of 1 file / the object's byte count), Excludes and Includes absent from the JSON body instead of [], Options reported as Gid=INT_VALUE / Uid=INT_VALUE / PosixPermissions=PRESERVE / SecurityDescriptorCopyFlags=OWNER_DACL instead of the configured NONE, and no Options.PreserveNfsAcls member.
