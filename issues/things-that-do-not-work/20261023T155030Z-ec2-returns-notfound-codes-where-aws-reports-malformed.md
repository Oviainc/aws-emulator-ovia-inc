Issue-Source: trace-sweep-divergence
Issue-Trace: ec2-cancel-task-notfound-error-paths
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ec2-cancel-task-notfound-error-paths ec2-hardware-and-byoip-not-found-error-catalog ec2-image-store-restore-error-paths ec2-image-tasks-and-usage-report-invalid-input-errors ec2-store-and-restore-image-mistakes
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2|CancelExportTask|4xx->4xx|$.Response.Errors.Error.Code
Issue-Key: ec2|CancelExportTask|4xx->4xx|$.Response.Errors.Error.Message
Issue-Key: ec2|CancelExportTask|4xx->4xx|$.Response.RequestID
Issue-Key: ec2|CancelExportTask|4xx->4xx|$headers.content-type
Issue-Key: ec2|DeleteFpgaImage|4xx->4xx|$.Response.Errors.Error.Code
Issue-Key: ec2|DeleteFpgaImage|4xx->4xx|$.Response.Errors.Error.Message
Issue-Key: ec2|DeleteFpgaImage|4xx->4xx|$.Response.RequestID
Issue-Key: ec2|DeleteFpgaImage|4xx->4xx|$headers.content-type
Issue-Key: ec2|DeleteImageUsageReport|4xx->4xx|$.Response.Errors.Error.Code
Issue-Key: ec2|DeleteImageUsageReport|4xx->4xx|$.Response.Errors.Error.Message
Issue-Key: ec2|DeleteImageUsageReport|4xx->4xx|$.Response.RequestID
Issue-Key: ec2|DeleteImageUsageReport|4xx->4xx|$headers.content-type
Issue-Key: ec2|DeleteSecondaryNetwork|4xx->4xx|$.Response.Errors.Error.Code
Issue-Key: ec2|DeleteSecondaryNetwork|4xx->4xx|$.Response.Errors.Error.Message
Issue-Key: ec2|DeleteSecondaryNetwork|4xx->4xx|$.Response.RequestID
Issue-Key: ec2|DeleteSecondaryNetwork|4xx->4xx|$headers.content-type
Issue-Key: ec2|CreateRestoreImageTask|4xx->4xx|$.Response.Errors.Error.Code
Issue-Key: ec2|CreateRestoreImageTask|4xx->4xx|$.Response.Errors.Error.Message
Issue-Key: ec2|CreateRestoreImageTask|4xx->4xx|$.Response.RequestID
Issue-Key: ec2|CreateRestoreImageTask|4xx->4xx|$headers.content-type
Issue-Commit: 6d7833292dd9d4a45d8e52e2aa7ee7e5a822f7f5

# EC2 returns NotFound codes where AWS reports Malformed identifiers

> _Note: this issue was identified against the code base at commit `6d7833292dd9d4a45d8e52e2aa7ee7e5a822f7f5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CancelExportTask

## Description
Five causes, all the same shape of defect: the emulator skips the SYNTACTIC validation of an input value and falls straight through to a lookup, so a value AWS rejects as malformed/invalid comes back with a not-found or a resource-access error code instead. The HTTP status (400) is right in every case; the Error/Code — which clients branch on — is wrong.

AWS: the EC2 Smithy model binds no error shapes to any of these operations (smithy_query query=operation returns "errors": [] for CancelExportTask, DeleteFpgaImage and CreateRestoreImageTask), so the codes below are recorded behavior only, taken from the recorded responses. The general rule the recordings show: an identifier whose SHAPE is wrong is rejected as malformed/invalid before any lookup; only a well-formed identifier that does not resolve yields a *.NotFound.

Cause 1 of 5 — CancelExportTask with a short/ill-formed export task id.
AWS: 400, Code 'InvalidID' (recorded message example: "The ID 'export-00000000' is not valid").
Emulator: 400, Code 'InvalidExportTaskID.NotFound' (message example: "The export task id 'export-00000000' does not exist").
Trace: ec2-cancel-task-notfound-error-paths#task#4.

Cause 2 of 5 — DeleteFpgaImage with a malformed afi- id.
AWS: 400, Code 'InvalidFpgaImageID.Malformed' (message example: "Image ID 'afi-00000000000000000' is invalid.").
Emulator: 400, Code 'InvalidFpgaImageID.NotFound'.
Trace: ec2-hardware-and-byoip-not-found-error-catalog#task#4.

Cause 3 of 5 — DeleteImageUsageReport with a malformed imgusgrpt- id.
AWS: 400, Code 'InvalidImageUsageReportId.Malformed'.
Emulator: 400, Code 'InvalidImageUsageReportId.NotFound'.
Trace: ec2-hardware-and-byoip-not-found-error-catalog#task#10.

Cause 4 of 5 — DeleteSecondaryNetwork with a malformed secnet- id.
AWS: 400, Code 'InvalidSecondaryNetworkId.Malformed'.
Emulator: 400, Code 'InvalidSecondaryNetworkId.NotFound'.
Trace: ec2-hardware-and-byoip-not-found-error-catalog#task#11.

Cause 5 of 5 — CreateRestoreImageTask with an S3 bucket name containing illegal characters.
AWS: 400, Code 'InvalidRequest' — the name is rejected as a malformed parameter (recorded message example: 'The S3 bucket name contains invalid characters.'). Smithy: CreateRestoreImageTask.Bucket is @required, target String, with no pattern, so the naming rule is recorded behavior only.
Emulator: 400, Code 'InaccessibleStorageLocation' — it treats an unparseable name as a reachability/permission failure on an existing location.
Trace: ec2-image-store-restore-error-paths#task#2 (also ec2-image-tasks-and-usage-report-invalid-input-errors#task#2, ec2-store-and-restore-image-mistakes#task#2).

Why: AWS clients branch on Error/Code, not on the message. '*.Malformed' / 'InvalidID' / 'InvalidRequest' are permanent client errors a caller must surface as a bad-argument bug; '*.NotFound' reads as 'the resource is gone', which callers routinely treat as idempotent success in cleanup paths, and 'InaccessibleStorageLocation' reads as a transient/IAM problem that callers retry or escalate to a permissions runbook. So a typo'd id is silently swallowed as a successful delete, and a bad bucket name sends the caller chasing S3 permissions.

Fix: the per-operation input validation of the ec2 service crate (the identifier/parameter syntax check that must run BEFORE the state lookup) and the error-code mapping it raises, in the handlers for CancelExportTask, DeleteFpgaImage, DeleteImageUsageReport, DeleteSecondaryNetwork and CreateRestoreImageTask.

Related open issue: 20261023T102725Z-ec2-picks-the-wrong-resource-specific-error-code.md describes a similar mis-pick, but it carries none of these five replay keys (the replay reports owned_by: [] for all five clusters), so they are filed here.

Traces: the five fixture#step bindings named under each cause above.

## Evidence
Replay of simparity-wire-aws-v1, 5 divergence clusters, status 400 expected and 400 returned on all five, Error/Code diverging.

1. fixture ec2-cancel-task-notfound-error-paths step task#4 (CancelExportTask) — "$.Response.Errors.Error.Code: value_mismatch (expected 'InvalidID', got 'InvalidExportTaskID.NotFound')".
2. fixture ec2-hardware-and-byoip-not-found-error-catalog step task#4 (DeleteFpgaImage) — "expected 'InvalidFpgaImageID.Malformed', got 'InvalidFpgaImageID.NotFound'".
3. fixture ec2-hardware-and-byoip-not-found-error-catalog step task#10 (DeleteImageUsageReport) — "expected 'InvalidImageUsageReportId.Malformed', got 'InvalidImageUsageReportId.NotFound'".
4. fixture ec2-hardware-and-byoip-not-found-error-catalog step task#11 (DeleteSecondaryNetwork) — "expected 'InvalidSecondaryNetworkId.Malformed', got 'InvalidSecondaryNetworkId.NotFound'".
5. fixture ec2-image-store-restore-error-paths step task#2 (CreateRestoreImageTask, Bucket='Invalid_Bucket_Name_123') — "expected 'InvalidRequest', got 'InaccessibleStorageLocation'"; cluster also covers ec2-image-tasks-and-usage-report-invalid-input-errors#task#2 and ec2-store-and-restore-image-mistakes#task#2.

The recorded Message strings quoted in the description are examples; the requirement asserted here is the status/Code pair.

## Steps to Reproduce
1. git_clone() then replay_wire_corpus().
2. Replay fixture ec2-cancel-task-notfound-error-paths, step task#4: CancelExportTask with ExportTaskId 'export-00000000' (too short to be a valid export task id). The emulator answers 400 InvalidExportTaskID.NotFound; the recording has 400 InvalidID.
3. Replay ec2-hardware-and-byoip-not-found-error-catalog steps task#4, task#10, task#11: DeleteFpgaImage / DeleteImageUsageReport / DeleteSecondaryNetwork with all-zero malformed ids. Each returns the '*.NotFound' variant where the recording has the '*.Malformed' variant.
4. Replay ec2-image-store-restore-error-paths, step task#2: CreateRestoreImageTask with Bucket 'Invalid_Bucket_Name_123'. The emulator answers 400 InaccessibleStorageLocation; the recording has 400 InvalidRequest.
