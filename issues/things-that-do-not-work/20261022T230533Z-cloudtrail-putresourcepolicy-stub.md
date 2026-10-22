Issue-Source: trace-sweep-divergence
Issue-Trace: cloudtrail-lake-mutation-error-contracts
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cloudtrail-lake-mutation-error-contracts
Issue-Shape-Id: com.amazonaws.cloudtrail#CloudTrail_20131101
Issue-Key: cloudtrail|stub:PutResourcePolicy
Issue-Commit: 2d1586b45d80f7c9b7f4b7e099aeb4390c46a491

# Implement cloudtrail PutResourcePolicy: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `2d1586b45d80f7c9b7f4b7e099aeb4390c46a491`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutResourcePolicy

## Description
`PutResourcePolicy` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-cloudtrail-cloudtrail-20131101/src/ops/put_resource_policy.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `PutResourcePolicy` call, so each one unblocks only when `PutResourcePolicy` works: `cloudtrail-lake-mutation-error-contracts#task#4`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-cloudtrail-cloudtrail-20131101/src/ops/put_resource_policy.rs`. Replaying the corpus, the emulator refused the `PutResourcePolicy` call: status: expected 400, got 501
$.__type: value_mismatch (expected 'ResourceNotFoundException', got 'NotImplemented')
$.Message: missing (expected 'The channel arn:aws:cloudtrail:us-west-2:000000000000:channel/a1b2c3d4-e5f6-7890-abcd-ef1234567890 was not found.', got None)

## Steps to Reproduce
Boot the cloudtrail emulator
Call PutResourcePolicy and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
