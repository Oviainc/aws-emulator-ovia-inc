Issue-Source: trace-sweep-divergence
Issue-Trace: ec2-instance-dependent-ops-error-paths
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ec2-instance-dependent-ops-error-paths ec2-replace-root-volume-task
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2|stub:CreateReplaceRootVolumeTask
Issue-Commit: 91a1d12adf13e91eb189076c1b2883c7fc3c7ce7

# Implement ec2 CreateReplaceRootVolumeTask: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `91a1d12adf13e91eb189076c1b2883c7fc3c7ce7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateReplaceRootVolumeTask

## Description
`CreateReplaceRootVolumeTask` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-ec2-amazonec2/src/ops/create_replace_root_volume_task.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

2 recorded unit(s) fail on their `CreateReplaceRootVolumeTask` call, so each one unblocks only when `CreateReplaceRootVolumeTask` works: `ec2-instance-dependent-ops-error-paths#task#4`, `ec2-replace-root-volume-task#task#6`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-ec2-amazonec2/src/ops/create_replace_root_volume_task.rs`. Replaying the corpus, the emulator refused the `CreateReplaceRootVolumeTask` call: status: expected 400, got 501
$.Response.Errors.Error.Code: value_mismatch (expected 'InvalidInstanceID.Malformed', got 'NotImplemented')
$.Response.Errors.Error.Message: value_mismatch (expected "The instance ID 'i-0000000000deadbee' is malformed", got 'operation CreateReplaceRootVolumeTask is scaffolded but not implemented in this emulator')
$.Response.RequestID: missing (expected 'd7bfc074-4b1a-43fd-9f9b-931ab56a5d22', got None)
$headers.content-type: value_mismatch (expected 'text/xml;charset=UTF-8', got 'text/xml')

## Steps to Reproduce
Boot the ec2 emulator
Call CreateReplaceRootVolumeTask and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal

## Resolution

Implemented: `CreateReplaceRootVolumeTask` starts one root volume replacement task for the
instance the request names, stores it as `state::ReplaceRootVolumeTaskRecord` and answers
it read back out of the store, so a later `DescribeReplaceRootVolumeTasks` reports the same
task. The scaffold header line is gone. The four restore forms are mutually exclusive, each
resource the request names is resolved before anything is stored, the task is handed back
`pending` with the clock owing it `in-progress` and then `succeeded`, one `ClientToken`
names one task, and `DryRun` reports whether the call would have succeeded without writing.
Covered end to end by `tests/all/replace_root_volume_tasks.rs`.

The recorded refusal of this operation (`ec2-instance-dependent-ops-error-paths#task#4`)
also wanted the `<RequestID>` an EC2 error envelope closes with and the `charset` parameter
on the media type. Both are answered by this service's `override_protocol` seam
(`src/service.rs`: `RESPONSE_CONTENT_TYPE` and `with_error_request_id`), the second of which
landed on mainline while this fix was being made; with them, that step no longer diverges.

DECLINED — the `snapshotId` of the task (`ec2-replace-root-volume-task#task#6`, and the same
member of the `DescribeReplaceRootVolumeTasks` report of it): a snapshot id is a value AWS
mints per run. This emulator mints the AMI's root device snapshot id in the published
`snap-` plus 17 lowercase hexadecimal digit form when the image is stored, reports that same
id on every read of the image and of the task, and derives it from state rather than from
the request — so the only difference from the recording is the value AWS itself generated
when the recording was made. Answering with no snapshot at all was tried against the replay
and diverges too (AWS reports one here), so the recorded value cannot be matched without
fitting the recording, which is exactly what a per-run id may not be fitted to.
