Issue-Source: trace-sweep-divergence
Issue-Trace: ec2-image-tasks-and-usage-report-invalid-input-errors
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ec2-image-tasks-and-usage-report-invalid-input-errors ec2-instance-dependent-ops-error-paths
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2|stub:CreateImageUsageReport
Issue-Commit: 91a1d12adf13e91eb189076c1b2883c7fc3c7ce7

# Implement ec2 CreateImageUsageReport: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `91a1d12adf13e91eb189076c1b2883c7fc3c7ce7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateImageUsageReport

## Description
`CreateImageUsageReport` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-ec2-amazonec2/src/ops/create_image_usage_report.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

2 recorded unit(s) fail on their `CreateImageUsageReport` call, so each one unblocks only when `CreateImageUsageReport` works: `ec2-image-tasks-and-usage-report-invalid-input-errors#task#1`, `ec2-instance-dependent-ops-error-paths#task#2`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-ec2-amazonec2/src/ops/create_image_usage_report.rs`. Replaying the corpus, the emulator refused the `CreateImageUsageReport` call: status: expected 400, got 501
$.Response.Errors.Error.Code: value_mismatch (expected 'InvalidAMIID.NotFound', got 'NotImplemented')
$.Response.Errors.Error.Message: value_mismatch (expected "The image id '[ami-00000000]' does not exist", got 'operation CreateImageUsageReport is scaffolded but not implemented in this emulator')
$.Response.RequestID: missing (expected 'd409f679-1d6c-483c-8d53-a6bcb03c9515', got None)
$headers.content-type: value_mismatch (expected 'text/xml;charset=UTF-8', got 'text/xml')

## Steps to Reproduce
Boot the ec2 emulator
Call CreateImageUsageReport and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal

## Resolution

Implemented: `CreateImageUsageReport` stores ONE AMI usage report over the image, resource
types, scope options and accounts the request names, as the modelled `ImageUsageReport` shape
`DescribeImageUsageReports` reports, and answers the `ReportId` read back out of the store.
The scaffold header line is gone. The report is handed back `pending` with the clock owing it
`available`, and carries the deletion AWS documents 30 days out; both moves live in the new
`state/image_usage_reports.rs` slice beside the store, so `DescribeImageUsageReports`
observes them rather than causing them. A launch-template resource type that names no scope
option gets the documented `version-depth` default. One `ClientToken` names one report, and a
token replayed with a different request is `IdempotentParameterMismatch`. `DryRun` reports
whether the call would have succeeded without writing. Every removal — the delete and the
clock-owed expiry — now goes through one state path that also gives back the bytes the create
charged. Covered end to end by `tests/all/create_image_usage_report.rs`.

The `ImageId` refusals follow the two recorded answers of this operation: a value in the
obfuscated long id form that names no image the account can see is
`InvalidAMIID.Malformed`, "The image ID '<id>' is malformed", while a legacy short-form id the
account holds no image for is `InvalidAMIID.NotFound`, "The image id '[<id>]' does not exist".

Still diverging in the replayed units, on causes this issue does not own and did not cause:
the `CreateImage` stub (`ec2-instance-dependent-ops-error-paths#task#1`, its own issue), the
`CreateStoreImageTask` refusal code and message
(`ec2-image-tasks-and-usage-report-invalid-input-errors#task#3`) and the `CreateSnapshots`
refusal message — the latter two surfaced only because this operation's step now passes.
