Issue-Source: trace-sweep-divergence
Issue-Trace: cloudtrail-dashboard-error-paths
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cloudtrail-dashboard-error-paths
Issue-Shape-Id: com.amazonaws.cloudtrail#CloudTrail_20131101
Issue-Key: cloudtrail|stub:UpdateDashboard
Issue-Commit: 2d1586b45d80f7c9b7f4b7e099aeb4390c46a491

# Implement cloudtrail UpdateDashboard: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `2d1586b45d80f7c9b7f4b7e099aeb4390c46a491`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateDashboard

## Description
`UpdateDashboard` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-cloudtrail-cloudtrail-20131101/src/ops/update_dashboard.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `UpdateDashboard` call, so each one unblocks only when `UpdateDashboard` works: `cloudtrail-dashboard-error-paths#task#4`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-cloudtrail-cloudtrail-20131101/src/ops/update_dashboard.rs`. Replaying the corpus, the emulator refused the `UpdateDashboard` call: status: expected 400, got 501
$.__type: value_mismatch (expected 'ResourceNotFoundException', got 'NotImplemented')
$.Message: missing (expected 'No matching dashboard found.', got None)

## Steps to Reproduce
Boot the cloudtrail emulator
Call UpdateDashboard and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
