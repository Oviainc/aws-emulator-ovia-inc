Issue-Source: trace-sweep-divergence
Issue-Trace: ec2-image-store-restore-error-paths
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ec2-image-store-restore-error-paths ec2-image-tasks-and-usage-report-invalid-input-errors ec2-store-and-restore-image-mistakes
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2|stub:CreateStoreImageTask
Issue-Commit: 91a1d12adf13e91eb189076c1b2883c7fc3c7ce7

# Implement ec2 CreateStoreImageTask: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `91a1d12adf13e91eb189076c1b2883c7fc3c7ce7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStoreImageTask

## Description
`CreateStoreImageTask` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-ec2-amazonec2/src/ops/create_store_image_task.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

3 recorded unit(s) fail on their `CreateStoreImageTask` call, so each one unblocks only when `CreateStoreImageTask` works: `ec2-image-store-restore-error-paths#task#1`, `ec2-image-tasks-and-usage-report-invalid-input-errors#task#3`, `ec2-store-and-restore-image-mistakes#task#1`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-ec2-amazonec2/src/ops/create_store_image_task.rs`. Replaying the corpus, the emulator refused the `CreateStoreImageTask` call: status: expected 400, got 501
$.Response.Errors.Error.Code: value_mismatch (expected 'InvalidAMIID.Malformed', got 'NotImplemented')
$.Response.Errors.Error.Message: value_mismatch (expected "The image ID 'ami-0000000000deadbee' is malformed", got 'operation CreateStoreImageTask is scaffolded but not implemented in this emulator')
$.Response.RequestID: missing (expected 'c2a9d7ef-6534-4cab-885f-84d00094f8f4', got None)
$headers.content-type: value_mismatch (expected 'text/xml;charset=UTF-8', got 'text/xml')

## Steps to Reproduce
Boot the ec2 emulator
Call CreateStoreImageTask and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal

## Resolution
`CreateStoreImageTask` is implemented: it stores a task against the AMI and the bucket the
request names, answers the derived object key read back out of the store, and
`DescribeStoreImageTasks` reports the same task. Its refusals now answer EC2's own words — the
strict image-id validator (`InvalidAMIID.Malformed`, "The image ID '<id>' is malformed", for
every id it cannot resolve to an image the account can see), the malformed bucket-name
parameter `CreateRestoreImageTask` answers for the same value, and `MissingParameter` for a
required member the request carried empty.

NOT every listed unit passes. `ec2-image-store-restore-error-paths#task#1` (this issue's bound
step) and `ec2-store-and-restore-image-mistakes#task#1` replay clean.
`ec2-image-tasks-and-usage-report-invalid-input-errors` still fails: first on its `task#1`,
which is `CreateImageUsageReport`, a scaffold owned by
`20261023T102725Z-ec2-createimageusagereport-stub.md`, and then its `task#3`
`CreateStoreImageTask` call still differs on `Error.Code` and `Error.Message`. That step's
input is a different invalid input from the two fixtures above, and the replay withholds the
recorded request and the expected values, so the answer it wants is not known: it is none of
`InvalidAMIID.Malformed`, `InvalidAMIID.NotFound`, `InvalidRequest`,
`InaccessibleStorageLocation` or `MissingParameter`, each of which was tried and re-replayed.
A fixer who can read that step's recording should finish it there rather than keying a branch
to the input's shape.
