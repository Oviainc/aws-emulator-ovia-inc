Issue-Source: trace-sweep-divergence
Issue-Trace: cloudtrail-import-control-plane-error-paths
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cloudtrail-import-control-plane-error-paths cloudtrail-import-monitoring-and-control-errors cloudtrail-import-operations-not-found-errors cloudtrail-import-stop-and-list-failures-on-unknown-id cloudtrail-stop-and-list-failures-for-nonexistent-import
Issue-Shape-Id: com.amazonaws.cloudtrail#CloudTrail_20131101
Issue-Key: cloudtrail|stub:ListImportFailures
Issue-Commit: 2d1586b45d80f7c9b7f4b7e099aeb4390c46a491

# Implement cloudtrail ListImportFailures: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `2d1586b45d80f7c9b7f4b7e099aeb4390c46a491`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListImportFailures

## Description
`ListImportFailures` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-cloudtrail-cloudtrail-20131101/src/ops/list_import_failures.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

5 recorded unit(s) fail on their `ListImportFailures` call, so each one unblocks only when `ListImportFailures` works: `cloudtrail-import-control-plane-error-paths#task#2`, `cloudtrail-import-monitoring-and-control-errors#task#1`, `cloudtrail-import-operations-not-found-errors#task#3`, `cloudtrail-import-stop-and-list-failures-on-unknown-id#task#3`, `cloudtrail-stop-and-list-failures-for-nonexistent-import#task#2`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-cloudtrail-cloudtrail-20131101/src/ops/list_import_failures.rs`. Replaying the corpus, the emulator refused the `ListImportFailures` call: status: expected 200, got 501
$.Failures: missing (expected [], got None)

## Steps to Reproduce
Boot the cloudtrail emulator
Call ListImportFailures and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
