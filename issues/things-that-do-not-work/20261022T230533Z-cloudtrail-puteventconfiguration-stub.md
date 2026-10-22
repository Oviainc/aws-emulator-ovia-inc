Issue-Source: trace-sweep-divergence
Issue-Trace: cloudtrail-event-configuration-error-paths
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cloudtrail-event-configuration-error-paths cloudtrail-event-configuration-trail-and-parameter-errors cloudtrail-event-configuration-trail-not-found-error cloudtrail-event-data-store-not-found-errors
Issue-Shape-Id: com.amazonaws.cloudtrail#CloudTrail_20131101
Issue-Key: cloudtrail|stub:PutEventConfiguration
Issue-Commit: 2d1586b45d80f7c9b7f4b7e099aeb4390c46a491

# Implement cloudtrail PutEventConfiguration: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `2d1586b45d80f7c9b7f4b7e099aeb4390c46a491`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutEventConfiguration

## Description
`PutEventConfiguration` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-cloudtrail-cloudtrail-20131101/src/ops/put_event_configuration.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

6 recorded unit(s) fail on their `PutEventConfiguration` call, so each one unblocks only when `PutEventConfiguration` works: `cloudtrail-event-configuration-error-paths#task#2`, `cloudtrail-event-configuration-error-paths#task#3`, `cloudtrail-event-configuration-trail-and-parameter-errors#task#3`, `cloudtrail-event-configuration-trail-and-parameter-errors#task#4`, `cloudtrail-event-configuration-trail-not-found-error#task#2`, `cloudtrail-event-data-store-not-found-errors#task#6`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-cloudtrail-cloudtrail-20131101/src/ops/put_event_configuration.rs`. Replaying the corpus, the emulator refused the `PutEventConfiguration` call: status: expected 400, got 501
$.__type: value_mismatch (expected 'ValidationException', got 'NotImplemented')
$.message: value_mismatch (expected 'You must specify the maximum event size and context key selectors.', got 'operation amazonaws.cloudtrail.v20131101.CloudTrail_20131101.PutEventConfiguration is not implemented in this emulator')

## Steps to Reproduce
Boot the cloudtrail emulator
Call PutEventConfiguration and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
