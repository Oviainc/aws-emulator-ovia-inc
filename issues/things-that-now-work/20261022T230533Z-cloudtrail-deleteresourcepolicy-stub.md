Issue-Source: trace-sweep-divergence
Issue-Trace: cloudtrail-delete-resource-policy-error-paths
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cloudtrail-delete-resource-policy-error-paths cloudtrail-delete-resource-policy-not-found-errors cloudtrail-resource-policy-error-paths cloudtrail-resource-policy-not-found-and-invalid-arn cloudtrail-tagging-channel-and-policy-not-found-error-paths
Issue-Shape-Id: com.amazonaws.cloudtrail#CloudTrail_20131101
Issue-Key: cloudtrail|stub:DeleteResourcePolicy
Issue-Commit: 2d1586b45d80f7c9b7f4b7e099aeb4390c46a491

# Implement cloudtrail DeleteResourcePolicy: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `2d1586b45d80f7c9b7f4b7e099aeb4390c46a491`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteResourcePolicy

## Description
`DeleteResourcePolicy` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-cloudtrail-cloudtrail-20131101/src/ops/delete_resource_policy.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

13 recorded unit(s) fail on their `DeleteResourcePolicy` call, so each one unblocks only when `DeleteResourcePolicy` works: `cloudtrail-delete-resource-policy-error-paths#task#1`, `cloudtrail-delete-resource-policy-error-paths#task#2`, `cloudtrail-delete-resource-policy-error-paths#task#3`, `cloudtrail-delete-resource-policy-not-found-errors#task#1`, `cloudtrail-delete-resource-policy-not-found-errors#task#2`, `cloudtrail-delete-resource-policy-not-found-errors#task#3`, `cloudtrail-resource-policy-error-paths#task#1`, `cloudtrail-resource-policy-error-paths#task#2`, `cloudtrail-resource-policy-error-paths#task#3`, `cloudtrail-resource-policy-not-found-and-invalid-arn#task#2`, `cloudtrail-resource-policy-not-found-and-invalid-arn#task#4`, `cloudtrail-resource-policy-not-found-and-invalid-arn#task#6`, `cloudtrail-tagging-channel-and-policy-not-found-error-paths#task#5`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-cloudtrail-cloudtrail-20131101/src/ops/delete_resource_policy.rs`. Replaying the corpus, the emulator refused the `DeleteResourcePolicy` call: status: expected 400, got 501
$.__type: value_mismatch (expected 'ResourceTypeNotSupportedException', got 'NotImplemented')
$.Message: missing (expected 'Resource policies are not supported for the resource: trail.', got None)

## Steps to Reproduce
Boot the cloudtrail emulator
Call DeleteResourcePolicy and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
